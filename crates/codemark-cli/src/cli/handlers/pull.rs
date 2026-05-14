use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};

// Re-export auth resolution helpers
use crate::cli::handlers::auth_resolve::get_token_for_server;
use crate::cli::handlers::sync::{SyncDirection, SyncOptions, build_sync_http_client, sync};

pub async fn handle_pull(cli: &Cli, mode: &OutputMode, args: &PullArgs) -> Result<()> {
    // 1. Resolve server and token
    let (server_url, token, mut collection_id) = resolve_pull_params(cli, args)?;

    // 2. Short ID resolution
    if uuid::Uuid::parse_str(&collection_id).is_err() {
        let client = build_sync_http_client()?;
        let response: reqwest::Response = client
            .get(format!("{}/tours", server_url))
            .send()
            .await
            .map_err(|e| Error::Operation(format!("failed to query tours list: {e}")))?;

        if response.status().is_success() {
            let list: serde_json::Value = response
                .json()
                .await
                .map_err(|e| Error::Operation(format!("failed to parse tours list: {e}")))?;
            if let Some(tours) = list["tours"].as_array() {
                let matches: Vec<String> = tours
                    .iter()
                    .filter_map(|t| t["tour_id"].as_str())
                    .filter(|id: &&str| id.starts_with(&collection_id))
                    .map(|s| s.to_string())
                    .collect();

                match matches.len() {
                    1 => {
                        collection_id = matches[0].to_string();
                    }
                    0 => {
                        return Err(Error::Input(format!(
                            "tour ID '{}' not found on server",
                            collection_id
                        )));
                    }
                    _ => {
                        return Err(Error::Input(format!(
                            "tour ID '{}' is ambiguous (matches: {})",
                            collection_id,
                            matches.join(", ")
                        )));
                    }
                }
            }
        }
    }

    // 3. Use unified sync interface
    let save_name =
        args.save.as_ref().map(|s| if s.is_empty() { "".to_string() } else { s.clone() });

    let sync_opts = SyncOptions {
        collection_id,
        server_url,
        direction: SyncDirection::Pull,
        token,
        visibility: None,
        title: None,
        description: None,
        dry_run: false,
        save_name,
        db: None,
        project_root: None,
        cli: cli as *const Cli,
    };

    sync(cli, mode, sync_opts).await
}

fn resolve_pull_params(cli: &Cli, args: &PullArgs) -> Result<(String, Option<String>, String)> {
    if args.tour.starts_with("http://") || args.tour.starts_with("https://") {
        // Parse URL: http://server/tours/id
        let url = args.tour.clone();
        let parts: Vec<&str> = url.rsplitn(2, "/tours/").collect();
        if parts.len() != 2 {
            return Err(Error::Input(
                "invalid tour URL format, expected .../tours/<id>".to_string(),
            ));
        }
        let server_url = parts[1].to_string();
        let tour_id = parts[0].to_string();

        // Check for token in registry first, then use CLI flag
        let registry_token = get_token_for_server(&server_url).ok().flatten();
        let token = args.token.as_ref().or(registry_token.as_ref()).cloned();

        return Ok((server_url, token, tour_id));
    }

    // Bare ID, requires --server
    let server_name = if let Some(ref server) = args.server {
        server.clone()
    } else {
        let config = super::load_config(cli);
        config
            .codetours
            .default_server
            .clone()
            .ok_or_else(|| Error::Input("server is required when pull by ID".to_string()))?
    };

    // If it's a URL, get token from registry
    if server_name.starts_with("http") {
        let registry_token = get_token_for_server(&server_name).ok().flatten();
        let token = args.token.as_ref().or(registry_token.as_ref()).cloned();
        return Ok((server_name.to_string(), token, args.tour.clone()));
    }

    // Look up server in config
    let config = super::load_config(cli);
    let s = config
        .codetours
        .servers
        .iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| Error::Input(format!("server '{}' not found in config", server_name)))?;

    // Priority: CLI flag > Server config > Registry
    let registry_token = get_token_for_server(&s.url).ok().flatten();
    let token = args.token.as_ref().or(s.token.as_ref()).or(registry_token.as_ref()).cloned();

    Ok((s.url.clone(), token, args.tour.clone()))
}
