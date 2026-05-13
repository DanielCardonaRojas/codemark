//! Authentication context extractor with JWT and stub support.

use crate::router::AppState;
use axum::{
    Json, async_trait,
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use jsonwebtoken::{DecodingKey, decode};
use serde::{Deserialize, Serialize};

/// Leeway for clock skew in JWT validation (30 seconds).
const LEEWAY: u64 = 30;

/// Scope represents a permission granted to an authenticated user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Scope {
    /// Permission to publish new tours.
    Publish,
    /// Permission to read private tours.
    Read,
    /// Permission to delete tours.
    Delete,
}

/// AuthContext represents the authentication state of a request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthContext {
    /// The request is authenticated with JWT token.
    Jwt {
        /// Unique identifier for the user.
        user_id: String,
    },
    /// The request is authenticated via stub auth (dev mode).
    Stub {
        /// Stub user identifier.
        user_id: String,
    },
    /// The request is anonymous.
    Anonymous,
}

impl AuthContext {
    /// Returns true if the request is authenticated.
    pub fn is_authenticated(&self) -> bool {
        !matches!(self, AuthContext::Anonymous)
    }

    /// Returns the user ID if authenticated.
    pub fn user_id(&self) -> Option<&str> {
        match self {
            AuthContext::Jwt { user_id } | AuthContext::Stub { user_id } => Some(user_id),
            AuthContext::Anonymous => None,
        }
    }
}

/// Response body for authentication errors.
#[derive(Debug, Serialize)]
pub struct AuthErrorResponse {
    /// Short machine-readable error code.
    pub error: String,
    /// Human-readable error message.
    pub message: String,
}

/// Possible authentication errors.
#[derive(Debug)]
pub enum AuthError {
    /// The Authorization header is missing or invalid.
    MissingOrInvalidToken,
    /// JWT token validation failed.
    InvalidJwt(String),
    /// Server is misconfigured.
    ServerMisconfigured,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::MissingOrInvalidToken => write!(f, "Missing or invalid authorization token"),
            AuthError::InvalidJwt(msg) => write!(f, "Invalid JWT: {}", msg),
            AuthError::ServerMisconfigured => write!(f, "Authentication server is misconfigured"),
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            AuthError::MissingOrInvalidToken => (
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "Authorization header is missing or invalid".to_string(),
            ),
            AuthError::InvalidJwt(msg) => (StatusCode::UNAUTHORIZED, "invalid_jwt", msg),
            AuthError::ServerMisconfigured => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Authentication server is misconfigured".to_string(),
            ),
        };

        let body = Json(AuthErrorResponse { error: error_code.to_string(), message });

        (status, body).into_response()
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthContext
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // First, try JWT authentication
        if let Some(auth_header) = parts.headers.get("Authorization") {
            let auth_str = auth_header.to_str().map_err(|_| AuthError::MissingOrInvalidToken)?;

            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                // Validate JWT token
                let state = AppState::from_ref(_state);
                let config = &state.config.auth.github;

                let jwt_secret = config.get_jwt_secret();
                if !jwt_secret.is_empty() {
                    match decode_jwt(token, &jwt_secret) {
                        Ok(user_id) => return Ok(AuthContext::Jwt { user_id }),
                        Err(e) => {
                            tracing::error!("JWT validation failed: {}", e);
                            return Err(e);
                        }
                    }
                } else {
                    tracing::error!("JWT secret is missing from server configuration");
                    return Err(AuthError::ServerMisconfigured);
                }
            } else {
                tracing::warn!("Authorization header provided but missing 'Bearer ' prefix");
                return Err(AuthError::MissingOrInvalidToken);
            }
        }

        // Fall back to stub auth if X-Tour-Token is provided
        let token = parts.headers.get("X-Tour-Token").and_then(|v| v.to_str().ok());

        if let Some(token) = token {
            let state = AppState::from_ref(_state);

            // Ensure dev_token is non-empty to prevent bypass
            if state.config.auth.dev_token.is_empty() {
                tracing::error!("Auth mode is 'stub' but dev_token is empty");
                return Err(AuthError::ServerMisconfigured);
            }

            if token == state.config.auth.dev_token {
                return Ok(AuthContext::Stub { user_id: "stub".to_string() });
            }
            return Err(AuthError::MissingOrInvalidToken);
        }

        Ok(AuthContext::Anonymous)
    }
}

/// Validate a JWT token and return the user ID.
fn decode_jwt(token: &str, secret: &str) -> Result<String, AuthError> {
    use jsonwebtoken::Validation;
    use serde_json::Value;

    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;
    validation.leeway = LEEWAY;

    match decode::<Value>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation) {
        Ok(token_data) => {
            if let Some(user_id) = token_data.claims.get("sub").and_then(|v: &Value| v.as_str()) {
                Ok(user_id.to_string())
            } else {
                Err(AuthError::InvalidJwt("Token missing 'sub' claim".to_string()))
            }
        }
        Err(e) => Err(AuthError::InvalidJwt(e.to_string())),
    }
}
