use crate::cli::output::{OutputMode, write_json_success};
use crate::cli::*;
use codemark_core::error::{Error, Result};
use codemark_core::git::remote;
use comfy_table::Table;
use serde::{Deserialize, Serialize};
use serde_json::json;

// Re-export auth resolution helpers
use crate::cli::handlers::auth_resolve::{build_auth_headers, resolve_server_and_token};

/// Normalize a repository reference to a standard SSH URL format.
///
/// Accepts:
/// - owner/repo format
/// - HTTPS URLs
/// - SSH URLs
///
/// Returns a normalized SSH URL (git@github.com:owner/repo.git) for GitHub,
/// or the original URL for other hosts.
fn normalize_repo_url(repo: &str) -> String {
    // If it's already in owner/repo format, convert to SSH URL
    if !repo.contains('/') && !repo.contains(':') {
        return repo.to_string();
    }

    // Check if it's owner/repo format (simple heuristic)
    if let Some((owner, repo_name)) = repo.split_once('/')
        && !owner.contains('.') && !repo_name.contains(':') {
            // Likely owner/repo format
            return format!("git@github.com:{}/{}.git", owner, repo_name);
        }

    // For full URLs, try to parse and normalize
    if let Some((owner, repo_name)) = remote::parse_remote_url(repo) {
        return format!("git@github.com:{}/{}.git", owner, repo_name);
    }

    // Return as-is if we can't normalize
    repo.to_string()
}

#[derive(Debug, Deserialize, Serialize)]
struct TourSummary {
    tour_id: String,
    title: String,
    repo_url: Option<String>,
    updated_at: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct ListToursResponse {
    tours: Vec<TourSummary>,
    total: usize,
    limit: usize,
    offset: usize,
}

pub async fn handle_tour_list(cli: &Cli, mode: &OutputMode, args: &TourListArgs) -> Result<()> {
    // 1. Determine the repository URL to filter by (only if explicitly provided)
    let repo_url = args.repo.as_ref().map(|repo| normalize_repo_url(repo));

    // 2. Resolve server and token from registry
    let (server_url, token) =
        resolve_server_and_token(cli, args.server.as_deref(), repo_url.as_deref())?;

    // 3. Query server
    let client = reqwest::Client::new();
    let headers = build_auth_headers(token.as_ref())?;

    let mut query = vec![("limit", args.limit.to_string()), ("offset", args.offset.to_string())];
    if let Some(ref repo) = repo_url {
        query.push(("repo_url", repo.clone()));
    }

    let url = format!("{}/tours", server_url);

    let response = client
        .get(&url)
        .headers(headers)
        .query(&query)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to list tours: {e}")))?;

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

    // JSON output
    if matches!(mode, OutputMode::Json) {
        write_json_success(&json!({
            "tours": res.tours,
            "total": res.total,
            "limit": res.limit,
            "offset": res.offset,
        }))?;
        return Ok(());
    }

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
    println!(
        "\nTotal: {} (Showing {}-{})",
        res.total,
        res.offset + 1,
        res.offset + res.tours.len()
    );

    Ok(())
}
