//! Authentication handlers for logging in and out of Codetours servers.

use crate::cli::output::OutputMode;
use crate::cli::{AuthArgs, AuthCommand, AuthLoginArgs, AuthLogoutArgs, Cli};
use codemark_core::storage::registry;
use codemark_core::error::{Error, Result};

/// Handle auth commands.
pub async fn handle_auth(cli: &Cli, mode: &OutputMode, args: &AuthArgs) -> Result<()> {
    match &args.command {
        AuthCommand::Login(args) => handle_login(cli, mode, args).await,
        AuthCommand::Logout(args) => handle_logout(cli, mode, args).await,
        AuthCommand::List => handle_list(cli, mode).await,
    }
}

/// Handle login to a server.
pub async fn handle_login(_cli: &Cli, mode: &OutputMode, args: &AuthLoginArgs) -> Result<()> {
    let server_url = normalize_server_url(&args.server);

    // If a token is provided directly, use it
    if let Some(token) = &args.token {
        let conn = registry::open_registry()?;
        registry::upsert_server(&conn, &server_url, Some(token))?;
        drop(conn);

        let message = format!("Logged in to {}", server_url);
        if matches!(mode, OutputMode::Json) {
            println!("{}", serde_json::json!({
                "status": "success",
                "message": message,
                "server": server_url,
                "method": "token",
            }));
        } else {
            println!("{}", message);
        }
        return Ok(());
    }

    // TODO: Implement OAuth flow
    // For now, instruct the user to use --token or implement OAuth
    let message = "OAuth login not yet implemented. Use --token to provide a token directly.";
    if matches!(mode, OutputMode::Json) {
        println!("{}", serde_json::json!({
            "status": "info",
            "message": message,
            "server": server_url,
        }));
    } else {
        println!("{}", message);
    }

    Ok(())
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
        println!("{}", serde_json::json!({
            "status": "success",
            "message": message,
            "server": server_url,
        }));
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
            println!("{}", serde_json::json!({
                "status": "success",
                "message": "No authenticated servers",
                "servers": [],
            }));
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
        println!("{}", serde_json::json!({
            "status": "success",
            "message": message,
            "servers": servers_json,
        }));
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
