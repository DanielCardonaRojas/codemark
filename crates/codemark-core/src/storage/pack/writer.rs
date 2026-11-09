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
            // Embeddings are local, regenerable, and their vec0 shadow tables are
            // not in the pack schema allowlist — drop them (this cascades to the
            // vec0 shadow tables) so packs stay lean and pass schema validation.
            let _ = conn.execute("DROP TABLE IF EXISTS bookmark_embeddings", []);
            let _ = conn.execute("DROP TABLE IF EXISTS collection_embeddings", []);
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
            "INSERT INTO collections (id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, status, health, health_computed_at, updated_at, imported_from_url, prompt) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                c.imported_from_url,
                c.prompt
            ],
        )?;

        // 2. Insert bookmarks
        for bm in &payload.bookmarks {
            tracing::debug!(target: "codemark::db", id = %bm.id, file = %bm.file_path, "exporting bookmark to pack");
            // repo_id is intentionally omitted: packs don't include repos rows,
            // so persisting it would create a dangling foreign key on import.
            conn.execute(
                "INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
                 created_at, created_by, current_resolution_id, grammar_source, forked_from, branch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
                    // Carried as authored, not translated to the importer's
                    // setup: it records which grammar wrote the query, which is
                    // what makes a mismatch on the importing machine visible
                    // rather than silently re-labelled as a local grammar.
                    bm.grammar_source.map(|g| g.as_str()),
                    // Fork lineage travels with the pack for inspection; the
                    // importer drops it (see `sync::import_pack`).
                    bm.forked_from,
                    bm.branch,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{
        Bookmark, BookmarkHealth, Collection, ResolutionMethod, Visibility,
    };
    use crate::parser::languages::GrammarIdentity;
    use crate::storage::pack::PackReader;

    fn bookmark(id: &str, grammar_source: Option<GrammarIdentity>) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: format!("(function_declaration) @target /* {id} */"),
            language: "swift".to_string(),
            grammar_source,
            forked_from: None,
            branch: None,
            file_path: format!("src/{id}.swift"),
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
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        }
    }

    /// A one-collection snapshot wrapping `bookmarks`.
    fn payload_with(bookmarks: Vec<Bookmark>) -> crate::engine::snapshot::SnapshotPayload {
        crate::engine::snapshot::SnapshotPayload {
            collection: Collection {
                id: "c1".to_string(),
                name: "Tour".to_string(),
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
                prompt: None,
            },
            bookmarks,
            resolutions: vec![],
            tags: vec![],
            comments: vec![],
            collection_tags: vec![],
            collection_links: vec![],
        }
    }

    /// A shared pack carries the grammar identity its author recorded.
    ///
    /// The identity is what resolution uses to pick the grammar that generated a
    /// query (#244). Dropping it on export would silently downgrade every
    /// imported bookmark to name-only resolution — the exact bug — while the
    /// pack otherwise looked complete. It is deliberately carried *as authored*
    /// rather than translated to the importer's setup, so a grammar mismatch on
    /// the importing machine stays visible.
    #[test]
    fn a_pack_preserves_the_authors_grammar_identity() {
        let payload = payload_with(vec![
            bookmark("b-builtin", Some(GrammarIdentity::Builtin)),
            bookmark("b-dynamic", Some(GrammarIdentity::Dynamic)),
            // Written by a codemark predating the column: stays unknown rather
            // than being guessed at on export.
            bookmark("b-unknown", None),
        ]);

        let pack_db = std::env::temp_dir()
            .join(format!("codemark-identity-pack-{}.sqlite", uuid::Uuid::new_v4()));
        {
            let db = Database::create(&pack_db).unwrap();
            let packer = Packer::new(&db);
            packer.insert_snapshot_data(db.conn(), &payload).unwrap();
        }

        let reader = PackReader::open(&pack_db).unwrap();
        let round_tripped: std::collections::HashMap<_, _> = reader
            .bookmarks()
            .unwrap()
            .into_iter()
            .map(|b| (b.id.clone(), b.grammar_source))
            .collect();

        assert_eq!(round_tripped.get("b-builtin"), Some(&Some(GrammarIdentity::Builtin)));
        assert_eq!(round_tripped.get("b-dynamic"), Some(&Some(GrammarIdentity::Dynamic)));
        assert_eq!(round_tripped.get("b-unknown"), Some(&None));

        let _ = std::fs::remove_file(&pack_db);
    }

    /// A pack written before the column existed still reads back once migrated,
    /// which is what lets `reader.rs` select `grammar_source` unconditionally.
    ///
    /// This covers the reader in isolation: it asserts the read fails before the
    /// migration and succeeds after. The *pull path* actually running them in
    /// that order is pinned separately by
    /// `sync::tests::pull_path_imports_a_pre_v25_pack`, which drives the real
    /// sequence rather than restating it here.
    #[test]
    fn a_pre_v25_pack_reads_back_after_migration() {
        let pack_db =
            std::env::temp_dir().join(format!("codemark-old-pack-{}.sqlite", uuid::Uuid::new_v4()));

        {
            let db = Database::create(&pack_db).unwrap();
            let packer = Packer::new(&db);
            packer
                .insert_snapshot_data(db.conn(), &payload_with(vec![bookmark("b-old", None)]))
                .unwrap();
            // Drop the column to recreate a genuinely pre-v25 pack, and rewind
            // the version so the importer migrates it forward again.
            db.conn()
                .execute_batch(
                    "ALTER TABLE bookmarks DROP COLUMN grammar_source; PRAGMA user_version = 24;",
                )
                .unwrap();
        }

        // Selecting the column now fails, which is what an old pack looks like.
        assert!(PackReader::open(&pack_db).unwrap().bookmarks().is_err());

        // The importer migrates first; after that the read succeeds.
        let mut conn = rusqlite::Connection::open(&pack_db).unwrap();
        Database::run_migrations_on(&mut conn).unwrap();
        drop(conn);

        let bookmarks = PackReader::open(&pack_db).unwrap().bookmarks().unwrap();
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].grammar_source, None, "migrated rows stay unknown");

        let _ = std::fs::remove_file(&pack_db);
    }
}
