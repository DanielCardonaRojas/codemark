//! Authentication handlers for logging in and out of Codetours servers.

use crate::cli::output::OutputMode;
use crate::cli::{AuthArgs, AuthCommand, AuthLoginArgs, AuthLogoutArgs, Cli};
use copypasta::ClipboardProvider;
use codemark_core::error::{Error, Result};
use codemark_core::storage::registry;
use std::time::Duration;

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

    handle_device_login(&server_url, mode).await
}

async fn handle_device_login(server_url: &str, _mode: &OutputMode) -> Result<()> {
    let server_url = server_url.trim_end_matches('/');
    let client = reqwest::Client::new();

    // 1. Get device code
    let device_resp: serde_json::Value = client
        .get(format!("{}/auth/github/device", server_url))
        .send()
        .await
        .map_err(|e| Error::Operation(format!("Failed to reach server: {}", e)))?
        .json()
        .await
        .map_err(|e| Error::Operation(format!("Failed to parse device response: {}", e)))?;

    let uri = device_resp["verification_uri"].as_str().ok_or_else(|| Error::Operation("Missing verification_uri".to_string()))?;
    let code = device_resp["user_code"].as_str().ok_or_else(|| Error::Operation("Missing user_code".to_string()))?;

    // Attempt to copy to clipboard
    if let Ok(mut ctx) = copypasta::ClipboardContext::new() {
        let _ = ctx.set_contents(code.to_owned());
    }

    println!("Opening browser to authorize...");
    let _ = open::that(uri);

    println!("Verification code: {} (copied to clipboard)", code);
    println!("Please authorize in your browser.");

    // 2. Poll for token
    let device_code = device_resp["device_code"].as_str().ok_or_else(|| Error::Operation("Missing device_code".to_string()))?;
    let interval = device_resp["interval"].as_u64().unwrap_or(5);

    loop {
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

        if let Some(token) = poll_resp["access_token"].as_str() {
            let conn = registry::open_registry()?;
            registry::upsert_server(&conn, server_url, Some(token))?;
            println!("Successfully authenticated!");
            return Ok(());
        }

        if let Some(error) = poll_resp["error"].as_str() {
            if error == "authorization_pending" {
                continue;
            }
            return Err(Error::Operation(format!("Auth error: {}", error)));
        }
    }
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
