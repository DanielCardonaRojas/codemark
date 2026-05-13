use crate::config::Config;
use crate::handlers::{self, auth, tours};
use crate::observability::request_id_middleware;
use crate::storage::{StorageManager, registry::RegistryManager};
use axum::{
    Router, middleware,
    routing::{get, post},
};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

/// Application state shared across all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<StorageManager>,
    pub registry: Arc<RegistryManager>,
}

impl AppState {
    /// Create a new AppState with the registry initialized.
    pub fn new(config: Config, storage: StorageManager) -> anyhow::Result<Self> {
        let registry = RegistryManager::new(&config.data_dir)?;
        Ok(Self {
            config: Arc::new(config),
            storage: Arc::new(storage),
            registry: Arc::new(registry),
        })
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(handlers::health::handler))
        .route("/auth/github", get(auth::github_login))
        .route("/auth/github/device", get(auth::github_device_login))
        .route("/auth/github/device/poll", post(auth::github_device_poll))
        .route("/auth/github/callback", get(auth::github_callback))
        .route("/callback", get(auth::github_callback))
        .route("/tours", get(tours::list::handler).post(tours::create::handler))
        .route("/tours/:id", get(tours::get::handler).delete(tours::delete::handler))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
}
