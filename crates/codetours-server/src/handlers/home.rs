use axum::response::{IntoResponse, Redirect};

pub async fn handler() -> impl IntoResponse {
    Redirect::temporary("/tours")
}
