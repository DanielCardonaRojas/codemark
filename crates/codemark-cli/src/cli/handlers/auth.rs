//! Authentication handlers for logging in and out of Codetours servers.

use crate::cli::output::OutputMode;
use crate::cli::{AuthArgs, AuthCommand, AuthLoginArgs, AuthLogoutArgs, Cli};
use axum::{
    Router,
    extract::{Query as AxumQuery, State as AxumState},
    response::IntoResponse,
    routing::get,
};
use codemark_core::error::{Error, Result};
use codemark_core::storage::registry;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Shared state between the main login flow and the callback handler.
struct CallbackState {
    received_code: Arc<tokio::sync::RwLock<Option<String>>>,
    cancel_token: CancellationToken,
}

/// Handle auth commands.
pub async fn handle_auth(cli: &Cli, mode: &OutputMode, args: &AuthArgs) -> Result<()> {
    match &args.command {
        AuthCommand::Login(args) => handle_login(cli, mode, args).await,
        AuthCommand::Logout(args) => handle_logout(cli, mode, args).await,
        AuthCommand::List => handle_list(cli, mode).await,
    }
}

/// Handle login to a server.
///
/// If a token is provided directly, store it.
/// Otherwise, initiate the OAuth flow.
pub async fn handle_login(_cli: &Cli, mode: &OutputMode, args: &AuthLoginArgs) -> Result<()> {
    let server_url = normalize_server_url(&args.server);

    // If a token is provided directly, use it
    if let Some(token) = &args.token {
        let conn = registry::open_registry()?;
        registry::upsert_server(&conn, &server_url, Some(token))?;
        drop(conn);

        let message = format!("Logged in to {}", server_url);
        if matches!(mode, OutputMode::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "status": "success",
                    "message": message,
                    "server": server_url,
                    "method": "token",
                })
            );
        } else {
            println!("{}", message);
        }
        return Ok(());
    }

    // Initiate OAuth flow
    if args.browser {
        perform_oauth_flow(&server_url, mode).await
    } else {
        // No browser mode - print instructions
        let auth_url = format!("{}/auth/github", server_url.trim_end_matches('/'));
        let message = format!(
            "Visit the following URL to authorize:\n  {}\n\nThen use --token with the received token.",
            auth_url
        );
        if matches!(mode, OutputMode::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "status": "info",
                    "message": message,
                    "auth_url": auth_url,
                    "server": server_url,
                })
            );
        } else {
            println!("{}", message);
        }
        Ok(())
    }
}

/// Perform the full OAuth flow.
async fn perform_oauth_flow(server_url: &str, mode: &OutputMode) -> Result<()> {
    let server_url = server_url.trim_end_matches('/');

    // 1. Generate a state parameter for CSRF protection
    let state_param = Uuid::new_v4().to_string();

    // 2. Build the authorization URL
    let auth_url = format!("{}/auth/github?state={}", server_url, state_param);

    // 3. Prepare callback state
    let callback_state = Arc::new(CallbackState {
        received_code: Arc::new(tokio::sync::RwLock::new(None)),
        cancel_token: CancellationToken::new(),
    });

    // 4. Start local callback server on fixed port
    let port = 34500;
    let callback_url = format!("http://localhost:{}/callback", port);
    let auth_url_with_callback =
        format!("{}?redirect_uri={}", auth_url, urlencoding::encode(&callback_url));

    println!("Opening browser for GitHub authorization...");
    println!("If the browser doesn't open, visit:");
    println!("  {}", auth_url_with_callback);

    // Spawn the local server
    let server_state = callback_state.clone();
    let state_param_clone = state_param.clone();
    let cancel_token_clone = callback_state.cancel_token.clone();
    let server_handle = tokio::spawn(async move {
        run_callback_server(port, server_state, state_param_clone, cancel_token_clone).await
    });

    // 5. Open the browser
    if let Err(e) = webbrowser::open(&auth_url_with_callback) {
        eprintln!("Failed to open browser: {}", e);
        eprintln!("Please visit the URL manually.");
    }

    // 6. Wait for the callback or timeout
    let code = tokio::select! {
        result = wait_for_code(callback_state.clone()) => result?,
        _ = tokio::time::sleep(Duration::from_secs(300)) => {
            callback_state.cancel_token.cancel();
            return Err(Error::Operation(
                "OAuth flow timed out after 5 minutes".to_string()
            ));
        }
    };

    // 7. Exchange the code for a token with the server
    let token = exchange_code_for_token(server_url, &code).await?;

    // 8. Store the token
    let conn = registry::open_registry()?;
    registry::upsert_server(&conn, server_url, Some(&token))?;
    drop(conn);

    // 9. Wait for server to shut down gracefully
    callback_state.cancel_token.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_handle).await;

    let message = format!("Successfully logged in to {}", server_url);
    if matches!(mode, OutputMode::Json) {
        println!(
            "{}",
            serde_json::json!({
                "status": "success",
                "message": message,
                "server": server_url,
                "method": "oauth",
            })
        );
    } else {
        println!("{}", message);
    }

    Ok(())
}

/// Run a local HTTP server to receive the OAuth callback.
async fn run_callback_server(
    port: u16,
    state: Arc<CallbackState>,
    expected_state: String,
    cancel_token: CancellationToken,
) -> Result<()> {
    let app = Router::new()
        .route("/callback", get(handle_oauth_callback))
        .with_state((state, expected_state));

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::Operation(format!("Failed to bind to port {}: {}", port, e)))?;

    // Serve until cancelled
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(cancel_token))
        .await
        .map_err(|e| Error::Operation(format!("Server error: {}", e)))?;

    Ok(())
}

/// Handle the OAuth callback from the local server.
async fn handle_oauth_callback(
    AxumState((state, expected_state)): AxumState<(Arc<CallbackState>, String)>,
    AxumQuery(params): AxumQuery<CallbackParams>,
) -> impl IntoResponse {
    // Verify state parameter for CSRF protection
    if params.state != expected_state {
        let html = error_html("State parameter mismatch. Please try again.");
        return axum::response::Html::from(html);
    }

    // Check for error from GitHub
    if let Some(error) = &params.error {
        let error_desc = params.error_description.as_deref().unwrap_or("Unknown error");
        let html = error_html(&format!("GitHub authorization failed: {} - {}", error, error_desc));
        return axum::response::Html::from(html);
    }

    // Store the received code
    if let Some(code) = params.code {
        let mut received = state.received_code.write().await;
        *received = Some(code);
    } else {
        let html = error_html("No authorization code received.");
        return axum::response::Html::from(html);
    }

    // Return success HTML
    axum::response::Html::from(success_html())
}

/// Wait for the authorization code to be received.
async fn wait_for_code(state: Arc<CallbackState>) -> Result<String> {
    loop {
        {
            let received = state.received_code.read().await;
            if let Some(ref code) = *received {
                return Ok(code.clone());
            }
        }
        if state.cancel_token.is_cancelled() {
            return Err(Error::Operation("OAuth flow was cancelled".to_string()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Exchange the authorization code for a JWT token with the server.
async fn exchange_code_for_token(server_url: &str, code: &str) -> Result<String> {
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/auth/github/callback", server_url.trim_end_matches('/')))
        .header("x-code", code)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("Failed to contact server: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_else(|_| "unable to read response".to_string());
        return Err(Error::Operation(format!("Server returned {}: {}", status, body)));
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        token: String,
    }

    let token_response: TokenResponse = response
        .json()
        .await
        .map_err(|e| Error::Operation(format!("Failed to parse token response: {}", e)))?;

    Ok(token_response.token)
}

/// Signal to shut down the callback server.
async fn shutdown_signal(cancel_token: CancellationToken) {
    #[cfg(unix)]
    {
        let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler");
        let _ = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("failed to install signal handler");
    }

    #[cfg(windows)]
    {
        let _ = tokio::signal::windows::ctrl_c().expect("failed to install CTRL-C handler");
        let _ = tokio::signal::windows::ctrl_shutdown()
            .expect("failed to install CTRL-SHUTDOWN handler");
    }

    cancel_token.cancelled().await;
}

/// Handle logout from a server.
pub async fn handle_logout(_cli: &Cli, mode: &OutputMode, args: &AuthLogoutArgs) -> Result<()> {
    let server_url = normalize_server_url(&args.server);

    let conn = registry::open_registry()?;

    // Check if server exists
    let server = registry::get_server(&conn, &server_url)?;
    if server.is_none() {
        return Err(Error::Operation(format!("Not logged in to {}", server_url)));
    }

    registry::delete_server(&conn, &server_url)?;
    drop(conn);

    let message = format!("Logged out from {}", server_url);
    if matches!(mode, OutputMode::Json) {
        println!(
            "{}",
            serde_json::json!({
                "status": "success",
                "message": message,
                "server": server_url,
            })
        );
    } else {
        println!("{}", message);
    }

    Ok(())
}

/// Handle listing authenticated servers.
pub async fn handle_list(_cli: &Cli, mode: &OutputMode) -> Result<()> {
    let conn = registry::open_registry()?;
    let servers = registry::list_servers(&conn)?;
    drop(conn);

    if servers.is_empty() {
        if matches!(mode, OutputMode::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "status": "success",
                    "message": "No authenticated servers",
                    "servers": [],
                })
            );
        } else {
            println!("No authenticated servers");
        }
        return Ok(());
    }

    let servers_json: Vec<serde_json::Value> = servers
        .iter()
        .map(|s| {
            serde_json::json!({
                "url": s.url,
                "has_token": s.token.is_some(),
                "last_login": s.last_login,
            })
        })
        .collect();

    let message = format!("{} authenticated server(s)", servers.len());
    if matches!(mode, OutputMode::Json) {
        println!(
            "{}",
            serde_json::json!({
                "status": "success",
                "message": message,
                "servers": servers_json,
            })
        );
    } else {
        println!("{}", message);
        for server in &servers {
            println!("  - {}", server.url);
        }
    }

    Ok(())
}

/// Normalize a server URL by removing trailing slash and ensuring https:// if no scheme.
fn normalize_server_url(url: &str) -> String {
    let mut url = url.trim().to_string();

    // Remove trailing slash
    if url.ends_with('/') {
        url.pop();
    }

    // Add https:// if no scheme
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{}", url);
    }

    url
}

/// Query parameters for the OAuth callback.
#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: String,
    error: Option<String>,
    error_description: Option<String>,
}

/// HTML page for successful authorization.
fn success_html() -> String {
    r#"
<!DOCTYPE html>
<html>
<head>
    <title>Authentication Successful</title>
    <style>
        body {
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #f5f5f5;
        }
        .container {
            text-align: center;
            background: white;
            padding: 2rem 3rem;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }
        .checkmark {
            font-size: 4rem;
            color: #28a745;
        }
        h1 {
            color: #333;
            margin-bottom: 0.5rem;
        }
        p {
            color: #666;
            margin: 0;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="checkmark">✓</div>
        <h1>Authentication Successful</h1>
        <p>You can close this window and return to the terminal.</p>
    </div>
</body>
</html>
    "#
    .to_string()
}

/// HTML page for errors.
fn error_html(message: &str) -> String {
    format!(
        r#"
<!DOCTYPE html>
<html>
<head>
    <title>Authentication Error</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            display: flex;
            justify-content: center;
            align-items: center;
            height: 100vh;
            margin: 0;
            background: #f5f5f5;
        }}
        .container {{
            text-align: center;
            background: white;
            padding: 2rem 3rem;
            border-radius: 8px;
            box-shadow: 0 2px 8px rgba(0,0,0,0.1);
        }}
        .error {{
            font-size: 4rem;
            color: #dc3545;
        }}
        h1 {{
            color: #333;
            margin-bottom: 0.5rem;
        }}
        p {{
            color: #666;
            margin: 0;
        }}
    </style>
</head>
<body>
    <div class="container">
        <div class="error">✕</div>
        <h1>Authentication Failed</h1>
        <p>{}</p>
    </div>
</body>
</html>
    "#,
        message
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_server_url() {
        assert_eq!(normalize_server_url("https://example.com"), "https://example.com");
        assert_eq!(normalize_server_url("https://example.com/"), "https://example.com");
        assert_eq!(normalize_server_url("example.com"), "https://example.com");
        assert_eq!(normalize_server_url("http://example.com"), "http://example.com");
    }
}
