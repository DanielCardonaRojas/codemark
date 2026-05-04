use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use futures_util::StreamExt;
use serde::Serialize;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::observability::RequestId;
use crate::pack::{PackError, inspect};
use crate::pack_cache::PackCache;
use crate::router::AppState;

/// Successful response body for tour creation.
#[derive(Debug, Serialize)]
pub struct CreateTourResponse {
    /// Unique identifier for the created tour.
    pub tour_id: String,
    /// Relative API URL for the tour.
    pub url: String,
    /// Status of the tour (e.g., 'ready').
    pub status: String,
    /// Visibility of the tour (public/private).
    pub visibility: String,
    /// ISO 8601 timestamp of publication.
    pub published_at: String,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
    /// ISO 8601 timestamp of last update.
    pub updated_at: String,
}

/// Error response body for the tours API.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Short machine-readable error code.
    pub error: String,
    /// Human-readable explanation of the error.
    pub reason: Option<String>,
    /// Request ID for tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Handler for POST /tours. Uploads a SQLite pack and merges it into the database.
pub async fn handler(
    State(state): State<AppState>,
    auth: AuthContext,
    req: Request,
) -> impl IntoResponse {
    if !auth.is_authenticated() {
        return (StatusCode::UNAUTHORIZED, "Auth required").into_response();
    }
    let request_id = req
        .extensions()
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // 1. Buffer upload to temp file
    let temp_dir = std::env::temp_dir().join("codetours").join("incoming");
    if let Err(e) = fs::create_dir_all(&temp_dir).await {
        tracing::error!("Failed to create temp dir: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal".to_string(),
                reason: Some("Failed to create temp directory".to_string()),
                request_id: Some(request_id),
            }),
        )
            .into_response();
    }

    let temp_path = temp_dir.join(format!("{}.sqlite", request_id));
    let mut file = match File::create(&temp_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to create temp file: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal".to_string(),
                    reason: Some("Failed to create temporary file".to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
    };

    let mut body = req.into_body().into_data_stream();
    let mut bytes_written = 0;
    let max_size = state.config.max_pack_size;

    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to read body chunk: {}", e);
                let _ = fs::remove_file(&temp_path).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "internal".to_string(),
                        reason: Some("Failed to read upload body".to_string()),
                        request_id: Some(request_id),
                    }),
                )
                    .into_response();
            }
        };

        bytes_written += chunk.len();
        if bytes_written > max_size {
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(ErrorResponse {
                    error: "payload_too_large".to_string(),
                    reason: Some(format!("Upload exceeds {} byte limit", max_size)),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }

        if let Err(e) = file.write_all(&chunk).await {
            tracing::error!("Failed to write to temp file: {}", e);
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal".to_string(),
                    reason: Some("Failed to write to temporary file".to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
    }

    if let Err(e) = file.flush().await {
        tracing::error!("Failed to flush temp file: {}", e);
        let _ = fs::remove_file(&temp_path).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal".to_string(),
                reason: Some("Failed to flush temporary file".to_string()),
                request_id: Some(request_id),
            }),
        )
            .into_response();
    }
    // Explicitly drop file handle to allow rename/move on all platforms
    drop(file);

    // 4. Check for zstd compression and decompress if needed
    let is_zstd = {
        let f = fs::File::open(&temp_path).await;
        match f {
            Ok(mut f) => {
                let mut magic = [0u8; 4];
                use tokio::io::AsyncReadExt;
                f.read_exact(&mut magic).await.is_ok() && magic == [0x28, 0xB5, 0x2F, 0xFD]
            }
            Err(e) => {
                tracing::error!("Failed to open temp file for magic check: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        }
    };

    if is_zstd {
        let decompressed_path = temp_path.with_extension("decompressed");
        let temp_path_blocking = temp_path.clone();
        let decompressed_path_blocking = decompressed_path.clone();
        let max_size_blocking = max_size;

        let res = tokio::task::spawn_blocking(move || {
            let compressed_file = std::fs::File::open(&temp_path_blocking)?;
            let mut decoder = zstd::stream::read::Decoder::new(compressed_file)?;
            let mut decompressed_file = std::fs::File::create(&decompressed_path_blocking)?;

            // Limit decompression to max_pack_size to prevent decompression bombs
            use std::io::Read;
            let mut limited = (&mut decoder).take(max_size_blocking as u64);
            let written = std::io::copy(&mut limited, &mut decompressed_file)?;

            // If we reached the limit, check if there's more data
            if written >= max_size_blocking as u64 {
                let mut buf = [0u8; 1];
                if decoder.read(&mut buf)? > 0 {
                    return Err(std::io::Error::other("Decompressed size exceeds limit"));
                }
            }

            Ok::<_, std::io::Error>(())
        })
        .await;

        let res = match res {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Blocking task panicked during decompression: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if let Err(e) = res {
            tracing::error!("Failed to decompress pack: {}", e);
            let _ = fs::remove_file(&temp_path).await;
            let _ = fs::remove_file(&decompressed_path).await;
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ErrorResponse {
                    error: "invalid_pack".to_string(),
                    reason: Some("Failed to decompress zstd pack".to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
        let _ = fs::remove_file(&temp_path).await;
        if let Err(e) = fs::rename(&decompressed_path, &temp_path).await {
            tracing::error!("Failed to rename decompressed pack: {}", e);
            let _ = fs::remove_file(&decompressed_path).await;
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // 3. Run pack pre-inspector (safety check)
    let user_version = match crate::pack::pre_inspect(&temp_path) {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&temp_path).await;
            let status = match e {
                PackError::NotSqlite => StatusCode::BAD_REQUEST,
                _ => StatusCode::UNPROCESSABLE_ENTITY,
            };
            return (status, Json(e)).into_response();
        }
    };

    // 4. Handle migrations
    if user_version < crate::pack::CURRENT_SERVER_VERSION {
        let temp_path_blocking = temp_path.clone();
        let res = tokio::task::spawn_blocking(move || {
            let mut conn = rusqlite::Connection::open(&temp_path_blocking)?;
            crate::pack::migrate_pack_forward(&mut conn)?;
            Ok::<_, anyhow::Error>(())
        })
        .await;

        let res = match res {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Blocking task panicked during migration: {}", e);
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        if let Err(e) = res {
            tracing::error!("Failed to migrate pack forward: {}", e);
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "migration_failed".to_string(),
                    reason: Some(e.to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
    } else if user_version > crate::pack::CURRENT_SERVER_VERSION {
        let _ = fs::remove_file(&temp_path).await;
        return (
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "server_out_of_date".to_string(),
                reason: Some(format!(
                    "Server version is {}, pack version is {}",
                    crate::pack::CURRENT_SERVER_VERSION,
                    user_version
                )),
                request_id: Some(request_id),
            }),
        )
            .into_response();
    }

    // 5. Run full pack inspector (schema check)
    let _pack_info = match inspect(&temp_path) {
        Ok(info) => info,
        Err(e) => {
            let _ = fs::remove_file(&temp_path).await;
            return (StatusCode::UNPROCESSABLE_ENTITY, Json(e)).into_response();
        }
    };

    // 6. Merge into single tenant DB
    let storage = state.storage.clone();
    let temp_path_clone = temp_path.clone();

    let conn = match storage.get_conn().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to acquire DB connection: {}", e);
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "internal".to_string(),
                    reason: Some("Database pool exhausted".to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
    };

    let result = conn
        .interact(move |conn| {
            // ATTACH the pack BEFORE transaction
            let pack_path_str = temp_path_clone
                .to_str()
                .ok_or_else(|| rusqlite::Error::InvalidPath(temp_path_clone.clone()))?;

            // Security: Escape single quotes to prevent SQL injection in ATTACH
            let escaped_path = pack_path_str.replace('\'', "''");
            conn.execute(&format!("ATTACH DATABASE '{}' AS pack", escaped_path), [])?;

            let res = (|| -> rusqlite::Result<(CreateTourResponse, bool)> {
            // Get the collection ID from the pack
            let collection_id: String = conn.query_row(
                "SELECT id FROM pack.collections WHERE visibility IS NOT NULL ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0)
            )?;

            let tx = conn.transaction()?;

            // 1. Republish cleanup if it exists
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM main.collections WHERE id = ?1)",
                [&collection_id],
                |row| row.get(0)
            )?;

            if exists {
                tx.execute("CREATE TEMP TABLE IF NOT EXISTS _republish_orphan_candidates AS SELECT bookmark_id FROM main.collection_bookmarks WHERE collection_id = ?1", [&collection_id])?;
                tx.execute("DELETE FROM main.collection_bookmarks WHERE collection_id = ?1", [&collection_id])?;
                tx.execute("DELETE FROM main.collections WHERE id = ?1", [&collection_id])?;
                tx.execute("DELETE FROM main.bookmarks WHERE id IN (SELECT bookmark_id FROM _republish_orphan_candidates) AND id NOT IN (SELECT bookmark_id FROM main.collection_bookmarks)", [])?;
                tx.execute("DROP TABLE IF EXISTS _republish_orphan_candidates", [])?;
            }

            // 2. Merge SQL
            // Use explicit columns to be safe against schema drift
            tx.execute("INSERT INTO main.bookmarks (id, query, language, file_path, content_hash, commit_hash, status, resolution_method, last_resolved_at, stale_since, created_at, created_by)
                        SELECT id, query, language, file_path, content_hash, commit_hash, status, resolution_method, last_resolved_at, stale_since, created_at, created_by
                        FROM pack.bookmarks WHERE id NOT IN (SELECT id FROM main.bookmarks)", [])?;

            tx.execute(
                "INSERT INTO main.collections (
                    id, name, description, visibility,
                    repo_url, created_branch, published_commit_sha,
                    status, published_at, created_at, updated_at
                )
                SELECT
                    p.id, p.name, p.description, p.visibility,
                    p.repo_url, p.created_branch, p.published_commit_sha,
                    'ready', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                    COALESCE(p.created_at, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                FROM pack.collections AS p
                WHERE p.visibility IS NOT NULL",
                []
            )?;

            tx.execute("INSERT INTO main.collection_bookmarks (collection_id, bookmark_id, position, added_at)
                        SELECT collection_id, bookmark_id, position, added_at FROM pack.collection_bookmarks", [])?;

            tx.execute("INSERT INTO main.resolutions (id, bookmark_id, resolved_at, commit_hash, method, match_count, file_path, byte_range, line_range, content_hash, headline, preview_lines)
                        SELECT id, bookmark_id, resolved_at, commit_hash, method, match_count, file_path, byte_range, line_range, content_hash, headline, preview_lines
                        FROM pack.resolutions WHERE id NOT IN (SELECT id FROM main.resolutions)", [])?;

            tx.execute("INSERT INTO main.bookmark_annotations (id, bookmark_id, added_at, added_by, notes, context, source)
                        SELECT id, bookmark_id, added_at, added_by, notes, context, source
                        FROM pack.bookmark_annotations WHERE id NOT IN (SELECT id FROM main.bookmark_annotations)", [])?;

            tx.execute("INSERT OR IGNORE INTO main.bookmark_tags (bookmark_id, tag, added_at, added_by)
                        SELECT bookmark_id, tag, added_at, added_by FROM pack.bookmark_tags", [])?;

            // Optional bookmark_comments
            let has_comments: bool = tx.query_row(
                "SELECT count(*) FROM pack.sqlite_master WHERE type='table' AND name='bookmark_comments'",
                [],
                |row| Ok(row.get::<_, i64>(0)? > 0)
            )?;
            if has_comments {
                tx.execute("INSERT INTO main.bookmark_comments (id, bookmark_id, author, body, created_at, parent_id)
                            SELECT id, bookmark_id, author, body, created_at, parent_id
                            FROM pack.bookmark_comments WHERE id NOT IN (SELECT id FROM main.bookmark_comments)", [])?;
            }

            // Get the final collection row to return
            let response: CreateTourResponse = tx.query_row(
                "SELECT id, visibility, published_at, created_at, updated_at FROM main.collections WHERE id = ?1",
                [&collection_id],
                |row| {
                    let id: String = row.get(0)?;
                    Ok(CreateTourResponse {
                        tour_id: id.clone(),
                        url: format!("/tours/{}", id),
                        status: "ready".to_string(),
                        visibility: row.get(1)?,
                        published_at: row.get(2)?,
                        created_at: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                }
            )?;

            tx.commit()?;

            Ok((response, exists))
        })();

        let _ = conn.execute("DETACH DATABASE pack", []);
        res
    }).await;

    let (response, is_republish) = match result {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => {
            tracing::error!("Database error during merge: {}", e);
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal".to_string(),
                    reason: Some(e.to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("Interaction error during merge: {}", e);
            let _ = fs::remove_file(&temp_path).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "internal".to_string(),
                    reason: Some(e.to_string()),
                    request_id: Some(request_id),
                }),
            )
                .into_response();
        }
    };

    // 7. Save to pack cache
    let cache = PackCache::new(state.config.data_dir.clone());
    if let Err(e) = cache.save_pack(&response.tour_id, &temp_path).await {
        tracing::error!("Failed to save pack to cache: {}. Performing compensating deletion.", e);

        // Compensating deletion in DB to maintain consistency (Full cleanup including orphans)
        let storage = state.storage.clone();
        let tour_id = response.tour_id.clone();
        if let Ok(conn) = storage.get_conn().await {
            let _ = conn
                .interact(move |conn| {
                    let tx = conn.transaction()?;
                    tx.execute("CREATE TEMP TABLE IF NOT EXISTS _comp_delete_candidates AS SELECT bookmark_id FROM main.collection_bookmarks WHERE collection_id = ?1", [&tour_id])?;
                    tx.execute("DELETE FROM main.collection_bookmarks WHERE collection_id = ?1", [&tour_id])?;
                    tx.execute("DELETE FROM main.collections WHERE id = ?1", [&tour_id])?;
                    tx.execute("DELETE FROM main.bookmarks WHERE id IN (SELECT bookmark_id FROM _comp_delete_candidates) AND id NOT IN (SELECT bookmark_id FROM main.collection_bookmarks)", [])?;
                    tx.execute("DROP TABLE IF EXISTS _comp_delete_candidates", [])?;
                    tx.commit()?;
                    Ok::<_, rusqlite::Error>(())
                })
                .await;
        }

        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal".to_string(),
                reason: Some("Failed to persist pack artifact".to_string()),
                request_id: Some(request_id),
            }),
        )
            .into_response();
    }

    let status = if is_republish { StatusCode::OK } else { StatusCode::CREATED };

    (status, Json(response)).into_response()
}
