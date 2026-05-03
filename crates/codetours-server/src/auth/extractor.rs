use axum::{
    async_trait,
    extract::{FromRef, FromRequestParts},
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use crate::router::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Scope {
    Publish,
    Read,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthContext {
    Authenticated { user_id: String, scopes: Vec<Scope> },
    Anonymous,
}

impl AuthContext {
    pub fn is_authenticated(&self) -> bool {
        matches!(self, AuthContext::Authenticated { .. })
    }

    pub fn has_scope(&self, scope: Scope) -> bool {
        match self {
            AuthContext::Authenticated { scopes, .. } => {
                scopes.iter().any(|s| matches!((s, &scope), (Scope::Publish, Scope::Publish) | (Scope::Read, Scope::Read) | (Scope::Delete, Scope::Delete)))
            }
            AuthContext::Anonymous => false,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AuthErrorResponse {
    pub error: String,
    pub message: String,
}

#[derive(Debug)]
pub enum AuthError {
    InvalidToken,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, error_code, message) = match self {
            AuthError::InvalidToken => (
                StatusCode::UNAUTHORIZED,
                "invalid_token",
                "X-Tour-Token header is invalid",
            ),
        };

        let body = Json(AuthErrorResponse {
            error: error_code.to_string(),
            message: message.to_string(),
        });

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

        if token != state.config.auth.dev_token {
            return Err(AuthError::InvalidToken);
        }

        Ok(AuthContext::Authenticated {
            user_id: "stub".to_string(),
            scopes: vec![Scope::Publish, Scope::Read, Scope::Delete],
        })
    }
}
