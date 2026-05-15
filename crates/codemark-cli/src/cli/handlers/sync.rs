//! Unified sync interface for pull/push operations.
//!
//! This module provides a single `sync()` function that handles both
//! downloading (pull) and uploading (push) collections to/from a server.
//! The server preserves the collection_id, so we use it as the unified
//! identifier for both directions.

use crate::cli::Cli;
use crate::cli::output::OutputMode;
use codemark_core::engine::snapshot::build_snapshot;
use codemark_core::error::{Error, Result};
use codemark_core::storage::db::Database;
use codemark_core::storage::pack::{PackReader, Packer, inspect, pre_inspect};
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use rusqlite::params;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Direction of the sync operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    Pull,
    Push,
}

/// Options for the unified sync operation.
pub struct SyncOptions {
    /// Unified identifier (collection_id) - the server preserves this
    pub collection_id: String,

    /// Server URL
    pub server_url: String,

    /// Direction of sync
    pub direction: SyncDirection,

    /// Optional auth token
    pub token: Option<String>,

    // Push-specific options
    /// Collection visibility (for push)
    pub visibility: Option<String>,

    /// Collection title override (for push)
    pub title: Option<String>,

    /// Collection description override (for push)
    pub description: Option<String>,

    /// Dry run - don't actually upload (for push)
    pub dry_run: bool,

    /// Save name for pulled collection (for pull)
    /// Empty string means use original name, Some(name) means custom name
    pub save_name: Option<String>,

    /// Database reference (required for push)
    pub db: Option<Database>,

    /// Project root (required for push)
    pub project_root: Option<String>,

    /// CLI reference (for loading config)
    pub cli: *const Cli,
}

unsafe impl Send for SyncOptions {}

/// Build a reqwest client with reasonable timeouts for sync operations.
pub fn build_sync_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| Error::Operation(format!("failed to build HTTP client: {e}")))
}

/// Build authorization headers for a server request.
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

/// Download a pack from the server and decompress it.
async fn download_pack(
    server_url: &str,
    collection_id: &str,
    token: Option<&String>,
) -> Result<Vec<u8>> {
    let client = build_sync_http_client()?;
    let mut headers = build_auth_headers(token)?;
    headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.codetours.pack+sqlite"));

    let response = client
        .get(format!("{}/tours/{}", server_url, collection_id))
        .headers(headers)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("download failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(Error::Operation(format!("server returned {status}")));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::Operation(format!("failed to read response: {e}")))?;

    // Check if it is zstd compressed
    let is_zstd = bytes.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]);
    if is_zstd {
        tokio::task::spawn_blocking(move || {
            let mut decoder = zstd::stream::read::Decoder::new(&bytes[..])
                .map_err(|e| Error::Operation(format!("zstd decoder failed: {e}")))?;
            let mut out = Vec::new();
            std::io::copy(&mut decoder, &mut out)
                .map_err(|e| Error::Operation(format!("decompression failed: {e}")))?;
            Ok::<_, Error>(out)
        })
        .await
        .map_err(|_| Error::Operation("Blocking task panicked during decompression".to_string()))?
    } else {
        Ok(bytes.to_vec())
    }
}

/// Import a pack into the local database.
async fn import_pack(
    db: &Database,
    pack_path: &std::path::Path,
    collection_name: Option<&str>,
    source_url: &str,
) -> Result<()> {
    let reader = PackReader::open(pack_path)?;

    let tours = reader.tours()?;
    match tours.len() {
        0 => return Err(Error::Operation("pack contains no tours".to_string())),
        1 => (),
        _ => {
            return Err(Error::Operation(
                "pack contains multiple tours; only single-tour packs are supported for pull"
                    .to_string(),
            ));
        }
    }
    let tour = &tours[0];

    // Create local collection
    let collection_id = uuid::Uuid::new_v4().to_string();
    let mut collection = tour.clone();
    collection.id = collection_id.clone();

    // Inherit original name if none provided
    if let Some(name) = collection_name {
        collection.name = name.to_string();
    }

    collection.imported_from_url = Some(source_url.to_string());

    db.insert_collection(&collection)?;

    // Merge bookmarks for this specific collection
    let bookmarks = reader.bookmarks_for_collection(&tour.id)?;
    for mut bm in bookmarks {
        let old_id = bm.id.clone();
        bm.id = uuid::Uuid::new_v4().to_string();
        // Tag with source URL
        bm.tags.push(format!("imported:{}", source_url));

        let bookmark_id = db.insert_bookmark(&bm)?;
        db.add_to_collection(&collection_id, std::slice::from_ref(&bookmark_id))?;

        // Import annotations
        let mut ann_stmt = reader.conn().prepare("SELECT id, bookmark_id, added_at, added_by, notes, context, source FROM bookmark_annotations WHERE bookmark_id = ?1")?;
        let annotations = ann_stmt.query_map([&old_id], |row: &rusqlite::Row| {
            Ok(codemark_core::engine::bookmark::Annotation {
                id: uuid::Uuid::new_v4().to_string(),
                bookmark_id: bookmark_id.clone(),
                added_at: row.get(2)?,
                added_by: row.get(3)?,
                notes: row.get(4)?,
                context: row.get(5)?,
                source: row.get(6)?,
            })
        })?;
        for ann in annotations {
            db.insert_annotation(&ann?)?;
        }

        // Import tags
        let mut tag_stmt = reader.conn().prepare(
            "SELECT bookmark_id, tag, added_at, added_by FROM bookmark_tags WHERE bookmark_id = ?1",
        )?;
        let tags = tag_stmt.query_map([&old_id], |row: &rusqlite::Row| {
            Ok(codemark_core::engine::bookmark::Tag {
                bookmark_id: bookmark_id.clone(),
                tag: row.get(1)?,
                added_at: row.get(2)?,
                added_by: row.get(3)?,
            })
        })?;
        for tag in tags {
            db.insert_tag(&tag?)?;
        }

        // Import comments with ID mapping to preserve threading
        let mut com_stmt = reader.conn().prepare(
            "SELECT id, bookmark_id, author, body, created_at, parent_id FROM bookmark_comments WHERE bookmark_id = ?1",
        )?;
        let rows = com_stmt.query_map([&old_id], |row: &rusqlite::Row| {
            Ok((
                row.get::<_, String>(0)?,         // old_id
                row.get::<_, String>(2)?,         // author
                row.get::<_, String>(3)?,         // body
                row.get::<_, String>(4)?,         // created_at
                row.get::<_, Option<String>>(5)?, // parent_id
            ))
        })?;

        let mut comment_id_map = HashMap::new();
        let mut pending_comments = Vec::new();

        for row in rows {
            let (old_comment_id, author, body, created_at, parent_id) = row?;
            let new_comment_id = uuid::Uuid::new_v4().to_string();
            comment_id_map.insert(old_comment_id, new_comment_id.clone());

            pending_comments.push((new_comment_id, author, body, created_at, parent_id));
        }

        for (new_id, author, body, created_at, old_parent_id) in pending_comments {
            let new_parent_id = old_parent_id.and_then(|id| comment_id_map.get(&id).cloned());
            db.insert_comment(&codemark_core::engine::bookmark::BookmarkComment {
                id: new_id,
                bookmark_id: bookmark_id.clone(),
                author,
                body,
                created_at,
                parent_id: new_parent_id,
            })?;
        }

        // Import resolutions
        let resolutions = reader.resolutions(&old_id)?;
        for mut res in resolutions {
            res.id = uuid::Uuid::new_v4().to_string();
            res.bookmark_id = bookmark_id.clone();
            db.insert_resolution(&res)?;
        }
    }

    Ok(())
}

/// Upload a pack to the server.
async fn upload_pack(
    pack_path: &std::path::Path,
    server_url: &str,
    token: Option<&String>,
    db: &Database,
    collection_id: &str,
) -> Result<String> {
    let client = reqwest::Client::new();
    let mut headers = build_auth_headers(token)?;
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/vnd.codetours.pack+sqlite"));

    let pack_bytes = tokio::fs::read(pack_path)
        .await
        .map_err(|e| Error::Operation(format!("failed to read pack: {e}")))?;

    let response = client
        .post(format!("{}/tours", server_url))
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

    let tour_info: Value = response
        .json()
        .await
        .map_err(|e| Error::Operation(format!("failed to parse server response: {e}")))?;
    let tour_id = tour_info["tour_id"]
        .as_str()
        .ok_or_else(|| Error::Operation("missing tour_id in response".to_string()))?;

    // Record in local DB
    db.conn().execute(
        "INSERT INTO published_tours (source_collection_id, server_url, tour_id, last_published_at)
         VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         ON CONFLICT (source_collection_id, server_url) DO UPDATE
           SET tour_id = excluded.tour_id, last_published_at = excluded.last_published_at",
        params![collection_id, server_url, tour_id],
    )?;

    Ok(tour_id.to_string())
}

/// Perform a sync operation (pull or push).
///
/// This is the main entry point for the unified sync interface.
/// It handles both downloading collections from a server (Pull)
/// and uploading collections to a server (Push).
pub async fn sync(cli: &Cli, mode: &OutputMode, opts: SyncOptions) -> Result<()> {
    match opts.direction {
        SyncDirection::Pull => sync_pull(cli, mode, opts).await,
        SyncDirection::Push => sync_push(cli, mode, opts).await,
    }
}

/// Handle pulling a collection from the server.
async fn sync_pull(cli: &Cli, mode: &OutputMode, opts: SyncOptions) -> Result<()> {
    let temp_dir = std::env::temp_dir();
    let pack_path = temp_dir.join(format!("codemark-pull-{}.sqlite", uuid::Uuid::new_v4()));

    let res = async {
        // Download pack
        let decompressed_bytes =
            download_pack(&opts.server_url, &opts.collection_id, opts.token.as_ref()).await?;

        tokio::fs::write(&pack_path, decompressed_bytes)
            .await
            .map_err(|e| Error::Operation(format!("failed to write pack: {e}")))?;

        // Inspect and Migrate pack
        let user_version = pre_inspect(&pack_path)
            .map_err(|e| Error::Operation(format!("pre-inspection failed: {e}")))?;

        if user_version < codemark_core::storage::db::Database::CURRENT_VERSION {
            let pack_path_clone = pack_path.clone();
            tokio::task::spawn_blocking(move || {
                let mut conn = rusqlite::Connection::open(&pack_path_clone)
                    .map_err(|e| Error::Database(e.to_string()))?;
                codemark_core::storage::db::Database::run_migrations_on(&mut conn)
                    .map_err(|e| Error::Database(e.to_string()))?;
                Ok::<_, Error>(())
            })
            .await
            .map_err(|_| {
                Error::Operation("Blocking task panicked during migration".to_string())
            })??;
        }

        // Full inspection after potential migration
        inspect(&pack_path)
            .map_err(|e| Error::Operation(format!("pack inspection failed: {e}")))?;

        // Pull is now persistent by default - always save locally
        let db = super::open_db_for_write(cli)?;
        let source_url = format!("{}/tours/{}", opts.server_url, opts.collection_id);

        match &opts.save_name {
            Some(name) if !name.is_empty() => {
                // Save with custom name
                import_pack(&db, &pack_path, Some(name), &source_url).await?;
                crate::cli::output::write_success(
                    mode,
                    &format!("Saved as collection '{}'", name),
                )?;
            }
            _ => {
                // Save with original name
                import_pack(&db, &pack_path, None, &source_url).await?;
                crate::cli::output::write_success(mode, "Collection imported successfully")?;
            }
        }

        Ok(())
    }
    .await;

    // Cleanup
    let _ = tokio::fs::remove_file(&pack_path).await;
    res
}

/// Handle pushing a collection to the server.
async fn sync_push(cli: &Cli, mode: &OutputMode, opts: SyncOptions) -> Result<()> {
    let db = opts.db.ok_or_else(|| Error::Operation("database required for push".to_string()))?;
    let project_root = opts
        .project_root
        .ok_or_else(|| Error::Operation("project_root required for push".to_string()))?;

    let config = super::load_config(cli);
    let project_root_path = Path::new(&project_root);

    // Build snapshot
    let mut payload = build_snapshot(&db, &opts.collection_id, project_root_path, &config).await?;

    // Override metadata if provided
    if let Some(ref title) = opts.title {
        payload.collection.name = title.clone();
    }
    if let Some(ref desc) = opts.description {
        payload.collection.description = Some(desc.clone());
    }
    if let Some(ref visibility) = opts.visibility {
        payload.collection.visibility =
            visibility.parse().map_err(|e| Error::Input(format!("invalid visibility: {e}")))?;
    }

    // Create pack
    let temp_dir = std::env::temp_dir();
    let pack_path_dest = temp_dir.join(format!("codemark-publish-{}.sqlite", uuid::Uuid::new_v4()));
    let packer = Packer::new(&db);
    let result_path = packer.create_pack_from_snapshot(&payload, &pack_path_dest)?;

    let res = async {
        if opts.dry_run {
            println!("Dry run: pack created at {}", result_path.display());
            return Ok(());
        }

        // Upload pack
        let tour_id = upload_pack(
            &result_path,
            &opts.server_url,
            opts.token.as_ref(),
            &db,
            &opts.collection_id,
        )
        .await?;

        crate::cli::output::write_success(
            mode,
            &format!("Published tour: {}/tours/{}", opts.server_url, tour_id),
        )?;

        Ok(())
    }
    .await;

    // Cleanup
    let _ = tokio::fs::remove_file(&result_path).await;
    res
}
