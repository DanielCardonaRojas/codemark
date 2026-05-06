use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};
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
    let config = super::load_config(cli);

    // 1. Resolve server and token
    let server_name =
        args.server.as_deref().or(config.codetours.default_server.as_deref()).unwrap_or("default");

    let (server_url, token) = if server_name.starts_with("http") {
        (server_name.to_string(), None)
    } else {
        let s =
            config.codetours.servers.iter().find(|s| s.name == server_name).ok_or_else(|| {
                Error::Input(format!("server '{}' not found in config", server_name))
            })?;
        (s.url.clone(), s.token.clone())
    };

    // 2. Query server
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert(reqwest::header::ACCEPT, HeaderValue::from_static("application/json"));
    if let Some(t) = &token {
        headers.insert(
            "X-Tour-Token",
            HeaderValue::from_str(t).map_err(|_| Error::Operation("Invalid token".to_string()))?,
        );
    }

    let mut query = vec![("limit", args.limit.to_string()), ("offset", args.offset.to_string())];
    if let Some(repo) = &args.repo {
        query.push(("repo_url", repo.clone()));
    }

    let response = client
        .get(format!("{}/tours", server_url))
        .headers(headers)
        .query(&query)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to list tours: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(Error::Operation(format!("server returned {status}")));
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
            &tour.tour_id[..8],
            &tour.title,
            tour.repo_url.as_deref().unwrap_or("-"),
            &tour.updated_at,
        ]);
    }

    println!("{table}");
    println!("\nTotal: {} (Showing {}-{})", res.total, res.offset, res.offset + res.tours.len());

    Ok(())
}
