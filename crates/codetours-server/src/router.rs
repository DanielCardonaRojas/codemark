use crate::config::Config;
use crate::handlers;
use crate::observability::request_id_middleware;
use crate::storage::StorageManager;
use axum::{Router, middleware, routing::get};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<StorageManager>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health::handler))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
}
