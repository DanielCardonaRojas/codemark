//! GitHub OAuth authentication handlers.

use super::HandlerError;
use crate::router::AppState;
use crate::storage::registry::{self, UserUpsert};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use uuid::Uuid;

/// GitHub user info from the API.
#[derive(Debug, Deserialize)]
pub struct GithubUser {
    pub id: serde_json::Number,
    pub login: String,
}

/// GitHub OAuth token response.
#[derive(Debug, Deserialize)]
pub struct GithubTokenResponse {
    pub access_token: String,
}

/// Query parameters for the OAuth callback.
#[derive(Debug, Deserialize)]
pub struct CallbackQueryParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub redirect_uri: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Initiate GitHub OAuth login.
///
/// Redirects the user to GitHub's authorization page.
pub async fn github_login(
    State(state): State<AppState>,
    Query(params): Query<CallbackQueryParams>,
) -> Result<Redirect, HandlerError> {
    let config = &state.config.auth.github;

    if config.client_id.is_empty() {
        return Err(HandlerError::BadRequest(
            "GitHub OAuth is not configured on the server".to_string(),
        ));
    }

    // Generate a state parameter for CSRF protection
    let state_param = Uuid::new_v4().to_string();

    // Get redirect_uri from query params or use default
    let redirect_uri = params.redirect_uri.clone().unwrap_or_else(|| config.callback_url.clone());

    let auth_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read:org&state={}",
        config.client_id,
        urlencoding::encode(&redirect_uri),
        state_param
    );

    Ok(Redirect::to(&auth_url))
}

/// Handle GitHub OAuth callback.
///
/// Supports two modes:
/// 1. Header-based flow (CLI): Authorization code in `x-code` header
/// 2. Query-based flow (Browser): Authorization code in query parameters
///
/// Exchanges the authorization code for an access token,
/// fetches user info, and creates/updates the user in the registry.
/// Returns a JWT session token to the client.
pub async fn github_callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQueryParams>,
    headers: HeaderMap,
) -> Result<Response, HandlerError> {
    // Check for OAuth error response
    if let Some(error) = &query.error {
        let error_desc = query.error_description.as_deref().unwrap_or("Unknown error").to_string();
        return Err(HandlerError::BadRequest(format!(
            "GitHub authorization error: {} - {}",
            error, error_desc
        )));
    }

    // Get code from header or query params
    let code = headers
        .get("x-code")
        .and_then(|h| h.to_str().ok())
        .or(query.code.as_deref())
        .ok_or_else(|| HandlerError::BadRequest("Missing authorization code".to_string()))?;

    let config = &state.config.auth.github;

    // Exchange code for access token
    let client = reqwest::Client::new();
    let token_resp_bytes = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&serde_json::json!({
            "client_id": config.client_id,
            "client_secret": config.client_secret,
            "code": code,
        }))
        .send()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to contact GitHub: {}", e)))?
        .bytes()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to read token response: {}", e)))?;

    let token_resp: GithubTokenResponse = serde_json::from_slice(&token_resp_bytes)
        .map_err(|e| HandlerError::Internal(format!("Failed to parse token response: {}", e)))?;

    // Fetch user info
    let user_resp: GithubUser = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", token_resp.access_token))
        .header("User-Agent", "codetours-server")
        .send()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to fetch user: {}", e)))?
        .json()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to parse user response: {}", e)))?;

    // Get or create user in registry
    let github_id = user_gp_id_to_string(&user_resp.id);
    let user_id = format!("user-{}", Uuid::new_v4());
    let github_login = user_resp.login.clone();
    let access_token = token_resp.access_token.clone();

    // Get registry connection from pool
    let registry_conn = state.registry.get_conn().await;

    // Use interact to run the sync registry operations
    let user_id = registry_conn
        .interact(move |conn| {
            // Check if user exists
            let existing_user = registry::find_user_by_github_id(conn, &github_id)
                .map_err(|e| HandlerError::Internal(format!("Failed to query user: {}", e)))?;

            let user_id = if let Some(existing) = existing_user {
                // Update last login and token
                registry::upsert_user(
                    conn,
                    &UserUpsert {
                        id: &existing.id,
                        github_id: &github_id,
                        github_login: &github_login,
                        github_token: Some(&access_token),
                    },
                )
                .map_err(|e| HandlerError::Internal(format!("Failed to update user: {}", e)))?;
                existing.id
            } else {
                // Create new user
                registry::upsert_user(
                    conn,
                    &UserUpsert {
                        id: &user_id,
                        github_id: &github_id,
                        github_login: &github_login,
                        github_token: Some(&access_token),
                    },
                )
                .map_err(|e| HandlerError::Internal(format!("Failed to create user: {}", e)))?;
                user_id
            };

            Ok::<String, HandlerError>(user_id)
        })
        .await
        .map_err(|e| HandlerError::Internal(format!("Registry operation failed: {}", e)))?
        .map_err(|e| HandlerError::Internal(format!("Registry operation error: {}", e)))?;

    // Generate JWT session token
    let session_token = generate_jwt(&user_id, config)?;

    // Return the session token as JSON
    Ok((
        StatusCode::OK,
        [(("content-type"), "application/json")],
        serde_json::json!({ "token": session_token }).to_string(),
    )
        .into_response())
}

/// Generate a JWT token for the user.
fn generate_jwt(
    user_id: &str,
    config: &crate::config::GithubAuthConfig,
) -> Result<String, HandlerError> {
    use jsonwebtoken::{EncodingKey, Header, encode};

    if config.jwt_secret.is_empty() {
        return Err(HandlerError::Internal("JWT secret is not configured".to_string()));
    }

    let claims = serde_json::json!({
        "sub": user_id,
        "exp": chrono::Utc::now().timestamp() + config.session_expires_in as i64,
    });

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.jwt_secret.as_bytes()),
    )
    .map_err(|e| HandlerError::Internal(format!("Failed to generate token: {}", e)))?;

    Ok(token)
}

/// Convert GitHub ID (which can be a large number) to a string.
fn user_gp_id_to_string(id: &serde_json::Number) -> String {
    id.as_u64()
        .map(|n| n.to_string())
        .or_else(|| id.as_i64().map(|n| n.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_github_id_to_string() {
        let small = serde_json::Number::from(12345_u64);
        assert_eq!(user_gp_id_to_string(&small), "12345");

        let large = serde_json::Number::from(9876543210_u64);
        assert_eq!(user_gp_id_to_string(&large), "9876543210");
    }
}
