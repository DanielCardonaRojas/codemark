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
use std::collections::HashMap;

/// Maximum number of repositories that may be named in a single `repos` query.
///
/// Caps the fan-out of access checks so one call can't trigger a quota storm
/// (see "repos cardinality cap" in the discovery-safety decision doc).
const MAX_REPOS_PER_QUERY: usize = 50;

/// Request parameters for listing tours.
///
/// `GET /tours` is an **authorization-scoped lookup, not a broadcast directory**:
/// a caller must name the repositories they are asking about. There is no
/// browse-everything surface.
#[derive(Debug, Deserialize)]
pub struct ListToursParams {
    /// Required. Comma-separated repositories to look up, each as `owner/name`
    /// (full `git@`/`https://` GitHub URLs are also accepted). Missing → `400`.
    pub repos: Option<String>,
    /// Legacy single-repo filter (full URL). Folded into `repos` if present.
    pub repo_url: Option<String>,
    /// Legacy single-repo owner. Used with `repo_name`, folded into `repos`.
    pub repo_owner: Option<String>,
    /// Legacy single-repo name. Used with `repo_owner`, folded into `repos`.
    pub repo_name: Option<String>,
    /// Maximum number of tours to return (default 50, max 200).
    pub limit: Option<usize>,
    /// Number of tours to skip.
    pub offset: Option<usize>,
    /// Sort order (updated_at_desc, updated_at_asc, title_asc).
    pub sort: Option<String>,
}

/// Summary information for a single tour in a list.
///
/// Field-minimized for the authorization-scoped model: there is intentionally
/// **no `description`** (free text, highest accidental-secret risk — detail page
/// only). The `repo_url` is retained since the caller named the repo and is
/// entitled to see it.
#[derive(Debug, Serialize)]
pub struct TourSummary {
    /// Unique identifier for the tour.
    pub tour_id: String,
    /// Human-readable title.
    pub title: String,
    /// Repository URL if available.
    pub repo_url: Option<String>,
    /// GitHub login of the user who published the tour, if known.
    pub author: Option<String>,
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
    /// Total number of tours matching the (authorization-scoped) filters.
    pub total: usize,
    /// Page size used.
    pub limit: usize,
    /// Offset used.
    pub offset: usize,
}

/// Which tour visibilities the caller is entitled to see for a given repo.
#[derive(Debug, Clone, Copy)]
struct AllowedVisibility {
    /// May see `public` tours for this repo.
    public: bool,
    /// May see `private` tours for this repo.
    private: bool,
}

fn bad_request(error: &str, reason: impl Into<String>) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: error.to_string(),
            reason: Some(reason.into()),
            request_id: None,
        }),
    )
        .into_response()
}

/// Handler for GET /tours. Authorization-scoped lookup of tours for named repos.
///
/// `repos` is **required** (missing → `400 repos_required`). Results are filtered
/// to what the caller is entitled to see:
/// - **Anonymous**: tours only for **public repos** the caller named, and only
///   `public` tours. Private-repo tours are absent (no existence leak).
/// - **Authenticated**: `public` tours always; `private` tours only for repos the
///   user has **verified GitHub read access** to.
pub async fn handler(
    State(state): State<AppState>,
    auth: AuthContext,
    Query(params): Query<ListToursParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

    // 1. Collect the requested repos as a set of (owner, name), from `repos`
    //    plus the legacy single-repo params.
    let mut requested: Vec<(String, String)> = Vec::new();

    if let Some(ref repos) = params.repos {
        for raw in repos.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match GitHubVerifier::parse_repo_url(raw) {
                Some(parsed) => requested.push(parsed),
                None => {
                    return bad_request(
                        "invalid_repos",
                        format!("Could not parse repository: {}", raw),
                    );
                }
            }
        }
    }

    if let (Some(owner), Some(name)) = (&params.repo_owner, &params.repo_name) {
        requested.push((owner.clone(), name.clone()));
    } else if let Some(ref repo_url) = params.repo_url {
        match GitHubVerifier::parse_repo_url(repo_url) {
            Some(parsed) => requested.push(parsed),
            None => {
                return bad_request(
                    "invalid_repos",
                    format!("Could not parse repository URL: {}", repo_url),
                );
            }
        }
    }

    // Dedupe while preserving order.
    requested.sort();
    requested.dedup();

    if requested.is_empty() {
        return bad_request(
            "repos_required",
            "The `repos` parameter is required (comma-separated owner/name).",
        );
    }

    if requested.len() > MAX_REPOS_PER_QUERY {
        return bad_request(
            "too_many_repos",
            format!("At most {} repositories may be queried at once.", MAX_REPOS_PER_QUERY),
        );
    }

    // 2. Build the per-repo entitlement map via the appropriate access check.
    let verifier = &state.github;
    let user_id = auth.user_id();
    let mut allowed: HashMap<(String, String), AllowedVisibility> = HashMap::new();

    for (owner, name) in &requested {
        let vis = match user_id {
            // Authenticated: public tours always; private only with verified read.
            Some(user_id) => {
                let repo_url = format!("git@github.com:{}/{}.git", owner, name);
                let private = match verifier.verify_access(&state, &repo_url, user_id).await {
                    Ok(has_access) => has_access,
                    // No token / any error: fail closed — exclude private tours,
                    // but still show the repo's public tours.
                    Err(crate::github::GitHubVerifyError::NoGitHubToken) => false,
                    Err(e) => {
                        tracing::warn!(
                            owner = %owner,
                            repo = %name,
                            error = %e,
                            "Read-access check failed; excluding private tours for this repo"
                        );
                        false
                    }
                };
                AllowedVisibility { public: true, private }
            }
            // Anonymous: only public tours, and only if the repo itself is public.
            None => {
                let is_public = verifier.verify_public_repo(owner, name).await.unwrap_or(false);
                AllowedVisibility { public: is_public, private: false }
            }
        };
        allowed.insert((owner.clone(), name.clone()), vis);
    }

    // 3. Fetch ready tours and filter in Rust (repo_url formats vary: SSH vs
    //    HTTPS vs `.git` suffix), then paginate.
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
            let sort = match params.sort.as_deref() {
                Some("updated_at_asc") => "updated_at ASC",
                Some("title_asc") => "name ASC",
                _ => "updated_at DESC",
            };

            let query = format!(
                "SELECT id, name, repo_url, updated_at, created_by, visibility FROM collections
                 WHERE status = 'ready' AND visibility IN ('public', 'private')
                 ORDER BY {}",
                sort
            );

            let mut stmt = conn.prepare(&query)?;
            let rows: Vec<(TourSummary, String)> = stmt
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    let repo_url: Option<String> = row.get(2)?;
                    let visibility: String = row.get(5)?;
                    Ok((
                        TourSummary {
                            tour_id: id.clone(),
                            title: row.get(1)?,
                            repo_url,
                            updated_at: row.get(3)?,
                            author: row.get(4)?,
                            url: format!("/tours/{}", id),
                        },
                        visibility,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let filtered: Vec<TourSummary> = rows
                .into_iter()
                .filter_map(|(summary, visibility)| {
                    let parsed =
                        summary.repo_url.as_deref().and_then(GitHubVerifier::parse_repo_url)?;
                    let vis = allowed.get(&parsed)?;
                    let permitted = match visibility.as_str() {
                        "public" => vis.public,
                        "private" => vis.private,
                        _ => false,
                    };
                    if !permitted {
                        return None;
                    }
                    Some(summary)
                })
                .collect();

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
                repos = requested.len(),
                "Returning authorization-scoped tours list"
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
