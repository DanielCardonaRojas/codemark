use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;

pub async fn handler(Path(id): Path<String>) -> impl IntoResponse {
    tracing::info!(target = "heal", tour_id = %id, "Heal action triggered (stub)");
    (StatusCode::ACCEPTED, Json(json!({"status": "queued"}))).into_response()
}