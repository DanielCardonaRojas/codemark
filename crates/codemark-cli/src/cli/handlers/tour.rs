use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};
use comfy_table::Table;
use serde::Deserialize;

// Re-export auth resolution helpers
use crate::cli::handlers::auth_resolve::{
    build_auth_headers, detect_current_repo, resolve_server_and_token,
};

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
