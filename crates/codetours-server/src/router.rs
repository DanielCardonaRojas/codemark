use crate::config::Config;
use crate::handlers::{self, tours};
use crate::observability::request_id_middleware;
use crate::storage::StorageManager;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::{Router, middleware, routing::get};
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub storage: Arc<StorageManager>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::home::handler))
        .route("/health", get(handlers::health::handler))
        .route("/static/*path", get(handlers::static_assets::handler))
        .route("/my-tours", get(handlers::my_tours::handler))
        .route("/config", get(handlers::config::page_handler))
        .route("/config/prefs", axum::routing::post(handlers::config::prefs_handler))
        .route("/config/heal", axum::routing::post(handlers::config::stub_handler))
        .route("/config/maintenance", axum::routing::post(handlers::config::stub_handler))
        .route("/tours", get(tours::list::handler).post(tours::create::handler))
        .route("/tours/:id", get(tours::get::handler).delete(tours::delete::handler))
        .route("/tours/:id/permalink/:ordinal", get(tours::permalink::handler))
        .route("/tours/:id/publish", axum::routing::post(tours::publish::handler))
        .route("/tours/:id/heal", axum::routing::post(tours::heal::handler))
        .route("/tours/:id/links", axum::routing::post(tours::links::add_handler))
        .route("/tours/:id/bookmarks/:bid/comments", axum::routing::post(tours::comments::create_handler))
        .layer(SetResponseHeaderLayer::overriding(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none';"),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(request_id_middleware))
        .with_state(state)
}
