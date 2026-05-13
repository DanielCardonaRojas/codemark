//! GitHub OAuth authentication handlers.

use super::HandlerError;
use crate::router::AppState;
use crate::storage::registry::{self, UserUpsert};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use uuid::Uuid;

/// GitHub user info from the API.
#[derive(Debug, Deserialize)]
pub struct GithubUser {
    pub id: serde_json::Number,
    pub login: String,
}

/// Result of a GitHub device flow token poll.
#[derive(Debug)]
pub enum DeviceFlowResult {
    /// Authorization is still pending - CLI should continue polling.
    Pending,
    /// User needs to slow down - polling too frequently.
    SlowDown,
    /// Authorization succeeded - access token granted.
    Success(String),
    /// Terminal error - new device code required.
    Terminal(String),
}

/// GitHub OAuth token response.
#[derive(Debug, Deserialize)]
pub struct GithubTokenResponse {
    pub access_token: String,
}

/// GitHub OAuth error response.
#[derive(Debug, Deserialize)]
pub struct GithubErrorResponse {
    pub error: String,
    pub error_description: Option<String>,
    pub interval: Option<u64>,
}

/// Handle GitHub OAuth callback.
///
/// Direct code exchange: Authorization code in `x-code` header.
/// Exchanges the authorization code for an access token,
/// fetches user info, and creates/updates the user in the registry.
/// Returns a JWT session token to the client.
pub async fn github_callback(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, HandlerError> {
    // Get code from header
    let code = headers.get("x-code").and_then(|h| h.to_str().ok()).ok_or_else(|| {
        HandlerError::BadRequest("Missing authorization code in x-code header".to_string())
    })?;

    let config = &state.config.auth.github;
    let client_secret = config.get_client_secret();

    // Exchange code for access token
    let client = reqwest::Client::new();
    let token_resp_bytes = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&serde_json::json!({
            "client_id": config.get_client_id(),
            "client_secret": client_secret,
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
    let github_id = user_github_id_to_string(&user_resp.id);
    let user_id = format!("user-{}", Uuid::new_v4());
    let github_login = user_resp.login.clone();
    let access_token = token_resp.access_token.clone();

    // Get registry connection from pool
    let registry_conn =
        state.registry.get_conn().await.map_err(|e| {
            HandlerError::Internal(format!("Failed to get registry connection: {}", e))
        })?;

    // Use interact to run the sync registry operations
    // The upsert_user_returning_id function is atomic and eliminates the race
    // condition between checking for user existence and inserting/updating.
    let user_id = registry_conn
        .interact(move |conn| {
            registry::upsert_user_returning_id(
                conn,
                &UserUpsert {
                    id: &user_id,
                    github_id: &github_id,
                    github_login: &github_login,
                    github_token: Some(&access_token),
                },
            )
            .map_err(|e| HandlerError::Internal(format!("Failed to upsert user: {}", e)))
        })
        .await
        .map_err(|e| HandlerError::Internal(format!("Registry operation failed: {}", e)))?
        .map_err(|e| HandlerError::Internal(format!("Registry operation error: {}", e)))?;

    // Generate JWT session token
    let jwt_secret = config.get_jwt_secret();
    let session_token = generate_jwt(&user_id, &jwt_secret, config.session_expires_in)?;

    Ok((StatusCode::OK, axum::Json(serde_json::json!({ "token": session_token }))))
}

/// Generate a JWT token for the user.
fn generate_jwt(user_id: &str, jwt_secret: &str, expires_in: u64) -> Result<String, HandlerError> {
    use jsonwebtoken::{EncodingKey, Header, encode};

    if jwt_secret.is_empty() {
        return Err(HandlerError::Internal("JWT secret is not configured".to_string()));
    }

    let claims = serde_json::json!({
        "sub": user_id,
        "exp": chrono::Utc::now().timestamp() + expires_in as i64,
    });

    let token =
        encode(&Header::default(), &claims, &EncodingKey::from_secret(jwt_secret.as_bytes()))
            .map_err(|e| HandlerError::Internal(format!("Failed to generate token: {}", e)))?;

    Ok(token)
}

/// Convert GitHub ID (which can be a large number) to a string.
fn user_github_id_to_string(id: &serde_json::Number) -> String {
    id.as_u64()
        .map(|n| n.to_string())
        .or_else(|| id.as_i64().map(|n| n.to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Request a device code from GitHub.
#[derive(Debug, serde::Serialize)]
pub struct DeviceCodeRequest {
    pub client_id: String,
    pub scope: String,
}

#[derive(Debug, Deserialize, serde::Serialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn github_device_login(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, HandlerError> {
    let client_id = state.config.auth.github.get_client_id();
    if client_id.is_empty() {
        return Err(HandlerError::BadRequest(
            "GitHub OAuth is not configured on the server".to_string(),
        ));
    }

    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&serde_json::json!({
            "client_id": client_id,
            "scope": "read:org",
        }))
        .send()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to request device code: {}", e)))?
        .json::<DeviceCodeResponse>()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to parse device response: {}", e)))?;

    Ok(axum::Json(resp))
}

#[derive(Debug, Deserialize)]
pub struct DevicePollRequest {
    pub device_code: String,
}

pub async fn github_device_poll(
    State(state): State<AppState>,
    axum::Json(payload): axum::Json<DevicePollRequest>,
) -> Result<impl IntoResponse, HandlerError> {
    let config = &state.config.auth.github;
    let client_id = config.get_client_id();
    let client_secret = config.get_client_secret();

    let client = reqwest::Client::new();
    let token_resp_bytes = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "device_code": payload.device_code,
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        }))
        .send()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to poll GitHub: {}", e)))?
        .bytes()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to read response: {}", e)))?;

    // Parse response - could be success token or error
    let flow_result = parse_device_flow_response(&token_resp_bytes)?;

    let access_token = match flow_result {
        DeviceFlowResult::Pending => {
            return Ok(axum::Json(serde_json::json!({ "error": "authorization_pending" })));
        }
        DeviceFlowResult::SlowDown => {
            return Ok(axum::Json(serde_json::json!({ "error": "slow_down" })));
        }
        DeviceFlowResult::Terminal(msg) => {
            return Err(HandlerError::BadRequest(msg));
        }
        DeviceFlowResult::Success(token) => token,
    };

    // Fetch user info
    let user_resp: GithubUser = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "codetours-server")
        .send()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to fetch user: {}", e)))?
        .json()
        .await
        .map_err(|e| HandlerError::Internal(format!("Failed to parse user response: {}", e)))?;

    let github_id = user_github_id_to_string(&user_resp.id);
    let user_id = format!("user-{}", Uuid::new_v4());
    let github_login = user_resp.login.clone();

    let registry_conn =
        state.registry.get_conn().await.map_err(|e| {
            HandlerError::Internal(format!("Failed to get registry connection: {}", e))
        })?;

    let user_id = registry_conn
        .interact(move |conn| {
            registry::upsert_user_returning_id(
                conn,
                &UserUpsert {
                    id: &user_id,
                    github_id: &github_id,
                    github_login: &github_login,
                    github_token: Some(&access_token),
                },
            )
            .map_err(|e| HandlerError::Internal(format!("Failed to upsert user: {}", e)))
        })
        .await
        .map_err(|e| HandlerError::Internal(format!("Registry failed: {}", e)))??;

    let session_token =
        generate_jwt(&user_id, &config.get_jwt_secret(), config.session_expires_in)?;
    Ok(axum::Json(serde_json::json!({ "token": session_token })))
}

/// Parse the device flow response from GitHub, handling all documented error codes.
fn parse_device_flow_response(bytes: &[u8]) -> Result<DeviceFlowResult, HandlerError> {
    // First try to parse as success response
    if let Ok(token_resp) = serde_json::from_slice::<GithubTokenResponse>(bytes) {
        return Ok(DeviceFlowResult::Success(token_resp.access_token));
    }

    // Otherwise parse as error response
    let error_resp: GithubErrorResponse = serde_json::from_slice(bytes)
        .map_err(|_| HandlerError::Internal("Invalid response from GitHub".to_string()))?;

    match error_resp.error.as_str() {
        "authorization_pending" => Ok(DeviceFlowResult::Pending),
        "slow_down" => Ok(DeviceFlowResult::SlowDown),
        "expired_token" => Ok(DeviceFlowResult::Terminal(
            "Device code has expired. Please request a new device code.".to_string(),
        )),
        "access_denied" => Ok(DeviceFlowResult::Terminal("Access denied by user.".to_string())),
        "incorrect_device_code" => Ok(DeviceFlowResult::Terminal(
            "Invalid device code. Please request a new device code.".to_string(),
        )),
        "incorrect_client_credentials" => {
            Ok(DeviceFlowResult::Terminal("Invalid client credentials.".to_string()))
        }
        "device_flow_disabled" => Ok(DeviceFlowResult::Terminal(
            "Device flow is disabled for this application.".to_string(),
        )),
        "unsupported_grant_type" => {
            Ok(DeviceFlowResult::Terminal("Unsupported grant type.".to_string()))
        }
        other => Ok(DeviceFlowResult::Terminal(format!(
            "GitHub error: {}",
            error_resp.error_description.as_deref().unwrap_or(other)
        ))),
    }
}
