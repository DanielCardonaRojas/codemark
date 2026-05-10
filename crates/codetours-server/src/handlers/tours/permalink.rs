use axum::{
    extract::Path,
    response::{IntoResponse, Redirect},
};

pub async fn handler(Path((id, ordinal)): Path<(String, usize)>) -> impl IntoResponse {
    Redirect::to(&format!("/tours/{}#step-{}", id, ordinal))
}
