use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};
use codemark_core::git::remote;
use codemark_core::storage::registry;
use comfy_table::Table;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TourSummary {
    tour_id: String,
    title: String,
    repo_url: Option<String>,
    updated_at: String,
    url: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ListToursResponse {
    tours: Vec<TourSummary>,
    total: usize,
    limit: usize,
    offset: usize,
}

pub async fn handle_tour_list(cli: &Cli, _mode: &OutputMode, args: &TourListArgs) -> Result<()> {
    // 1. Determine the repository URL to filter by
    let repo_url = if let Some(ref repo) = args.repo {
        // User provided explicit repo URL
        Some(repo.clone())
    } else {
        // Try to detect from current git repository
        detect_current_repo()?
    };

    // 2. Resolve server and token from registry
    let (server_url, token) =
        resolve_server_and_token(cli, args.server.as_deref(), repo_url.as_deref())?;

    // 3. Query server
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();

    // Use Bearer token for JWT-based auth, fallback to X-Tour-Token for legacy
    if let Some(ref t) = token {
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

    let mut query = vec![("limit", args.limit.to_string()), ("offset", args.offset.to_string())];
    if let Some(ref repo) = repo_url {
        query.push(("repo_url", repo.clone()));
    }

    let url = format!("{}/tours", server_url);
    eprintln!("DEBUG: Calling URL: {}", url);

    let response = client
        .get(&url)
        .headers(headers)
        .query(&query)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to list tours: {e}")))?;

    eprintln!("DEBUG: Response status: {}", response.status());

    if !response.status().is_success() {
        let status = response.status();
        let error_body =
            response.text().await.unwrap_or_else(|_| "unable to read error".to_string());
        return Err(Error::Operation(format!("server returned {}: {}", status, error_body)));
    }

    let res: ListToursResponse = response
        .json()
        .await
        .map_err(|e| Error::Operation(format!("failed to parse server response: {e}")))?;

    if res.tours.is_empty() {
        println!("No tours found on server.");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec!["ID", "Title", "Repo", "Updated"]);

    for tour in &res.tours {
        table.add_row(vec![
            &tour.tour_id[..8.min(tour.tour_id.len())],
            &tour.title,
            tour.repo_url.as_deref().unwrap_or("-"),
            &tour.updated_at,
        ]);
    }

    println!("{table}");
    println!("\nTotal: {} (Showing {}-{})", res.total, res.offset, res.offset + res.tours.len());

    Ok(())
}

/// Detect the current repository's GitHub URL.
fn detect_current_repo() -> Result<Option<String>> {
    // Get the current directory
    let current_dir = std::env::current_dir()
        .map_err(|e| Error::Operation(format!("Failed to get current directory: {}", e)))?;

    // Try to parse the git remote
    match remote::parse_current_repo(&current_dir) {
        Ok((owner, repo)) => {
            let url = remote::build_github_url(&owner, &repo);
            // tracing::debug!(%url, "Detected current repository");
            Ok(Some(url))
        }
        Err(_) => {
            // tracing::debug!(error = %e, "Could not detect current repository");
            Ok(None)
        }
    }
}

/// Resolve server URL and token from registry.
///
/// Priority:
/// 1. Explicit server URL from args
/// 2. Server URL from local registry for current repo
/// 3. Default server from config
/// 4. Fallback to localhost
fn resolve_server_and_token(
    cli: &Cli,
    server_arg: Option<&str>,
    repo_url: Option<&str>,
) -> Result<(String, Option<String>)> {
    // If explicit server URL is provided, use it
    if let Some(server) = server_arg
        && server.starts_with("http")
    {
        // Direct URL - check for token in registry
        let token = get_token_for_server(server)?;
        return Ok((server.to_string(), token));
    }

    // Try to get server from registry based on current repo
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

        // If no repo-specific server, check for a default server in registry
        if let Ok(servers) = registry::list_servers(&conn)
            && let Some(server) = servers.first()
        {
            let token = server.token.clone();
            return Ok((server.url.clone(), token));
        }
    }

    // Fallback to config or localhost
    let config = super::load_config(cli);
    let server_name =
        server_arg.or(config.codetours.default_server.as_deref()).unwrap_or("default");

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

/// Get auth token for a server from the registry.
fn get_token_for_server(server_url: &str) -> Result<Option<String>> {
    let conn = registry::open_registry()?;
    let server = registry::get_server(&conn, server_url)
        .map_err(|e| Error::Database(format!("Failed to query server: {}", e)))?;

    Ok(server.and_then(|s| s.token))
}
