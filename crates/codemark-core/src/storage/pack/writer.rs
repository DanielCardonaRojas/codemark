//! Logic for creating portable SQLite "packs" for sharing collections.

use std::fs::File;
use std::path::{Path, PathBuf};

use rusqlite::params;
use uuid::Uuid;

use crate::engine::snapshot::SnapshotPayload;
use crate::error::Result;
use crate::storage::db::Database;

/// Handles surgical export of a collection into a portable SQLite pack.
pub struct Packer {}

impl Packer {
    /// Create a new Packer instance.
    pub fn new(_db: &Database) -> Self {
        Self {}
    }

    /// Create a portable SQLite pack from a SnapshotPayload.
    /// Returns the path to the compressed .zst pack file.
    pub fn create_pack_from_snapshot(
        &self,
        payload: &SnapshotPayload,
        output_path: &Path,
    ) -> Result<PathBuf> {
        // 1. Create a temporary SQLite file for the pack
        let temp_dir = std::env::temp_dir();
        let pack_db_path = temp_dir.join(format!("pack-{}.sqlite", Uuid::new_v4()));

        // Ensure parent directory exists
        if let Some(parent) = pack_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        {
            // 2. Open the pack database and run migrations
            let pack_db = Database::create(&pack_db_path)?;
            let conn = pack_db.conn();

            // 3. Insert data from payload
            self.insert_snapshot_data(conn, payload)?;

            // 4. Add pack metadata
            self.add_pack_meta(&pack_db, "publish")?;

            // 5. Thinning and VACUUM
            let _ = conn.execute("DROP TABLE IF EXISTS bookmark_embeddings", []);
            let _ = conn.execute("DROP TABLE IF EXISTS bookmarks_fts", []);
            let _ = conn.execute("DROP TABLE IF EXISTS published_tours", []);
            let _ = conn.execute("DROP TRIGGER IF EXISTS bookmarks_ai", []);
            let _ = conn.execute("DROP TRIGGER IF EXISTS bookmarks_ad", []);
            let _ = conn.execute("DROP TRIGGER IF EXISTS bookmarks_au", []);

            conn.execute("VACUUM", [])?;
        }

        // 6. Compress the pack database with zstd
        let compressed_path = output_path.with_extension("sqlite.zst");
        self.compress_file(&pack_db_path, &compressed_path)?;

        // 7. Cleanup the temporary SQLite file
        std::fs::remove_file(&pack_db_path)?;

        Ok(compressed_path)
    }

    fn insert_snapshot_data(
        &self,
        conn: &rusqlite::Connection,
        payload: &SnapshotPayload,
    ) -> Result<()> {
        conn.execute_batch("PRAGMA foreign_keys = OFF;")?;

        // 1. Insert collection
        let c = &payload.collection;
        conn.execute(
            "INSERT INTO collections (id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, status, health, health_computed_at, updated_at, imported_from_url) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                c.id,
                c.name,
                c.description,
                c.visibility.to_string(),
                c.created_at,
                c.created_by,
                c.created_branch,
                c.published_at,
                c.published_commit_sha,
                c.repo_url,
                c.status,
                c.health.as_ref().map(|h| h.to_string()),
                c.health_computed_at,
                c.updated_at,
                c.imported_from_url
            ],
        )?;

        // 2. Insert bookmarks
        for bm in &payload.bookmarks {
            conn.execute(
                "INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
                 created_at, created_by, current_resolution_id, repo_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    bm.id,
                    bm.query,
                    bm.language,
                    bm.file_path,
                    bm.content_hash,
                    bm.commit_hash,
                    bm.created_at,
                    bm.created_by,
                    bm.current_resolution_id,
                    bm.repo_id,
                ],
            )?;

            // Insert membership
            conn.execute(
                "INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at) VALUES (?1, ?2, ?3)",
                params![c.id, bm.id, chrono::Utc::now().to_rfc3339()],
            )?;

            // Insert annotations
            for ann in &bm.annotations {
                conn.execute(
                    "INSERT INTO bookmark_annotations (id, bookmark_id, added_at, added_by, notes, context, source)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![ann.id, ann.bookmark_id, ann.added_at, ann.added_by, ann.notes, ann.context, ann.source],
                )?;
            }
        }

        // 3. Insert tags
        for t in &payload.tags {
            conn.execute(
                "INSERT INTO bookmark_tags (bookmark_id, tag, added_at, added_by) VALUES (?1, ?2, ?3, ?4)",
                params![t.bookmark_id, t.tag, t.added_at, t.added_by],
            )?;
        }

        // 4. Insert comments
        for com in &payload.comments {
            conn.execute(
                "INSERT INTO bookmark_comments (id, bookmark_id, author, body, created_at, parent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![com.id, com.bookmark_id, com.author, com.body, com.created_at, com.parent_id],
            )?;
        }

        // 5. Insert resolutions
        for res in &payload.resolutions {
            conn.execute(
                "INSERT INTO resolutions (id, bookmark_id, resolved_at, health, commit_hash, method, match_count, file_path, byte_range, line_range, content_hash, headline, snapshot, breadcrumbs, is_dirty)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    res.id,
                    res.bookmark_id,
                    res.resolved_at,
                    res.health.to_string(),
                    res.commit_hash,
                    res.method.to_string(),
                    res.match_count,
                    res.file_path,
                    res.byte_range,
                    res.line_range,
                    res.content_hash,
                    res.headline,
                    res.snapshot,
                    res.breadcrumbs,
                    res.is_dirty
                ],
            )?;
        }

        // 6. Insert collection tags
        for t in &payload.collection_tags {
            conn.execute(
                "INSERT OR IGNORE INTO collection_tags (collection_id, tag, added_at, added_by) VALUES (?1, ?2, ?3, ?4)",
                params![t.collection_id, t.tag, t.added_at, t.added_by],
            )?;
        }

        // 7. Insert collection links
        for l in &payload.collection_links {
            conn.execute(
                "INSERT OR IGNORE INTO collection_links (id, collection_id, kind, label, url, sort_order, added_at, added_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![l.id, l.collection_id, l.kind.to_string(), l.label, l.url, l.sort_order, l.added_at, l.added_by],
            )?;
        }

        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(())
    }

    /// Add _pack_meta table and a single row describing this pack.
    fn add_pack_meta(&self, pack_db: &Database, purpose: &str) -> Result<()> {
        let conn = pack_db.conn();

        conn.execute(
            "CREATE TABLE IF NOT EXISTS _pack_meta (
                pack_id TEXT PRIMARY KEY,
                protocol_version INTEGER NOT NULL,
                purpose TEXT NOT NULL CHECK (purpose IN ('publish', 'mirror', 'export')),
                source_client TEXT NOT NULL,
                generated_at TEXT NOT NULL,
                notes TEXT
            )",
            [],
        )?;

        let pack_id = Uuid::new_v4().to_string();
        let protocol_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let source_client = format!("codemark@{}", env!("CARGO_PKG_VERSION"));
        let generated_at = chrono::Utc::now().to_rfc3339();

        conn.execute(
            "INSERT INTO _pack_meta (pack_id, protocol_version, purpose, source_client, generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![pack_id, protocol_version, purpose, source_client, generated_at],
        )?;

        Ok(())
    }

    /// Compress a file using zstd.
    fn compress_file(&self, source: &Path, destination: &Path) -> Result<()> {
        let mut source_file = File::open(source)?;
        let destination_file = File::create(destination)?;

        let mut encoder = zstd::stream::write::Encoder::new(destination_file, 0)?;
        std::io::copy(&mut source_file, &mut encoder)?;
        encoder.finish()?;

        Ok(())
    }
}
