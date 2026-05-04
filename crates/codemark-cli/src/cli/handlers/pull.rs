use crate::cli::output::{OutputMode, write_success};
use crate::cli::*;
use codemark_core::config::Config;
use codemark_core::error::{Error, Result};
use codemark_core::storage::pack::{PackReader, pre_inspect, inspect};
use comfy_table::Table;
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use std::collections::HashMap;

pub async fn handle_pull(cli: &Cli, mode: &OutputMode, args: &PullArgs) -> Result<()> {
    let config = super::load_config(cli);

    // 1. Resolve server and token
    let (server_url, token, mut tour_id) = resolve_pull_params(&config, args)?;

    // 2. Short ID resolution
    if uuid::Uuid::parse_str(&tour_id).is_err() {
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{}/tours", server_url))
            .send()
            .await
            .map_err(|e| Error::Operation(format!("failed to query tours list: {e}")))?;

        if response.status().is_success() {
            let list: serde_json::Value = response.json().await.map_err(|e| {
                Error::Operation(format!("failed to parse tours list: {e}"))
            })?;
            if let Some(tours) = list["tours"].as_array() {
                let matches: Vec<&str> = tours
                    .iter()
                    .filter_map(|t| t["tour_id"].as_str())
                    .filter(|id| id.starts_with(&tour_id))
                    .collect();

                match matches.len() {
                    1 => {
                        tour_id = matches[0].to_string();
                    }
                    0 => {
                        return Err(Error::Input(format!(
                            "tour ID '{}' not found on server",
                            tour_id
                        )));
                    }
                    _ => {
                        return Err(Error::Input(format!(
                            "tour ID '{}' is ambiguous (matches: {})",
                            tour_id,
                            matches.join(", ")
                        )));
                    }
                }
            }
        }
    }

    // 3. Download pack
    let temp_dir = std::env::temp_dir();
    let pack_path = temp_dir.join(format!("codemark-pull-{}.sqlite", uuid::Uuid::new_v4()));

    let res = async {
        let client = reqwest::Client::new();
        let mut headers = HeaderMap::new();
        if let Some(t) = token {
            headers.insert(
                "X-Tour-Token",
                HeaderValue::from_str(&t).map_err(|_| Error::Operation("Invalid token".to_string()))?,
            );
        }
        headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.codetours.pack+sqlite"));

        let response = client
            .get(format!("{}/tours/{}", server_url, tour_id))
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
        let decompressed_bytes = if is_zstd {
            tokio::task::spawn_blocking(move || {
                let mut decoder = zstd::stream::read::Decoder::new(&bytes[..])
                    .map_err(|e| Error::Operation(format!("zstd decoder failed: {e}")))?;
                let mut out = Vec::new();
                std::io::copy(&mut decoder, &mut out)
                    .map_err(|e| Error::Operation(format!("decompression failed: {e}")))?;
                Ok::<_, Error>(out)
            })
            .await
            .map_err(|_| {
                Error::Operation("Blocking task panicked during decompression".to_string())
            })??
        } else {
            bytes.to_vec()
        };

        tokio::fs::write(&pack_path, decompressed_bytes)
            .await
            .map_err(|e| Error::Operation(format!("failed to write pack: {e}")))?;

        // 4. Inspect and Migrate pack
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
            .map_err(|_| Error::Operation("Blocking task panicked during migration".to_string()))??;
        }

        // Full inspection after potential migration
        inspect(&pack_path).map_err(|e| Error::Operation(format!("pack inspection failed: {e}")))?;

        if let Some(save_name) = &args.save {
            let name_override = if save_name.is_empty() { None } else { Some(save_name.as_str()) };
            handle_save_pulled(
                cli,
                mode,
                &pack_path,
                name_override,
                &format!("{}/tours/{}", server_url, tour_id),
            )
            .await?;
        } else {
            handle_display_pulled(mode, &pack_path).await?;
        }
        Ok(())
    }.await;

    // cleanup
    let _ = tokio::fs::remove_file(&pack_path).await;
    res
}

async fn handle_save_pulled(
    cli: &Cli,
    mode: &OutputMode,
    pack_path: &std::path::Path,
    collection_name: Option<&str>,
    source_url: &str,
) -> Result<()> {
    let db = super::open_db_for_write(cli)?;
    let reader = PackReader::open(pack_path)?;

    let tours = reader.tours()?;
    if tours.is_empty() {
        return Err(Error::Operation("pack contains no tours".to_string()));
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
    let final_name = collection.name.clone();

    // Merge bookmarks
    let bookmarks = reader.bookmarks()?;
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
                row.get::<_, String>(0)?, // old_id
                row.get::<_, String>(2)?, // author
                row.get::<_, String>(3)?, // body
                row.get::<_, String>(4)?, // created_at
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

    write_success(mode, &format!("Saved tour as collection '{}'", final_name))?;
    Ok(())
}

async fn handle_display_pulled(_mode: &OutputMode, pack_path: &std::path::Path) -> Result<()> {
    let reader = PackReader::open(pack_path)?;
    let tours = reader.tours()?;
    if tours.is_empty() {
        println!("Pack contains no tours.");
        return Ok(());
    }

    for tour in tours {
        println!("Tour: {} ({})", tour.name, tour.id);
        if let Some(desc) = &tour.description {
            println!("Description: {}", desc);
        }
        println!();

        let bookmarks = reader.bookmarks()?;
        let mut table = Table::new();
        table.set_header(vec!["File", "Line", "Headline"]);

        for bm in bookmarks {
            let res = reader.resolutions(&bm.id)?;
            let line = res.first().and_then(|r| r.line_range.as_deref()).unwrap_or("?");
            let headline = res.first().and_then(|r| r.headline.as_deref()).unwrap_or("");
            table.add_row(vec![&bm.file_path, line, headline]);
        }
        println!("{table}");
    }
    Ok(())
}

fn resolve_pull_params(
    config: &Config,
    args: &PullArgs,
) -> Result<(String, Option<String>, String)> {
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

        // Find token in config if available, but OVERRIDE with --token flag
        let token = args.token.clone().or_else(|| {
            config
                .codetours
                .servers
                .iter()
                .find(|s| {
                    s.url == server_url
                        || s.url.trim_end_matches('/') == server_url.trim_end_matches('/')
                })
                .and_then(|s| s.token.clone())
        });

        return Ok((server_url, token, tour_id));
    }

    // Bare ID, requires --server
    let server_name = args
        .server
        .as_deref()
        .or(config.codetours.default_server.as_deref())
        .ok_or_else(|| Error::Input("server is required when pull by ID".to_string()))?;

    let (server_url, token) = if server_name.starts_with("http") {
        (server_name.to_string(), args.token.clone())
    } else {
        let s =
            config.codetours.servers.iter().find(|s| s.name == server_name).ok_or_else(|| {
                Error::Input(format!("server '{}' not found in config", server_name))
            })?;
        // Priority: CLI flag > Server config
        (s.url.clone(), args.token.clone().or(s.token.clone()))
    };

    Ok((server_url, token, args.tour.clone()))
}
