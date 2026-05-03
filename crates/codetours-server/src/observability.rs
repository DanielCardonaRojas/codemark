use crate::config::Config;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use axum::{
    extract::Request,
    middleware::Next,
    response::Response,
    http::HeaderValue,
};
use uuid::Uuid;
use tracing::{info_span, Instrument};

pub fn init_tracing(config: &Config, json_logs: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.log_level));

    let registry = tracing_subscriber::registry().with(filter);

    if json_logs {
        registry
            .with(fmt::layer().json().with_target(false))
            .init();
    } else {
        registry
            .with(fmt::layer().with_target(false))
            .init();
    }
}

pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    
    // Add to request extensions
    req.extensions_mut().insert(RequestId(request_id.clone()));

    let span = info_span!(
        "request",
        request_id = %request_id,
        method = %req.method(),
        uri = %req.uri(),
    );

    let mut response = next.run(req).instrument(span).await;
    
    // Echo back in header
    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("X-Request-Id", header_value);
    }
    
    response
}

#[derive(Debug, Clone)]
pub struct RequestId(pub String);
