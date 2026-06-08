//! Authentication handlers for logging in and out of Codetours servers.

use crate::cli::output::OutputMode;
use crate::cli::{AuthArgs, AuthCommand, AuthLoginArgs, AuthLogoutArgs, Cli};
use codemark_core::error::{Error, Result};
use codemark_core::storage::registry;
use copypasta::ClipboardProvider;
use std::time::{Duration, Instant};

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
/// Otherwise, initiate the device OAuth flow.
pub async fn handle_login(_cli: &Cli, mode: &OutputMode, args: &AuthLoginArgs) -> Result<()> {
    let server_url = normalize_server_url(&args.server)?;

    // If a token is provided directly, use it
    if let Some(token) = &args.token {
        let username = args.username.as_deref().unwrap_or("default");
        let conn = registry::open_registry()?;
        registry::clear_default_account(&conn, &server_url)?;
        registry::upsert_account(
            &conn,
            &registry::AccountUpsert {
                server_url: &server_url,
                username,
                email: None,
                token,
                is_default: true,
            },
        )?;
        drop(conn);

        let message = format!("Logged in to {} as {}", server_url, username);
        if matches!(mode, OutputMode::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "status": "success",
                    "message": message,
                    "server": server_url,
                    "username": username,
                    "method": "token",
                })
            );
        } else {
            println!("{}", message);
        }
        return Ok(());
    }

    handle_device_login(&server_url, args.username.as_deref(), mode).await
}

async fn handle_device_login(
    server_url: &str,
    username_override: Option<&str>,
    mode: &OutputMode,
) -> Result<()> {
    let server_url = server_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| Error::Operation(format!("Failed to build HTTP client: {}", e)))?;

    // 1. Get device code
    let device_resp: serde_json::Value = client
        .get(format!("{}/auth/github/device", server_url))
        .send()
        .await
        .map_err(|e| Error::Operation(format!("Failed to reach server: {}", e)))?
        .json()
        .await
        .map_err(|e| Error::Operation(format!("Failed to parse device response: {}", e)))?;

    let uri = device_resp["verification_uri"]
        .as_str()
        .ok_or_else(|| Error::Operation("Missing verification_uri".to_string()))?;
    let code = device_resp["user_code"]
        .as_str()
        .ok_or_else(|| Error::Operation("Missing user_code".to_string()))?;

    // Attempt to copy to clipboard
    if let Ok(mut ctx) = copypasta::ClipboardContext::new() {
        let _ = ctx.set_contents(code.to_owned());
    }

    if !matches!(mode, OutputMode::Json) {
        println!("Opening browser to authorize...");
        let _ = open::that(uri);

        println!("Verification code: {} (copied to clipboard)", code);
        println!("Please authorize in your browser.");
    } else {
        let _ = open::that(uri);
        println!(
            "{}",
            serde_json::json!({
                "status": "pending",
                "message": "Device code received. Please authorize in your browser.",
                "verification_uri": uri,
                "user_code": code,
            })
        );
    }

    // 2. Poll for token
    let device_code = device_resp["device_code"]
        .as_str()
        .ok_or_else(|| Error::Operation("Missing device_code".to_string()))?;
    let mut interval = device_resp["interval"].as_u64().unwrap_or(5);
    let expires_in = device_resp["expires_in"].as_u64().unwrap_or(300); // Default 5 minutes

    let start_time = Instant::now();
    let timeout = Duration::from_secs(expires_in);

    loop {
        // Check if we've exceeded the timeout
        if start_time.elapsed() > timeout {
            return Err(Error::Operation(
                "Authentication timed out. Please try again.".to_string(),
            ));
        }

        tokio::time::sleep(Duration::from_secs(interval)).await;

        let poll_resp: serde_json::Value = client
            .post(format!("{}/auth/github/device/poll", server_url))
            .json(&serde_json::json!({ "device_code": device_code }))
            .send()
            .await
            .map_err(|e| Error::Operation(format!("Poll failed: {}", e)))?
            .json()
            .await
            .map_err(|e| Error::Operation(format!("Failed to parse poll response: {}", e)))?;

        if let Some(token) = poll_resp["token"].as_str() {
            // Extract identity from poll response if available
            let username = username_override
                .map(|s| s.to_string())
                .or_else(|| poll_resp["username"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "default".to_string());
            let email = poll_resp["email"].as_str();

            let conn = registry::open_registry()?;
            registry::clear_default_account(&conn, server_url)?;
            registry::upsert_account(
                &conn,
                &registry::AccountUpsert {
                    server_url,
                    username: &username,
                    email,
                    token,
                    is_default: true,
                },
            )?;
            if matches!(mode, OutputMode::Json) {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "success",
                        "message": format!("Logged in to {} as {}", server_url, username),
                        "server": server_url,
                        "username": username,
                        "method": "device_oauth"
                    })
                );
            } else {
                println!("Successfully authenticated as {}!", username);
            }
            return Ok(());
        }

        if let Some(error) = poll_resp["error"].as_str() {
            match error {
                "authorization_pending" => continue,
                "slow_down" => {
                    // Increase interval by 5 seconds
                    interval = interval.saturating_add(5);
                    continue;
                }
                "expired_token" => {
                    return Err(Error::Operation("Device code expired".to_string()));
                }
                _ => {
                    return Err(Error::Operation(format!("Auth error: {}", error)));
                }
            }
        }
    }
}

/// Handle logout from a server.
pub async fn handle_logout(_cli: &Cli, mode: &OutputMode, args: &AuthLogoutArgs) -> Result<()> {
    let server_url = normalize_server_url(&args.server)?;

    let conn = registry::open_registry()?;

    // Check if any accounts exist for this server
    let accounts = registry::list_accounts(&conn, Some(&server_url))?;
    if accounts.is_empty() {
        return Err(Error::Operation(format!("Not logged in to {}", server_url)));
    }

    // If a specific user was requested, verify that account exists
    if let Some(ref user) = args.user {
        if !accounts.iter().any(|a| a.username == *user) {
            return Err(Error::Operation(format!("Not logged in to {} as {}", server_url, user)));
        }
    }

    registry::delete_account(&conn, &server_url, args.user.as_deref())?;
    drop(conn);

    let message = if let Some(ref user) = args.user {
        format!("Logged out {} from {}", user, server_url)
    } else {
        format!("Logged out from {}", server_url)
    };
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

/// Handle listing authenticated accounts.
pub async fn handle_list(_cli: &Cli, mode: &OutputMode) -> Result<()> {
    let conn = registry::open_registry()?;
    let accounts = registry::list_accounts(&conn, None)?;
    drop(conn);

    if accounts.is_empty() {
        if matches!(mode, OutputMode::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "status": "success",
                    "message": "No authenticated accounts",
                    "accounts": [],
                })
            );
        } else {
            println!("No authenticated accounts");
        }
        return Ok(());
    }

    let accounts_json: Vec<serde_json::Value> = accounts
        .iter()
        .map(|a| {
            serde_json::json!({
                "server_url": a.server_url,
                "username": a.username,
                "email": a.email,
                "is_default": a.is_default,
                "last_used": a.last_used,
            })
        })
        .collect();

    let message = format!("{} authenticated account(s)", accounts.len());
    if matches!(mode, OutputMode::Json) {
        println!(
            "{}",
            serde_json::json!({
                "status": "success",
                "message": message,
                "accounts": accounts_json,
            })
        );
    } else {
        println!("{}", message);
        for account in &accounts {
            println!("  - {} @ {}", account.username, account.server_url);
        }
    }

    Ok(())
}

/// Normalize a server URL by removing trailing slash and ensuring https:// if no scheme.
///
/// Returns an error if the URL uses an insecure (non-TLS) scheme for non-localhost addresses.
fn normalize_server_url(url: &str) -> Result<String> {
    let mut url = url.trim().to_string();

    // Remove trailing slash
    if url.ends_with('/') {
        url.pop();
    }

    // Add https:// if no scheme
    if !url.starts_with("http://") && !url.starts_with("https://") {
        url = format!("https://{}", url);
    }

    // Reject non-TLS URLs for non-localhost addresses
    if url.starts_with("http://")
        && !url.starts_with("http://localhost")
        && !url.starts_with("http://127.0.0.1")
        && !url.starts_with("http://[::1]")
        && !url.starts_with("http://[::1]")
    // IPv6 localhost
    {
        return Err(Error::Input("insecure server URL: use https://".to_string()));
    }

    Ok(url)
}
