use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::auth::AuthContext;
use crate::pack_cache::PackCache;
use crate::router::AppState;

/// Query parameters for fetching a tour.
#[derive(Debug, Deserialize)]
pub struct GetTourParams {
    /// Desired format: 'pack' for binary SQLite pack, or omitted for JSON.
    pub format: Option<String>,
}

/// Detailed information about a tour, including its bookmarks.
#[derive(Debug, Serialize)]
pub struct TourDetail {
    /// Unique identifier for the tour.
    pub tour_id: String,
    /// Human-readable title.
    pub title: String,
    /// Detailed description.
    pub description: Option<String>,
    /// Repository URL.
    pub repo_url: Option<String>,
    /// ISO 8601 timestamp of publication.
    pub published_at: String,
    /// List of bookmarks in the tour.
    pub bookmarks: Vec<BookmarkDetail>,
}

/// Information about a single bookmark within a tour.
#[derive(Debug, Serialize)]
pub struct BookmarkDetail {
    /// Unique identifier for the bookmark.
    pub id: String,
    /// Path to the file containing the bookmark.
    pub file_path: String,
    /// Resolved line range.
    pub line_range: Option<String>,
    /// Render snapshot from the latest resolution.
    pub snapshot: Option<ResolutionSnapshot>,
}

/// A snapshot of a bookmark's resolution at a point in time.
#[derive(Debug, Serialize)]
pub struct ResolutionSnapshot {
    /// Human-readable headline describing the code at this bookmark.
    pub headline: Option<String>,
    /// Preview lines showing the code around the bookmark.
    pub preview_lines: Option<String>,
}

/// Handler for GET /tours/:id. Returns tour details in JSON or binary pack format.
pub async fn handler(
    State(state): State<AppState>,
    auth: AuthContext,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(params): Query<GetTourParams>,
) -> impl IntoResponse {
    let accept =
        headers.get(header::ACCEPT).and_then(|v| v.to_str().ok()).unwrap_or("application/json");

    let is_pack_request = params.format.as_deref() == Some("pack")
        || accept == "application/vnd.codetours.pack+sqlite";

    if is_pack_request {
        // Check visibility/auth before serving the pack
        let storage = state.storage.clone();
        let id_clone = id.clone();
        let is_auth = auth.is_authenticated();

        let conn = match storage.get_conn().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to acquire DB connection: {}", e);
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        };

        let allowed = conn
            .interact(move |conn| {
                let sql = if is_auth {
                    "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1 AND visibility IS NOT NULL)"
                } else {
                    "SELECT EXISTS(SELECT 1 FROM collections WHERE id = ?1 AND visibility = 'public')"
                };
                conn.query_row(sql, [&id_clone], |row| row.get::<_, bool>(0))
            })
            .await;

        let allowed = match allowed {
            Ok(Ok(allowed)) => allowed,
            _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };

        if !allowed {
            return StatusCode::NOT_FOUND.into_response();
        }

        let cache = PackCache::new(state.config.data_dir.clone());
        let pack_path = cache.get_pack_path(&id);

        if let Some(path) = pack_path.filter(|p| p.exists()) {
            match fs::read(&path).await {
                Ok(bytes) => {
                    return (
                        [(header::CONTENT_TYPE, "application/vnd.codetours.pack+sqlite")],
                        bytes,
                    )
                        .into_response();
                }
                Err(e) => {
                    tracing::error!("Failed to read pack from cache: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        return StatusCode::NOT_FOUND.into_response();
    }

    // JSON response
    let storage = state.storage.clone();
    let is_auth = auth.is_authenticated();

    let conn = match storage.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to acquire DB connection: {}", e);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
    };

    let result = conn
        .interact(move |conn| {
            // 1. Get collection
            let sql = if is_auth {
                "SELECT id, name, description, repo_url, published_at FROM collections 
             WHERE id = ?1 AND visibility IS NOT NULL"
            } else {
                "SELECT id, name, description, repo_url, published_at FROM collections 
             WHERE id = ?1 AND visibility = 'public'"
            };

            let tour: TourDetail = conn
                .query_row(sql, [&id], |row| {
                    Ok(TourDetail {
                        tour_id: row.get(0)?,
                        title: row.get(1)?,
                        description: row.get(2)?,
                        repo_url: row.get(3)?,
                        published_at: row.get(4)?,
                        bookmarks: Vec::new(),
                    })
                })
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => StatusCode::NOT_FOUND,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                })?;

            // 2. Get bookmarks and join with latest resolution
            // Security: Use a deterministic latest-resolution query (correlated subquery)
            let mut stmt = conn
                .prepare(
                    "SELECT b.id, b.file_path, r.line_range, r.headline, r.preview_lines
             FROM collection_bookmarks cb
             JOIN bookmarks b ON cb.bookmark_id = b.id
             LEFT JOIN resolutions r ON r.id = (
                 SELECT id FROM resolutions 
                 WHERE bookmark_id = b.id 
                 ORDER BY resolved_at DESC LIMIT 1
             )
             WHERE cb.collection_id = ?1
             ORDER BY cb.position ASC",
                )
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let bookmarks = stmt
                .query_map([&id], |row| {
                    Ok(BookmarkDetail {
                        id: row.get(0)?,
                        file_path: row.get(1)?,
                        line_range: row.get(2)?,
                        snapshot: row.get::<_, Option<String>>(3)?.map(|h| ResolutionSnapshot {
                            headline: Some(h),
                            preview_lines: row.get(4).ok(),
                        }),
                    })
                })
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

            let mut tour = tour;
            tour.bookmarks = bookmarks;

            Ok::<_, StatusCode>(tour)
        })
        .await;

    match result {
        Ok(Ok(tour)) => (StatusCode::OK, Json(tour)).into_response(),
        Ok(Err(status)) => status.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
