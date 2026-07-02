use crate::cli::output::OutputMode;
use crate::cli::*;
use codemark_core::error::{Error, Result};
use codemark_core::sync::{SyncDirection, SyncOptions, build_sync_http_client, sync};

// Re-export auth resolution helpers
use crate::cli::handlers::auth_resolve::{build_auth_headers, get_token_for_server};

pub async fn handle_pull(cli: &Cli, mode: &OutputMode, args: &PullArgs) -> Result<()> {
    if args.p2p {
        #[cfg(feature = "p2p")]
        {
            return handle_pull_p2p(cli, mode, args).await;
        }
        #[cfg(not(feature = "p2p"))]
        {
            return Err(Error::Input(
                "this build was compiled without p2p support; rebuild with `--features p2p`"
                    .to_string(),
            ));
        }
    }

    // 1. Resolve server and token
    let (server_url, token, mut collection_id) = resolve_pull_params(cli, args)?;

    // 2. Short ID resolution.
    // `GET /tours` is an authorization-scoped lookup, so resolving a short id by
    // prefix requires naming the repo. Detect the current repo to scope it.
    if uuid::Uuid::parse_str(&collection_id).is_err() {
        let repos = detect_current_repo().ok_or_else(|| {
            Error::Input(
                "resolving a short tour ID requires a repository scope; \
                 use the full tour ID or URL, or run inside the repository"
                    .to_string(),
            )
        })?;
        // Forward auth: `GET /tours` only surfaces private tours to authenticated
        // callers, so a private short-ID pull must carry the token or it would
        // resolve to an empty list and report "not found".
        let client = build_sync_http_client()?;
        let response: reqwest::Response = client
            .get(format!("{}/tours", server_url))
            .query(&[("repos", repos.as_str())])
            .headers(build_auth_headers(token.as_ref())?)
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

    // 3. Open database for pull
    let db = super::open_db_for_write(cli)?;

    // 4. Use unified sync interface from core crate
    // Pull is now persistent by default - save with original name or custom name
    let save_name = args.name.clone();

    let sync_opts = SyncOptions {
        collection_id,
        server_url,
        direction: SyncDirection::Pull,
        token,
        visibility: None,
        title: None,
        description: None,
        dry_run: false,
        save_name: Some(save_name.unwrap_or_default()),
        db: Some(db),
        project_root: None,
        config: None,
    };

    sync(sync_opts).await?;

    // Show success message
    crate::cli::output::write_success(mode, "Collection imported successfully")?;

    Ok(())
}

/// Serverless peer-to-peer pull: fetch the pack bytes directly from a peer using
/// the provided ticket, then import it exactly like an HTTP pull.
#[cfg(feature = "p2p")]
async fn handle_pull_p2p(cli: &Cli, mode: &OutputMode, args: &PullArgs) -> Result<()> {
    let ticket = resolve_p2p_ticket(&args.tour)?;
    let bytes = codemark_p2p::pull_bytes(&ticket)
        .await
        .map_err(|e| Error::Operation(format!("p2p pull failed: {e:#}")))?;

    let db = super::open_db_for_write(cli)?;
    let name = args.name.as_deref().filter(|n| !n.is_empty());
    codemark_core::sync::import_pack_bytes(&db, bytes, name, "p2p").await?;

    crate::cli::output::write_success(mode, "Collection imported successfully")?;
    Ok(())
}

/// Resolve the ticket argument. A real ticket is a single `blob…` token, but a
/// long ticket is easily truncated when copied out of a terminal, so we also
/// accept a path to a file containing the ticket — transfer the file instead of
/// pasting. Anything that isn't a literal `blob…` ticket but names an existing
/// file is read from disk; both forms are trimmed of surrounding whitespace.
#[cfg(feature = "p2p")]
fn resolve_p2p_ticket(arg: &str) -> Result<String> {
    let trimmed = arg.trim();
    if !trimmed.starts_with("blob") && std::path::Path::new(trimmed).is_file() {
        let contents = std::fs::read_to_string(trimmed).map_err(|e| {
            Error::Operation(format!("failed to read ticket file '{trimmed}': {e}"))
        })?;
        return Ok(contents.trim().to_string());
    }
    Ok(trimmed.to_string())
}

/// Detect the current Git repository as `owner/name`, for scoping a short-id
/// lookup against the authorization-scoped `GET /tours`. Returns `None` if no
/// GitHub repo can be determined (fail-soft).
fn detect_current_repo() -> Option<String> {
    use codemark_core::git::{context as git_context, remote};
    let cwd = std::env::current_dir().ok()?;
    let ctx = git_context::detect_context(&cwd)?;
    let origin_url = remote::get_origin_url(&ctx.repo_root).ok()?;
    let (owner, name) = remote::parse_remote_url(&origin_url)?;
    Some(format!("{}/{}", owner, name))
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
