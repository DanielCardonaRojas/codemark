//! Error types for the codetours server.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Server error types.
#[derive(Debug)]
pub enum ServerError {
    /// Registry database error.
    Registry(String),
    /// Storage error.
    Storage(String),
    /// Authentication error.
    Auth(String),
    /// Not found error.
    NotFound(String),
    /// Bad request error.
    BadRequest(String),
    /// Internal error.
    Internal(anyhow::Error),
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            ServerError::Registry(msg) => {
                tracing::error!("Registry error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "registry_error",
                    format!("Registry error: {}", msg),
                )
            }
            ServerError::Storage(msg) => {
                tracing::error!("Storage error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage_error",
                    format!("Storage error: {}", msg),
                )
            }
            ServerError::Auth(msg) => (StatusCode::UNAUTHORIZED, "auth_error", msg),
            ServerError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg),
            ServerError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg),
            ServerError::Internal(e) => {
                tracing::error!("Internal error: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "An internal error occurred".to_string(),
                )
            }
        };

        let body = Json(ErrorResponse { error: error_code.to_string(), message });

        (status, body).into_response()
    }
}

impl From<anyhow::Error> for ServerError {
    fn from(e: anyhow::Error) -> Self {
        ServerError::Internal(e)
    }
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::Registry(msg) => write!(f, "Registry error: {}", msg),
            ServerError::Storage(msg) => write!(f, "Storage error: {}", msg),
            ServerError::Auth(msg) => write!(f, "Auth error: {}", msg),
            ServerError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ServerError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            ServerError::Internal(e) => write!(f, "Internal error: {:?}", e),
        }
    }
}

impl std::error::Error for ServerError {}

/// Error response body.
#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}

/// Result type for server operations.
pub type Result<T> = std::result::Result<T, ServerError>;
