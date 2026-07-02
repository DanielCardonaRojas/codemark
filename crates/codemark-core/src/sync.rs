//! Unified sync interface for pull/push operations.
//!
//! This module provides a single `sync()` function that handles both
//! downloading (pull) and uploading (push) collections to/from a server.
//! The server preserves the collection_id, so we use it as the unified
//! identifier for both directions.

use crate::config::Config;
use crate::engine::snapshot::build_snapshot;
use crate::error::{Error, Result};
use crate::storage::db::Database;
use crate::storage::pack::{PackReader, Packer, inspect, pre_inspect};
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

    /// Database reference (required for push and pull)
    pub db: Option<Database>,

    /// Project root (required for push)
    pub project_root: Option<String>,

    /// Config (required for push)
    pub config: Option<Config>,
}

// SAFETY: Database wraps rusqlite::Connection which is !Send, but SyncOptions is only
// transferred between threads before use — the Database is opened on the target thread
// in sync_pull, and consumed (not shared) in sync_push.
unsafe impl Send for SyncOptions {}

/// Build a reqwest client with reasonable timeouts for sync operations.
pub fn build_sync_http_client() -> Result<reqwest::Client> {
    tracing::debug!(target: "codemark::http", "building HTTP client");
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

/// Resolve server URL and token from config and registry.
///
/// This function handles the logic of resolving which server to use and
/// obtaining the authentication token. It follows this priority order:
///
/// 1. If `default_server` is a direct URL (starts with http), use it
/// 2. If `default_server` is a named server, look it up in config.servers
/// 3. If no config, use the global default account from the registry
/// 4. Get token from config (for named servers) or fallback to registry
///
/// Returns `(server_url, token)` tuple where token may be None if not found.
pub fn resolve_server_and_token(config: &Config) -> Result<(String, Option<String>)> {
    use crate::storage::registry;

    let (server_url, mut token) = if let Some(ref server_name) = config.codetours.default_server {
        if server_name.starts_with("http") {
            // Direct URL in default_server
            (server_name.clone(), None)
        } else {
            // Named server - look up in config
            if let Some(s) =
                config.codetours.servers.iter().find(|s| s.name == server_name.as_str())
            {
                (s.url.clone(), s.token.clone())
            } else {
                return Err(Error::Input(format!("server '{}' not found in config", server_name)));
            }
        }
    } else if let Some(account) = registry::open_registry()
        .ok()
        .and_then(|conn| registry::get_global_default_account(&conn).ok())
        .flatten()
    {
        // No config — fall back to the global default account from the registry
        (account.server_url, Some(account.token))
    } else {
        return Err(Error::Input("No default_server configured".to_string()));
    };

    // Normalize the server URL by dropping trailing slashes. `codemark auth
    // login` stores the normalized URL, so the registry token lookup below must
    // use the same form (a configured `http://host/` would otherwise miss the
    // stored token); it also keeps callers from building `host//tours` URLs.
    let server_url = server_url.trim_end_matches('/').to_string();

    // Try to get token from registry as fallback (if not in config)
    if token.is_none() {
        token = registry::open_registry()
            .ok()
            .and_then(|conn| registry::resolve_token(&conn, &server_url, None, None).ok())
            .flatten();
    }

    Ok((server_url, token))
}

/// Summary of a remote tour available on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTourSummary {
    pub tour_id: String,
    pub title: String,
    pub repo_url: Option<String>,
    /// GitHub login of the user who published the tour, if known.
    pub author: Option<String>,
    pub updated_at: String,
}

/// Options for listing remote tours.
pub struct ListRemoteToursOptions {
    pub server_url: String,
    pub token: Option<String>,
    /// Repositories to scope the lookup to, each as `owner/name`. Sent as the
    /// comma-separated `repos` query param (`GET /tours` is an authorization-
    /// scoped lookup and requires at least one repo). Empty → the server will
    /// reject the request with `400 repos_required`.
    pub repos: Vec<String>,
}

/// Fetch available tours from the server.
pub async fn list_remote_tours(opts: ListRemoteToursOptions) -> Result<Vec<RemoteTourSummary>> {
    let client = build_sync_http_client()?;
    let headers = build_auth_headers(opts.token.as_ref())?;

    let url = format!("{}/tours", opts.server_url);

    // Collect query params up front so the request URL — including the repo
    // scope — can be logged (the request builder doesn't expose them afterward).
    // Scope via the canonical comma-separated `repos` param (each `owner/name`).
    let mut query: Vec<(&str, String)> = Vec::new();
    if !opts.repos.is_empty() {
        query.push(("repos", opts.repos.join(",")));
    }

    let full_url = reqwest::Url::parse_with_params(&url, &query)
        .map_err(|e| Error::Operation(format!("invalid tours URL {url}: {e}")))?;
    tracing::debug!(target: "codemark::http", url = %full_url, "GET /tours");

    let response = client
        .get(full_url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| Error::Operation(format!("failed to list remote tours: {e}")))?;

    tracing::debug!(target: "codemark::http", status = %response.status(), "response received");

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(Error::Operation(format!("server returned {status}: {body}")));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| Error::Operation(format!("failed to parse response: {e}")))?;

    let tours = body["tours"]
        .as_array()
        .ok_or_else(|| Error::Operation("invalid response: missing tours array".to_string()))?;

    let summaries = tours
        .iter()
        .filter(|t| t["tour_id"].as_str().is_some_and(|id| !id.is_empty()))
        .map(|t| RemoteTourSummary {
            tour_id: t["tour_id"].as_str().unwrap_or_default().to_string(),
            title: t["title"].as_str().unwrap_or("Untitled").to_string(),
            repo_url: t["repo_url"].as_str().map(|s| s.to_string()),
            author: t["author"].as_str().map(|s| s.to_string()),
            updated_at: t["updated_at"].as_str().unwrap_or_default().to_string(),
        })
        .collect();

    Ok(summaries)
}

/// Download a pack from the server and decompress it.
async fn download_pack(
    server_url: &str,
    collection_id: &str,
    token: Option<&String>,
) -> Result<Vec<u8>> {
    tracing::debug!(target: "codemark::http", server = %server_url, collection_id = %collection_id, "downloading pack");
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

    decompress_if_zstd(bytes.to_vec()).await
}

/// Decompress `bytes` if they carry the zstd magic number, otherwise return them
/// unchanged. Shared by the HTTP pull path and the p2p import path.
async fn decompress_if_zstd(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if !bytes.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        return Ok(bytes);
    }
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
}

/// Migrate an on-disk pack to the current schema version if needed, then validate
/// it. Shared by the HTTP pull path and the p2p import path.
async fn prepare_pack_file(pack_path: &Path) -> Result<()> {
    let user_version = pre_inspect(pack_path)
        .map_err(|e| Error::Operation(format!("pre-inspection failed: {e}")))?;

    if user_version < Database::CURRENT_VERSION {
        let pack_path_clone = pack_path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut conn = rusqlite::Connection::open(&pack_path_clone)
                .map_err(|e| Error::Database(e.to_string()))?;
            Database::run_migrations_on(&mut conn).map_err(|e| Error::Database(e.to_string()))?;
            Ok::<_, Error>(())
        })
        .await
        .map_err(|_| Error::Operation("Blocking task panicked during migration".to_string()))??;
    }

    inspect(pack_path).map_err(|e| Error::Operation(format!("pack inspection failed: {e}")))?;
    Ok(())
}

/// Build a single-collection pack and return its (compressed) bytes.
///
/// This is the same portable pack format used by HTTP push/pull, so the bytes
/// are transport-agnostic: any channel (e.g. the p2p transport) can move them
/// and reconstruct the tour with [`import_pack_bytes`].
pub async fn build_pack_bytes(
    db: &Database,
    collection_id: &str,
    project_root: &Path,
    config: &Config,
    title: Option<&str>,
    description: Option<&str>,
) -> Result<Vec<u8>> {
    let mut payload = build_snapshot(db, collection_id, project_root, config).await?;

    if let Some(title) = title {
        payload.collection.name = title.to_string();
    }
    if let Some(description) = description {
        payload.collection.description = Some(description.to_string());
    }
    // Stamp the origin repo so the receiver can scope/anchor the tour, matching
    // the HTTP push path.
    if payload.collection.repo_url.is_none()
        && let Ok(origin_url) = crate::git::remote::get_origin_url(project_root)
    {
        payload.collection.repo_url = Some(origin_url);
    }

    let dest =
        std::env::temp_dir().join(format!("codemark-p2p-push-{}.sqlite", uuid::Uuid::new_v4()));
    let packer = Packer::new(db);
    let pack_path = packer.create_pack_from_snapshot(&payload, &dest)?;

    let read = tokio::fs::read(&pack_path)
        .await
        .map_err(|e| Error::Operation(format!("failed to read pack: {e}")));
    let _ = tokio::fs::remove_file(&pack_path).await;
    read
}

/// Import a pack produced by [`build_pack_bytes`] (or the HTTP pull path) into
/// the local database. Accepts either compressed or raw pack bytes.
pub async fn import_pack_bytes(
    db: &Database,
    bytes: Vec<u8>,
    collection_name: Option<&str>,
    source_url: &str,
) -> Result<ImportedTour> {
    let bytes = decompress_if_zstd(bytes).await?;
    let pack_path =
        std::env::temp_dir().join(format!("codemark-p2p-pull-{}.sqlite", uuid::Uuid::new_v4()));

    let res = async {
        tokio::fs::write(&pack_path, &bytes)
            .await
            .map_err(|e| Error::Operation(format!("failed to write pack: {e}")))?;
        prepare_pack_file(&pack_path).await?;
        import_pack(db, &pack_path, collection_name, source_url).await
    }
    .await;

    let _ = tokio::fs::remove_file(&pack_path).await;
    res
}

/// Summary of what a pull/import created locally.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportedTour {
    pub collection_id: String,
    pub name: String,
    pub bookmark_count: usize,
}

/// Import a pack into the local database.
async fn import_pack(
    db: &Database,
    pack_path: &std::path::Path,
    collection_name: Option<&str>,
    source_url: &str,
) -> Result<ImportedTour> {
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

    // Run the whole import in a single transaction so a failure part-way
    // through can't leave a half-imported collection — or two collections
    // sharing the same `imported_from_url` (the old one plus an incomplete new
    // one). On any early return below, `tx` is dropped without committing and
    // every write here rolls back atomically. `insert_bookmark` (and the other
    // helpers) detect the active transaction and run inside it rather than
    // opening a nested one.
    let tx = db.conn().unchecked_transaction()?;

    // Make pull idempotent: if this exact tour was already imported, we drop the
    // previous copies so re-pulling doesn't mint another. We capture *all* prior
    // copies (a database may carry duplicates accumulated under the old pull
    // behavior) and remove them after the new copy is imported (see below);
    // since it shares this transaction, either both the new import and the
    // old-copy removal commit, or neither does.
    let existing_collections = db.get_collections_by_imported_url(source_url)?;

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
    let bookmark_count = bookmarks.len();
    for mut bm in bookmarks {
        let old_id = bm.id.clone();
        let generated_id = uuid::Uuid::new_v4().to_string();
        bm.id = generated_id.clone();
        // Tag with source URL
        bm.tags.push(format!("imported:{}", source_url));

        let bookmark_id = db.insert_bookmark(&bm)?;
        db.add_to_collection(&collection_id, std::slice::from_ref(&bookmark_id))?;

        // `insert_bookmark` dedupes by (file_path, query): it returns our freshly
        // generated id only when it actually inserted a new row, otherwise it
        // returns the id of a pre-existing bookmark. A reused bookmark already
        // carries its annotations/comments/resolutions from a prior import — the
        // per-bookmark metadata below mints fresh ids on every call, so
        // re-importing it would pile up duplicate notes. This is the case hit
        // when a collection is deleted (which orphans, but does not remove, its
        // bookmarks) and then re-pulled. Skip metadata for reused bookmarks.
        //
        // Tags are exempt: `insert_tag` is `INSERT OR IGNORE` on (bookmark_id,
        // tag), so re-importing them is already idempotent.
        let bookmark_is_new = bookmark_id == generated_id;

        // Import tags
        let mut tag_stmt = reader.conn().prepare(
            "SELECT bookmark_id, tag, added_at, added_by FROM bookmark_tags WHERE bookmark_id = ?1",
        )?;
        let tags = tag_stmt.query_map([&old_id], |row: &rusqlite::Row| {
            Ok(crate::engine::bookmark::Tag {
                bookmark_id: bookmark_id.clone(),
                tag: row.get(1)?,
                added_at: row.get(2)?,
                added_by: row.get(3)?,
            })
        })?;
        for tag in tags {
            db.insert_tag(&tag?)?;
        }

        if !bookmark_is_new {
            continue;
        }

        // Import annotations
        let mut ann_stmt = reader.conn().prepare("SELECT id, bookmark_id, added_at, added_by, notes, context, source FROM bookmark_annotations WHERE bookmark_id = ?1")?;
        let annotations = ann_stmt.query_map([&old_id], |row: &rusqlite::Row| {
            Ok(crate::engine::bookmark::Annotation {
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
            db.insert_comment(&crate::engine::bookmark::BookmarkComment {
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

    // Now that the fresh copy is fully imported, drop the previous copies.
    //
    // Only the collection rows are removed (their membership rows cascade away),
    // NOT their bookmarks: a recursive delete would unconditionally remove
    // bookmarks the user may have added to their own collections, silently
    // orphaning them. The bookmarks were reused above (see the per-bookmark
    // metadata skip), so they now belong to the freshly imported collection
    // without duplicated notes.
    for existing in existing_collections {
        db.delete_collection_by_id(&existing.id)?;
    }

    tx.commit()?;
    Ok(ImportedTour { collection_id, name: collection.name, bookmark_count })
}

/// Upload a pack to the server.
async fn upload_pack(
    pack_path: &std::path::Path,
    server_url: &str,
    token: Option<&String>,
    db: &Database,
    collection_id: &str,
) -> Result<String> {
    tracing::debug!(target: "codemark::http", server = %server_url, collection_id = %collection_id, "uploading pack");
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

    // Also update the collection's published_at field for UI display
    db.conn().execute(
        "UPDATE collections SET published_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        params![collection_id],
    )?;

    Ok(tour_id.to_string())
}

/// Perform a sync operation (pull or push).
///
/// This is the main entry point for the unified sync interface.
/// It handles both downloading collections from a server (Pull)
/// and uploading collections to a server (Push).
pub async fn sync(opts: SyncOptions) -> Result<()> {
    match opts.direction {
        SyncDirection::Pull => sync_pull(opts).await,
        SyncDirection::Push => sync_push(opts).await,
    }
}

/// Handle pulling a collection from the server.
async fn sync_pull(opts: SyncOptions) -> Result<()> {
    tracing::info!(target: "codemark::http", direction = "pull", collection_id = %opts.collection_id, server = %opts.server_url, "starting sync");
    let db = opts.db.ok_or_else(|| Error::Operation("database required for pull".to_string()))?;

    let temp_dir = std::env::temp_dir();
    let pack_path = temp_dir.join(format!("codemark-pull-{}.sqlite", uuid::Uuid::new_v4()));

    let res = async {
        // Download pack
        let decompressed_bytes =
            download_pack(&opts.server_url, &opts.collection_id, opts.token.as_ref()).await?;

        tokio::fs::write(&pack_path, decompressed_bytes)
            .await
            .map_err(|e| Error::Operation(format!("failed to write pack: {e}")))?;

        // Inspect and migrate pack to the current schema version.
        prepare_pack_file(&pack_path).await?;

        // Pull is now persistent by default - always save locally
        let source_url = format!("{}/tours/{}", opts.server_url, opts.collection_id);

        match &opts.save_name {
            Some(name) if !name.is_empty() => {
                // Save with custom name
                import_pack(&db, &pack_path, Some(name), &source_url).await?;
            }
            _ => {
                // Save with original name
                import_pack(&db, &pack_path, None, &source_url).await?;
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
async fn sync_push(opts: SyncOptions) -> Result<()> {
    tracing::info!(target: "codemark::http", direction = "push", collection_id = %opts.collection_id, server = %opts.server_url, "starting sync");
    let db = opts.db.ok_or_else(|| Error::Operation("database required for push".to_string()))?;
    let project_root = opts
        .project_root
        .ok_or_else(|| Error::Operation("project_root required for push".to_string()))?;
    let config =
        opts.config.ok_or_else(|| Error::Operation("config required for push".to_string()))?;

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

    // Set repo_url from git origin if not already set
    if payload.collection.repo_url.is_none()
        && let Ok(origin_url) = crate::git::remote::get_origin_url(project_root_path)
    {
        payload.collection.repo_url = Some(origin_url);
    }

    // Create pack
    let temp_dir = std::env::temp_dir();
    let pack_path_dest = temp_dir.join(format!("codemark-publish-{}.sqlite", uuid::Uuid::new_v4()));
    let packer = Packer::new(&db);
    let result_path = packer.create_pack_from_snapshot(&payload, &pack_path_dest)?;

    let res = async {
        if opts.dry_run {
            return Err(Error::Operation("dry_run: pack created but not uploaded".to_string()));
        }

        // Upload pack
        upload_pack(&result_path, &opts.server_url, opts.token.as_ref(), &db, &opts.collection_id)
            .await?;

        Ok(())
    }
    .await;

    // Cleanup
    let _ = tokio::fs::remove_file(&result_path).await;
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_auth_headers_jwt() {
        let token = Some("eyJtest".to_string());
        let headers = build_auth_headers(token.as_ref()).unwrap();
        assert_eq!(headers.get("Authorization").unwrap().to_str().unwrap(), "Bearer eyJtest");
    }

    #[test]
    fn test_build_auth_headers_legacy() {
        let token = Some("legacy_token".to_string());
        let headers = build_auth_headers(token.as_ref()).unwrap();
        assert_eq!(headers.get("X-Tour-Token").unwrap().to_str().unwrap(), "legacy_token");
    }

    #[test]
    fn test_build_auth_headers_none() {
        let headers = build_auth_headers(None).unwrap();
        assert!(headers.get("Authorization").is_none());
        assert!(headers.get("X-Tour-Token").is_none());
    }

    /// Build a single-tour pack file on disk, returning its path and the source
    /// URL to import it under. The pack carries one bookmark with one annotation.
    fn write_test_pack() -> (std::path::PathBuf, String) {
        use crate::engine::bookmark::{
            Annotation, Bookmark, BookmarkHealth, Collection, ResolutionMethod, Visibility,
        };

        let pack_path = std::env::temp_dir()
            .join(format!("codemark-test-pack-{}.sqlite", uuid::Uuid::new_v4()));

        {
            let db = Database::create(&pack_path).unwrap();

            let collection = Collection {
                id: "pack-collection".to_string(),
                name: "Shared Tour".to_string(),
                description: None,
                visibility: Visibility::Public,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                created_by: None,
                created_branch: None,
                published_at: None,
                published_commit_sha: None,
                repo_url: None,
                repo_id: None,
                status: None,
                health: None,
                health_computed_at: None,
                updated_at: None,
                imported_from_url: None,
            };
            db.insert_collection(&collection).unwrap();

            let bookmark = Bookmark {
                id: "pack-bookmark".to_string(),
                query: "(function_declaration) @target".to_string(),
                language: "swift".to_string(),
                file_path: "src/main.swift".to_string(),
                content_hash: Some("sha256:abcd".to_string()),
                commit_hash: Some("abc123".to_string()),
                health: BookmarkHealth::Active,
                resolution_method: Some(ResolutionMethod::Exact),
                last_resolved_at: None,
                stale_since: None,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                created_by: None,
                current_resolution_id: None,
                repo_id: None,
                tags: Vec::new(),
                annotations: Vec::new(),
                comments: Vec::new(),
            };
            let bookmark_id = db.insert_bookmark(&bookmark).unwrap();
            db.add_to_collection(&collection.id, std::slice::from_ref(&bookmark_id)).unwrap();

            db.insert_annotation(&Annotation {
                id: "pack-annotation".to_string(),
                bookmark_id: bookmark_id.clone(),
                added_at: "2026-01-01T00:00:00Z".to_string(),
                added_by: Some("author".to_string()),
                notes: Some("the one and only note".to_string()),
                context: None,
                source: None,
            })
            .unwrap();
        }

        (pack_path, "http://example.com/tours/pack-collection".to_string())
    }

    // Regression: pulling the same collection twice must not create a duplicate
    // local collection, nor duplicate the bookmark annotations. `import_pack` is
    // idempotent per `imported_from_url`, so a re-pull refreshes the existing
    // copy instead of piling up collections and notes.
    #[tokio::test]
    async fn import_pack_twice_is_idempotent() {
        let (pack_path, source_url) = write_test_pack();
        let db = Database::open_in_memory().unwrap();

        import_pack(&db, &pack_path, None, &source_url).await.unwrap();
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();

        // Exactly one collection was imported (no duplicate on re-pull).
        let collections = db.list_collections().unwrap();
        assert_eq!(collections.len(), 1, "re-pull must not duplicate the collection");

        // Its single bookmark carries exactly one annotation (no duplicate note).
        let collection_id = collections[0].0.id.clone();
        let bookmarks = db.list_bookmarks_in_collection(&collection_id).unwrap();
        assert_eq!(bookmarks.len(), 1, "re-pull must not duplicate bookmarks");
        assert_eq!(
            bookmarks[0].annotations.len(),
            1,
            "re-pull must not duplicate bookmark annotations"
        );

        let _ = std::fs::remove_file(&pack_path);
    }

    // Regression: a re-pull must repair a database that already carries multiple
    // collections for the same source URL (duplicates accumulated under the old,
    // non-idempotent pull behavior) — all prior copies are removed, not just one.
    #[tokio::test]
    async fn re_pull_repairs_preexisting_duplicate_collections() {
        use crate::engine::bookmark::{Collection, Visibility};

        let (pack_path, source_url) = write_test_pack();
        let db = Database::open_in_memory().unwrap();

        // First import creates one collection tagged with the source URL.
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();

        // Simulate legacy state: a second collection sharing the same
        // imported_from_url, as the old duplicating pull would have produced.
        let legacy_dup = Collection {
            id: "legacy-duplicate".to_string(),
            name: "Shared Tour (dup)".to_string(),
            description: None,
            visibility: Visibility::Public,
            created_at: "2020-01-01T00:00:00Z".to_string(),
            created_by: None,
            created_branch: None,
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: None,
            health_computed_at: None,
            updated_at: None,
            imported_from_url: Some(source_url.clone()),
        };
        db.insert_collection(&legacy_dup).unwrap();
        assert_eq!(db.list_collections().unwrap().len(), 2);

        // Re-pull: both prior copies are removed and replaced by a single fresh one.
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();
        assert_eq!(
            db.list_collections().unwrap().len(),
            1,
            "re-pull must remove every prior copy, not just one"
        );

        let _ = std::fs::remove_file(&pack_path);
    }

    // Regression for the reported flow: pull a collection, delete it (a
    // non-recursive delete that orphans but keeps its bookmarks), then re-pull.
    // The orphaned bookmark is reused by (file_path, query), so its annotations
    // must not be re-imported — otherwise every note is duplicated on re-pull.
    #[tokio::test]
    async fn re_pull_after_deleting_collection_does_not_duplicate_notes() {
        let (pack_path, source_url) = write_test_pack();
        let db = Database::open_in_memory().unwrap();

        // First pull.
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();

        // Delete the collection without removing its bookmarks (the default
        // `delete` behaviour), leaving the bookmark + its annotation orphaned.
        let first = db.list_collections().unwrap();
        assert_eq!(first.len(), 1);
        db.delete_collection_by_id(&first[0].0.id).unwrap();

        // Re-pull the same collection.
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();

        let collections = db.list_collections().unwrap();
        assert_eq!(collections.len(), 1, "re-pull must not duplicate the collection");

        let bookmarks = db.list_bookmarks_in_collection(&collections[0].0.id).unwrap();
        assert_eq!(bookmarks.len(), 1, "re-pull must reuse the orphaned bookmark");
        assert_eq!(
            bookmarks[0].annotations.len(),
            1,
            "re-pull must not duplicate the orphaned bookmark's annotations"
        );

        let _ = std::fs::remove_file(&pack_path);
    }

    // Regression: re-pulling a collection must not delete a bookmark the user
    // has added to one of their own collections. The refresh drops only the
    // imported collection row, never its bookmarks, so shared bookmarks survive.
    #[tokio::test]
    async fn re_pull_preserves_bookmarks_shared_with_user_collection() {
        use crate::engine::bookmark::{Collection, Visibility};

        let (pack_path, source_url) = write_test_pack();
        let db = Database::open_in_memory().unwrap();

        // First pull, then grab the imported bookmark's id.
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();
        let imported = db.list_collections().unwrap();
        let imported_bm_id =
            db.list_bookmarks_in_collection(&imported[0].0.id).unwrap()[0].id.clone();

        // The user adds that bookmark to their own, separate collection.
        let user_collection = Collection {
            id: "user-collection".to_string(),
            name: "My Collection".to_string(),
            description: None,
            visibility: Visibility::Private,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            created_branch: None,
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: None,
            health_computed_at: None,
            updated_at: None,
            imported_from_url: None,
        };
        db.insert_collection(&user_collection).unwrap();
        db.add_to_collection(&user_collection.id, std::slice::from_ref(&imported_bm_id)).unwrap();

        // Re-pull the imported collection.
        import_pack(&db, &pack_path, None, &source_url).await.unwrap();

        // The user's collection still holds the shared bookmark — it was not
        // deleted out from under them.
        let still_there = db.list_bookmarks_in_collection(&user_collection.id).unwrap();
        assert_eq!(still_there.len(), 1, "re-pull must not delete a user-shared bookmark");
        assert_eq!(still_there[0].id, imported_bm_id);

        let _ = std::fs::remove_file(&pack_path);
    }

    #[test]
    fn resolve_server_and_token_strips_trailing_slash() {
        use crate::config::{CodetoursConfig, CodetoursServerConfig, Config};

        // A named server whose URL carries a trailing slash and an inline token.
        // The returned URL must be normalized so registry lookups and request
        // URLs match the form stored by `codemark auth login`.
        let config = Config {
            codetours: CodetoursConfig {
                default_server: Some("remote".to_string()),
                servers: vec![CodetoursServerConfig {
                    name: "remote".to_string(),
                    url: "http://example.com/".to_string(),
                    token: Some("tok".to_string()),
                }],
            },
            ..Default::default()
        };

        let (server_url, token) = resolve_server_and_token(&config).expect("resolve");
        assert_eq!(server_url, "http://example.com");
        assert_eq!(token.as_deref(), Some("tok"));
    }
}
