pub mod auth;
pub mod health;
pub mod tours;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Common handler error type.
pub enum HandlerError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Internal(String),
}

impl IntoResponse for HandlerError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            HandlerError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            HandlerError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            HandlerError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            HandlerError::Internal(msg) => {
                tracing::error!("Handler error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error".to_string())
            }
        };

        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl std::fmt::Display for HandlerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandlerError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            HandlerError::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            HandlerError::NotFound(msg) => write!(f, "Not found: {}", msg),
            HandlerError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}
