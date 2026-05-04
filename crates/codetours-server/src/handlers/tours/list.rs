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
    /// Optional repository URL filter.
    pub repo_url: Option<String>,
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
pub async fn handler(
    State(state): State<AppState>,
    Query(params): Query<ListToursParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);

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
            let mut query = "SELECT id, name, repo_url, updated_at FROM collections 
                         WHERE visibility = 'public' AND status = 'ready'"
                .to_string();

            if params.repo_url.is_some() {
                query.push_str(" AND repo_url = ?3");
            }

            let sort = match params.sort.as_deref() {
                Some("updated_at_asc") => "updated_at ASC",
                Some("title_asc") => "name ASC",
                _ => "updated_at DESC",
            };
            query.push_str(&format!(" ORDER BY {}", sort));
            query.push_str(" LIMIT ?1 OFFSET ?2");

            let mut stmt = conn.prepare(&query)?;

            let tours = if let Some(repo_url) = &params.repo_url {
                stmt.query_map([limit.to_string(), offset.to_string(), repo_url.clone()], |row| {
                    let id: String = row.get(0)?;
                    Ok(TourSummary {
                        tour_id: id.clone(),
                        title: row.get(1)?,
                        repo_url: row.get(2)?,
                        updated_at: row.get(3)?,
                        url: format!("/tours/{}", id),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map([limit.to_string(), offset.to_string()], |row| {
                    let id: String = row.get(0)?;
                    Ok(TourSummary {
                        tour_id: id.clone(),
                        title: row.get(1)?,
                        repo_url: row.get(2)?,
                        updated_at: row.get(3)?,
                        url: format!("/tours/{}", id),
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };

            let mut count_query =
                "SELECT COUNT(*) FROM collections WHERE visibility = 'public' AND status = 'ready'"
                    .to_string();
            if params.repo_url.is_some() {
                count_query.push_str(" AND repo_url = ?1");
            }

            let total: usize = if let Some(repo_url) = &params.repo_url {
                conn.query_row(&count_query, [repo_url], |row| row.get(0))?
            } else {
                conn.query_row(&count_query, [], |row| row.get(0))?
            };

            Ok::<_, rusqlite::Error>(ListToursResponse { tours, total, limit, offset })
        })
        .await;

    match result {
        Ok(Ok(res)) => (StatusCode::OK, Json(res)).into_response(),
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
