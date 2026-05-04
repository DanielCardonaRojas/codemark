use crate::cli::output::{OutputMode, write_success};
use crate::cli::*;
use codemark_core::config::Config;
use codemark_core::engine::snapshot::build_snapshot;
use codemark_core::error::{Error, Result};
use codemark_core::storage::pack::Packer;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use rusqlite::params;
use serde_json::Value;

pub async fn handle_publish(
    cli: &Cli,
    mode: &OutputMode,
    args: &PublishArgs,
) -> Result<()> {
    let db = super::open_db_for_write(cli)?;
    let config = super::load_config(cli);

    // 1. Resolve server and token
    let (server_url, token) = resolve_server_and_token(&config, args)?;

    // 2. Find collection
    let collection = if let Some(col) = db.get_collection_by_name(&args.collection)? {
        col
    } else if let Some(col) = db.get_collection_by_id_prefix(&args.collection)? {
        col
    } else {
        return Err(Error::Input(format!("collection '{}' not found", args.collection)));
    };

    // 3. Build snapshot
    let project_root = super::get_project_root(&db);
    // TODO: support --allow-stale
    let mut payload = build_snapshot(&db, &collection.id, &project_root, 5).await?;

    // 4. Override metadata if flags provided
    if let Some(title) = &args.title {
        payload.collection.name = title.clone();
    }
    if let Some(desc) = &args.description {
        payload.collection.description = Some(desc.clone());
    }
    
    // Honor visibility flag
    payload.collection.visibility = args.visibility.parse()
        .map_err(|e| Error::Input(format!("invalid visibility: {e}")))?;

    // 5. Create pack
    let temp_dir = std::env::temp_dir();
    let pack_path = temp_dir.join(format!("codemark-publish-{}.sqlite", uuid::Uuid::new_v4()));
    let packer = Packer::new(&db);
    let result_path = packer.create_pack_from_snapshot(&payload, &pack_path)?;

    if args.dry_run {
        println!("Dry run: pack created at {}", result_path.display());
        return Ok(());
    }

    // 5. Upload pack
    let client = reqwest::Client::new();
    let mut headers = HeaderMap::new();
    headers.insert("X-Tour-Token", HeaderValue::from_str(&token).map_err(|_| Error::Operation("Invalid token".to_string()))?);
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

    // 6. Record in local DB
    db.conn().execute(
        "INSERT INTO published_tours (source_collection_id, server_url, tour_id, last_published_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         ON CONFLICT (source_collection_id, server_url) DO UPDATE
           SET tour_id = excluded.tour_id, last_published_at = excluded.last_published_at",
        params![collection.id, server_url, tour_id],
    )?;

    // 7. Cleanup
    let _ = std::fs::remove_file(&result_path);

    write_success(mode, &format!("Published tour: {}/tours/{}", server_url, tour_id))?;

    Ok(())
}

fn resolve_server_and_token(config: &Config, args: &PublishArgs) -> Result<(String, String)> {
    let server_name = args.server.as_deref().or(config.codetours.default_server.as_deref()).unwrap_or("default");
    
    // Check if it's a URL
    if server_name.starts_with("http://") || server_name.starts_with("https://") {
        let token = args.token.clone().ok_or_else(|| Error::Input("token is required when using a direct server URL".to_string()))?;
        return Ok((server_name.to_string(), token));
    }

    // Lookup in config
    let server_cfg = config.codetours.servers.iter().find(|s| s.name == server_name)
        .ok_or_else(|| Error::Input(format!("server '{}' not found in config", server_name)))?;

    let token = args.token.clone().or(server_cfg.token.clone())
        .ok_or_else(|| Error::Input(format!("token not found for server '{}'", server_name)))?;

    Ok((server_cfg.url.clone(), token))
}
