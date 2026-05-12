use crate::cli::output::{OutputMode, write_success};
use crate::cli::*;
use codemark_core::engine::snapshot::build_snapshot;
use codemark_core::error::{Error, Result};
use codemark_core::storage::pack::Packer;
use codemark_core::storage::registry;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use rusqlite::params;
use serde_json::Value;

pub async fn handle_publish(cli: &Cli, mode: &OutputMode, args: &PublishArgs) -> Result<()> {
    let db = super::open_db_for_write(cli)?;

    // 1. Find collection
    let collection = if let Some(col) = db.get_collection_by_name(&args.collection)? {
        col
    } else if let Some(col) = db.get_collection_by_id_prefix(&args.collection)? {
        col
    } else {
        return Err(Error::Input(format!("collection '{}' not found", args.collection)));
    };

    // 2. Detect current repo URL for server discovery
    let repo_url = detect_current_repo()?;

    // 3. Resolve server and token from registry (with CLI token as fallback)
    let (server_url, registry_token) =
        resolve_server_and_token(cli, args.server.as_deref(), repo_url.as_deref())?;

    // Use CLI token if provided, otherwise use registry token
    let token = args.token.as_ref().or(registry_token.as_ref());

    // 4. Build snapshot
    let project_root = super::get_project_root(&db);
    let config = super::load_config(cli);
    // TODO: support --allow-stale
    let mut payload = build_snapshot(&db, &collection.id, &project_root, &config).await?;

    // 5. Override metadata if flags provided
    if let Some(title) = &args.title {
        payload.collection.name = title.clone();
    }
    if let Some(desc) = &args.description {
        payload.collection.description = Some(desc.clone());
    }

    // Honor visibility flag
    payload.collection.visibility =
        args.visibility.parse().map_err(|e| Error::Input(format!("invalid visibility: {e}")))?;

    // 6. Create pack
    let temp_dir = std::env::temp_dir();
    let pack_path_dest = temp_dir.join(format!("codemark-publish-{}.sqlite", uuid::Uuid::new_v4()));
    let packer = Packer::new(&db);
    let result_path = packer.create_pack_from_snapshot(&payload, &pack_path_dest)?;

    // Use a block to ensure cleanup on all exit paths
    let res = async {
        if args.dry_run {
            println!("Dry run: pack created at {}", result_path.display());
            return Ok(());
        }

        // 7. Upload pack with auth headers
        let client = reqwest::Client::new();
        let mut headers = build_auth_headers(token)?;

        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/vnd.codetours.pack+sqlite"));

        let pack_bytes = tokio::fs::read(&result_path).await.map_err(|e| Error::Operation(format!("failed to read pack: {e}")))?;

        let response = client.post(format!("{}/tours", server_url))
            .headers(headers)
            .body(pack_bytes)
            .send()
            .await
            .map_err(|e| Error::Operation(format!("upload failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Operation(format!("server returned {status}: {body}")));
        }

        let tour_info: Value = response.json().await.map_err(|e| Error::Operation(format!("failed to parse server response: {e}")))?;
        let tour_id = tour_info["tour_id"].as_str().ok_or_else(|| Error::Operation("missing tour_id in response".to_string()))?;

        // 8. Record in local DB
        db.conn().execute(
            "INSERT INTO published_tours (source_collection_id, server_url, tour_id, last_published_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT (source_collection_id, server_url) DO UPDATE
               SET tour_id = excluded.tour_id, last_published_at = excluded.last_published_at",
            params![collection.id, server_url, tour_id],
        )?;

        write_success(mode, &format!("Published tour: {}/tours/{}", server_url, tour_id))?;
        Ok(())
    }.await;

    // 9. Cleanup
    let _ = tokio::fs::remove_file(&result_path).await;
    res
}

/// Detect the current repository's GitHub URL.
fn detect_current_repo() -> Result<Option<String>> {
    use codemark_core::git::remote;

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
        // Direct URL - check for token in registry, but don't fail if not found
        // The caller may provide a token via --token flag
        let registry_token = get_token_for_server(server).unwrap_or_default();
        return Ok((server.to_string(), registry_token));
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

/// Build authorization headers for a server request.
///
/// Uses Bearer token for JWT-based auth, fallback to X-Tour-Token for legacy.
fn build_auth_headers(token: Option<&String>) -> Result<HeaderMap> {
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
fn get_token_for_server(server_url: &str) -> Result<Option<String>> {
    let conn = registry::open_registry()?;
    let server = registry::get_server(&conn, server_url)
        .map_err(|e| Error::Database(format!("Failed to query server: {}", e)))?;

    Ok(server.and_then(|s| s.token))
}
