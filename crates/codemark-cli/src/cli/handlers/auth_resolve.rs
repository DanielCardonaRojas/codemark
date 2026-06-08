//! Shared authentication and server resolution helpers.
//!
//! This module provides common functionality for resolving servers, tokens,
//! and building authentication headers across different CLI commands.

use crate::cli::Cli;
use codemark_core::error::{Error, Result};
use codemark_core::git::remote;
use codemark_core::storage::registry;
use reqwest::header::{HeaderMap, HeaderValue};

/// Detect the current repository's GitHub URL.
pub fn detect_current_repo() -> Result<Option<String>> {
    // Get the current directory
    let current_dir = std::env::current_dir()
        .map_err(|e| Error::Operation(format!("Failed to get current directory: {}", e)))?;

    // Try to parse the git remote
    match remote::parse_current_repo(&current_dir) {
        Ok((owner, repo)) => {
            let url = remote::build_github_url(&owner, &repo);
            Ok(Some(url))
        }
        Err(_) => Ok(None),
    }
}

/// Resolve server URL and token from registry.
///
/// Priority:
/// 1. Explicit server URL from args (http:// or https://)
/// 2. Named server from config (if server_arg is a name)
/// 3. Server URL from local registry for current repo
/// 4. Default server from config
/// 5. Fallback to localhost
pub fn resolve_server_and_token(
    cli: &Cli,
    server_arg: Option<&str>,
    repo_url: Option<&str>,
) -> Result<(String, Option<String>)> {
    // 1. Honor explicit --server arg (URL or named server) before registry/repo lookup
    if let Some(server) = server_arg {
        if server.starts_with("http") {
            // Direct URL - check for token in registry
            let registry_token = get_token_for_server(server)?;
            return Ok((server.to_string(), registry_token));
        }
        // Look up named server in config
        let config = super::load_config(cli);
        if let Some(s) = config.codetours.servers.iter().find(|s| s.name == server) {
            return Ok((s.url.clone(), s.token.clone()));
        }
        return Err(Error::Input(format!("server '{}' not found in config", server)));
    }

    // 2. Registry lookup keyed by current repo
    if let Ok(conn) = registry::open_registry() {
        if let Some(repo) = repo_url {
            // Look for a server associated with this repo
            if let Ok(Some(known_repo)) = registry::find_repo_by_origin(&conn, repo)
                && let Some(ref server_url) = known_repo.server_url
            {
                let token = get_token_for_server(server_url)?;
                return Ok((server_url.clone(), token));
            }
        }

        // 3. Check for a global default account in registry
        if let Ok(Some(account)) = registry::get_global_default_account(&conn) {
            return Ok((account.server_url.clone(), Some(account.token.clone())));
        }
    }

    // 4. Fallback to config or localhost
    let config = super::load_config(cli);
    let server_name = config.codetours.default_server.as_deref().unwrap_or("default");

    if server_name.starts_with("http") {
        return Ok((server_name.to_string(), None));
    }

    let s = config
        .codetours
        .servers
        .iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| Error::Input(format!("server '{}' not found in config", server_name)))?;

    Ok((s.url.clone(), s.token.clone()))
}

/// Build authorization headers for a server request.
///
/// Uses Bearer token for JWT-based auth, fallback to X-Tour-Token for legacy.
pub fn build_auth_headers(token: Option<&String>) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();

    if let Some(t) = token {
        if t.starts_with("eyJ") {
            // JWT token
            headers.insert(
                "Authorization",
                HeaderValue::from_str(&format!("Bearer {}", t))
                    .map_err(|_| Error::Operation("Invalid token".to_string()))?,
            );
        } else {
            // Legacy token
            headers.insert(
                "X-Tour-Token",
                HeaderValue::from_str(t)
                    .map_err(|_| Error::Operation("Invalid token".to_string()))?,
            );
        }
    }

    Ok(headers)
}

/// Get auth token for a server from the registry.
pub fn get_token_for_server(server_url: &str) -> Result<Option<String>> {
    let conn = registry::open_registry()?;
    registry::resolve_token(&conn, server_url, None)
        .map_err(|e| Error::Database(format!("Failed to query account: {}", e)))
}
