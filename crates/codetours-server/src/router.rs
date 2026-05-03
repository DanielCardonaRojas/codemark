use axum::{
    routing::get,
    Router,
    middleware,
};
use tower_http::trace::TraceLayer;
use crate::handlers;
use crate::observability::request_id_middleware;
use crate::config::Config;
use crate::storage::StorageManager;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<StorageManager>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health::handler))
        .layer(middleware::from_fn(request_id_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
