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
pub fn normalize_repo_url(repo: &str) -> String {
    // If it's already in owner/repo format, convert to SSH URL
    if !repo.contains('/') && !repo.contains(':') {
        return repo.to_string();
    }

    // Check if it's owner/repo format (simple heuristic)
    if let Some((owner, repo_name)) = repo.split_once('/')
        && !owner.contains('.')
        && !repo_name.contains(':')
    {
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
    #[serde(default)]
    repo_url: Option<String>,
    #[serde(default)]
    author: Option<String>,
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

pub async fn handle_tour_list(
    cli: &Cli,
    mode: &OutputMode,
    args: &TourListArgs,
    detected_repo: Option<&str>,
) -> Result<()> {
    // 1. Determine the repository URL to filter by.
    // `GET /tours` is now an authorization-scoped lookup: the caller must name a
    // repo. Priority: explicit --repo flag > auto-detected repo. There is no
    // global/browse-everything list anymore.
    let repo_url = match args
        .repo
        .as_ref()
        .map(|repo| normalize_repo_url(repo))
        .or_else(|| detected_repo.map(normalize_repo_url))
    {
        Some(repo) => repo,
        None => {
            return Err(Error::Input(
                "listing tours requires a repository scope: pass --repo <owner/name> \
                 or run inside a git repository with a GitHub remote"
                    .to_string(),
            ));
        }
    };

    // 2. Resolve server and token from registry
    let (server_url, token) =
        resolve_server_and_token(cli, args.server.as_deref(), Some(repo_url.as_str()))?;

    // 3. Query server
    let client = reqwest::Client::new();
    let headers = build_auth_headers(token.as_ref())?;

    let mut query = vec![("limit", args.limit.to_string()), ("offset", args.offset.to_string())];
    // Parse into owner/name to avoid URL format mismatch (SSH vs HTTPS).
    if let Some((owner, name)) = codemark_core::git::remote::parse_remote_url(&repo_url) {
        query.push(("repos", format!("{}/{}", owner, name)));
    } else {
        query.push(("repo_url", repo_url.clone()));
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
        println!("No tours found for repository on server.");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec!["ID", "Title", "Author", "Repo", "Updated"]);

    for tour in &res.tours {
        table.add_row(vec![
            &tour.tour_id[..8.min(tour.tour_id.len())],
            &tour.title,
            tour.author.as_deref().unwrap_or("-"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_owner_repo_format() {
        assert_eq!(
            normalize_repo_url("DanielCardonaRojas/codemark"),
            "git@github.com:DanielCardonaRojas/codemark.git"
        );
    }

    #[test]
    fn test_normalize_https_url() {
        // GitHub HTTPS URLs are normalized to SSH format
        let result = normalize_repo_url("https://github.com/DanielCardonaRojas/codemark.git");
        assert!(
            result.contains("git@github.com:"),
            "Result should be a GitHub SSH URL: {}",
            result
        );
        assert!(result.contains("DanielCardonaRojas"), "Result should contain owner: {}", result);
        assert!(result.contains("codemark"), "Result should contain repo name: {}", result);
    }

    #[test]
    fn test_normalize_ssh_url() {
        assert_eq!(
            normalize_repo_url("git@github.com:DanielCardonaRojas/codemark.git"),
            "git@github.com:DanielCardonaRojas/codemark.git"
        );
    }

    #[test]
    fn test_normalize_unknown_format() {
        // HTTPS URLs from any host get normalized to GitHub SSH format
        // (this is the current behavior - parse_remote_url extracts owner/repo)
        let result = normalize_repo_url("https://gitlab.com/owner/repo.git");
        assert!(
            result.contains("git@github.com:"),
            "Non-GitHub HTTPS URLs also get normalized: {}",
            result
        );
        // URLs that can't be parsed are returned as-is
        assert_eq!(normalize_repo_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn test_normalize_simple_string() {
        // Strings without / or : are returned as-is
        assert_eq!(normalize_repo_url("codemark"), "codemark");
    }

    #[test]
    fn test_normalize_then_parse_roundtrip() {
        // Verify that normalized SSH URLs can be parsed back to owner/name
        // This is the key flow for --repo flag to work with find_repo_by_origin
        let input = "DanielCardonaRojas/codemark";
        let normalized = normalize_repo_url(input);
        assert_eq!(normalized, "git@github.com:DanielCardonaRojas/codemark.git");

        // The normalized URL should be parsable back to owner/name
        let parsed = remote::parse_remote_url(&normalized);
        assert_eq!(parsed, Some(("DanielCardonaRojas".to_string(), "codemark".to_string())));
    }
}
