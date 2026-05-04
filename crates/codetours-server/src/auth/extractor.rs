use crate::router::AppState;
use axum::{
    Json, async_trait,
    extract::{FromRef, FromRequestParts},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

/// Scope represents a permission granted to an authenticated user.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// The request is authenticated.
    Authenticated {
        /// Unique identifier for the user.
        user_id: String,
        /// List of permissions granted to the user.
        scopes: Vec<Scope>,
    },
    /// The request is anonymous.
    Anonymous,
}

impl AuthContext {
    /// Returns true if the request is authenticated.
    pub fn is_authenticated(&self) -> bool {
        matches!(self, AuthContext::Authenticated { .. })
    }

    /// Returns true if the authenticated user has the specified scope.
    pub fn has_scope(&self, scope: Scope) -> bool {
        match self {
            AuthContext::Authenticated { scopes, .. } => scopes.iter().any(|s| {
                matches!(
                    (s, &scope),
                    (Scope::Publish, Scope::Publish)
                        | (Scope::Read, Scope::Read)
                        | (Scope::Delete, Scope::Delete)
                )
            }),
            AuthContext::Anonymous => false,
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
    /// The X-Tour-Token header is invalid.
    InvalidToken,
    /// Server is misconfigured (missing dev_token in stub mode).
    ServerMisconfigured,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            AuthError::InvalidToken => {
                (StatusCode::UNAUTHORIZED, "invalid_token", "X-Tour-Token header is invalid")
            }
            AuthError::ServerMisconfigured => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Authentication server is misconfigured",
            ),
        };

        let body =
            Json(AuthErrorResponse { error: error_code.to_string(), message: message.to_string() });

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

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);

        let token = match parts.headers.get("X-Tour-Token").and_then(|v| v.to_str().ok()) {
            Some(t) => t,
            None => return Ok(AuthContext::Anonymous),
        };

        // M1: Stub auth
        // Ensure dev_token is non-empty to prevent bypass
        if state.config.auth.dev_token.is_empty() {
            tracing::error!("Auth mode is 'stub' but dev_token is empty");
            return Err(AuthError::ServerMisconfigured);
        }

        if token != state.config.auth.dev_token {
            return Err(AuthError::InvalidToken);
        }

        Ok(AuthContext::Authenticated {
            user_id: "stub".to_string(),
            scopes: vec![Scope::Publish, Scope::Read, Scope::Delete],
        })
    }
}
