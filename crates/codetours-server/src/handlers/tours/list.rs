use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use crate::router::AppState;
use crate::handlers::tours::create::ErrorResponse;

#[derive(Debug, Deserialize)]
pub struct ListToursParams {
    pub repo_url: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub sort: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TourSummary {
    pub tour_id: String,
    pub title: String,
    pub repo_url: Option<String>,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct ListToursResponse {
    pub tours: Vec<TourSummary>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
}

pub async fn handler(
    State(state): State<AppState>,
    Query(params): Query<ListToursParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(200);
    let offset = params.offset.unwrap_or(0);
    
    let storage = state.storage.clone();
    let result = storage.get_conn().await.unwrap().interact(move |conn| {
        let mut query = "SELECT id, name, repo_url, updated_at FROM collections 
                         WHERE visibility = 'public' AND status = 'ready'".to_string();
        
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
            })?.collect::<rusqlite::Result<Vec<_>>>()?
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
            })?.collect::<rusqlite::Result<Vec<_>>>()?
        };

        let total: usize = conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE visibility = 'public' AND status = 'ready'",
            [],
            |row| row.get(0)
        )?;

        Ok::<_, rusqlite::Error>(ListToursResponse {
            tours,
            total,
            limit,
            offset,
        })
    }).await;

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
