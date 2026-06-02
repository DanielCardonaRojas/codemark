use crate::auth::AuthContext;
use crate::github::GitHubVerifier;
use crate::handlers::tours::create::ErrorResponse;
use crate::router::AppState;
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};

/// Request parameters for listing tours.
#[derive(Debug, Deserialize)]
pub struct ListToursParams {
    /// Optional repository URL filter (legacy, use repo_owner + repo instead).
    pub repo_url: Option<String>,
    /// Repository owner (e.g. "DanielCardonaRojas"). Preferred over repo_url.
    pub repo_owner: Option<String>,
    /// Repository name (e.g. "codemark"). Used with repo_owner.
    pub repo_name: Option<String>,
    /// Maximum number of tours to return (default 50, max 200).
    pub limit: Option<usize>,
    /// Number of tours to skip.
    pub offset: Option<usize>,
    /// Sort order (updated_at_desc, updated_at_asc, title_asc).
    pub sort: Option<String>,
}

/// Summary information for a single tour in a list.
#[derive(Debug, Serialize)]
pub struct TourSummary {
    /// Unique identifier for the tour.
    pub tour_id: String,
    /// Human-readable title.
    pub title: String,
    /// Repository URL if available.
    pub repo_url: Option<String>,
    /// Timestamp of last update.
    pub updated_at: String,
    /// Relative API URL for this tour.
    pub url: String,
}

/// Paginated response body for listing tours.
#[derive(Debug, Serialize)]
pub struct ListToursResponse {
    /// List of tours in the current page.
    pub tours: Vec<TourSummary>,
    /// Total number of public tours matching the filters.
    pub total: usize,
    /// Page size used.
    pub limit: usize,
    /// Offset used.
    pub offset: usize,
}

/// Handler for GET /tours. Lists public tours with filtering and pagination.
///
/// Supports filtering by repository using either:
/// - `repo_owner` + `repo_name` (preferred, format-agnostic)
/// - `repo_url` (legacy, exact match)
///
/// When repository filtering is used, verifies the user has access to that
/// repository via GitHub API before returning tours.
pub async fn handler(
    State(state): State<AppState>,
    auth: AuthContext,
    Query(params): Query<ListToursParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    // Resolve repo filter: prefer repo_owner+repo_name, fall back to repo_url
    let repo_filter: Option<(String, String)> =
        if let (Some(owner), Some(name)) = (&params.repo_owner, &params.repo_name) {
            Some((owner.clone(), name.clone()))
        } else if let Some(ref repo_url) = params.repo_url {
            GitHubVerifier::parse_repo_url(repo_url)
        } else {
            None
        };

    // If filtering by repo, verify GitHub access
    if let Some((ref owner, ref repo)) = repo_filter {
        let repo_url_for_access = format!("git@github.com:{}/{}.git", owner, repo);

        let user_id = match auth.user_id() {
            Some(id) => id,
            None => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "unauthorized".to_string(),
                        reason: Some("Authentication required to filter by repository".to_string()),
                        request_id: None,
                    }),
                )
                    .into_response();
            }
        };

        let verifier = GitHubVerifier::new();

        match verifier.verify_access(&state, &repo_url_for_access, user_id).await {
            Ok(has_access) => {
                if !has_access {
                    tracing::warn!(
                        repo_owner = %owner,
                        repo_name = %repo,
                        "Access denied: user does not have access to repository"
                    );
                    return (
                        StatusCode::FORBIDDEN,
                        Json(ErrorResponse {
                            error: "access_denied".to_string(),
                            reason: Some("You do not have access to this repository".to_string()),
                            request_id: None,
                        }),
                    )
                        .into_response();
                }
                tracing::debug!(repo_owner = %owner, repo_name = %repo, "Access granted to repository");
            }
            Err(crate::github::GitHubVerifyError::NoGitHubToken) => {
                tracing::warn!(
                    repo_owner = %owner,
                    repo_name = %repo,
                    "Access check failed: no GitHub token linked"
                );
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(ErrorResponse {
                        error: "unauthorized".to_string(),
                        reason: Some(
                            "GitHub account must be linked to access this repository".to_string(),
                        ),
                        request_id: None,
                    }),
                )
                    .into_response();
            }
            Err(e) => {
                tracing::error!(
                    repo_owner = %owner,
                    repo_name = %repo,
                    error = %e,
                    "Failed to verify GitHub access"
                );
                // For now, allow access if verification fails (graceful degradation)
            }
        }
    }

    let storage = state.storage.clone();

    let conn = match storage.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to acquire DB connection: {}", e);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    let result = conn
        .interact(move |conn| {
            // Always fetch all public/ready tours, then filter by parsed owner/repo in Rust.
            // This avoids fragile exact-string matching on repo_url (SSH vs HTTPS vs .git suffix).
            let sort = match params.sort.as_deref() {
                Some("updated_at_asc") => "updated_at ASC",
                Some("title_asc") => "name ASC",
                _ => "updated_at DESC",
            };

            let query = format!(
                "SELECT id, name, repo_url, updated_at FROM collections
                 WHERE visibility = 'public' AND status = 'ready'
                 ORDER BY {}",
                sort
            );

            let mut stmt = conn.prepare(&query)?;
            let all_tours: Vec<TourSummary> = stmt
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    Ok(TourSummary {
                        tour_id: id.clone(),
                        title: row.get(1)?,
                        repo_url: row.get(2)?,
                        updated_at: row.get(3)?,
                        url: format!("/tours/{}", id),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Filter by owner/repo if provided
            let filtered: Vec<TourSummary> = if let Some((ref owner, ref repo)) = repo_filter {
                all_tours
                    .into_iter()
                    .filter(|t| {
                        t.repo_url.as_deref().and_then(GitHubVerifier::parse_repo_url)
                            == Some((owner.clone(), repo.clone()))
                    })
                    .collect()
            } else {
                all_tours
            };

            let total = filtered.len();
            let paged: Vec<TourSummary> = filtered.into_iter().skip(offset).take(limit).collect();

            Ok::<_, rusqlite::Error>(ListToursResponse { tours: paged, total, limit, offset })
        })
        .await;

    match result {
        Ok(Ok(res)) => {
            tracing::info!(
                tours_count = res.tours.len(),
                total = res.total,
                "Returning tours list"
            );
            (StatusCode::OK, Json(res)).into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal".to_string(),
                reason: Some(e.to_string()),
                request_id: None,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal".to_string(),
                reason: Some(e.to_string()),
                request_id: None,
            }),
        )
            .into_response(),
    }
}
