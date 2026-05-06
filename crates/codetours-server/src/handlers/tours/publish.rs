use axum::{extract::Path, http::StatusCode, response::IntoResponse};

// M2 Stub: Publishing is done via CLI (Phase 4). Web UI publishing will land in a future phase.
pub async fn handler(Path(_id): Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "Publishing drafts from the Web UI is not implemented yet. Please use the Codemark CLI.",
    )
        .into_response()
}
