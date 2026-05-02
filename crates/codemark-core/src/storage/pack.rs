//! Logic for creating portable SQLite "packs" for sharing collections.

use std::fs::File;
use std::path::{Path, PathBuf};

use rusqlite::params;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::storage::db::Database;

/// Handles surgical export of a collection into a portable SQLite pack.
pub struct Packer<'a> {
    db: &'a Database,
}

impl<'a> Packer<'a> {
    /// Create a new Packer instance.
    pub fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Create a portable SQLite pack for the given collection.
    /// Returns the path to the compressed .zst pack file.
    pub fn create_pack(&self, collection_id: Uuid, output_path: &Path) -> Result<PathBuf> {
        let collection_id_str = collection_id.to_string();

        // 1. Create a temporary SQLite file for the pack
        let temp_dir = std::env::temp_dir();
        let pack_db_path = temp_dir.join(format!("pack-{}.db", Uuid::new_v4()));
        
        // Ensure parent directory exists
        if let Some(parent) = pack_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        {
            // 2. Open the pack database and run migrations
            // We use Database::create to ensure it's a fresh, migrated database
            let pack_db = Database::create(&pack_db_path)?;
            
            // 3. Attach the pack database to the main connection
            // We use a separate connection from the main DB to perform the ATTACH
            let conn = self.db.conn();
            let pack_path_str = pack_db_path.to_str().ok_or_else(|| {
                Error::Operation("Invalid pack database path".to_string())
            })?;
            
            conn.execute(&format!("ATTACH DATABASE '{}' AS pack", pack_path_str), [])?;

            // 4. Surgically copy data using INSERT INTO ... SELECT
            let result = self.copy_collection_data(conn, &collection_id_str);
            
            // 5. Detach the pack database regardless of success
            conn.execute("DETACH DATABASE pack", [])?;
            result?;

            // 6. Add pack metadata
            self.add_pack_meta(&pack_db, "publish")?;

            // 7. Pack Thinning: Omit vec_ tables and run VACUUM
            // Our copy_collection_data doesn't copy vec tables, so they are empty.
            // We can explicitly drop the virtual table if we want to be sure.
            let _ = pack_db.conn().execute("DROP TABLE IF EXISTS bookmark_embeddings", []);
            // Drop FTS tables too to be really "thin"
            let _ = pack_db.conn().execute("DROP TABLE IF EXISTS bookmarks_fts", []);
            
            // Drop FTS triggers to avoid errors when modifying bookmarks in the pack
            let _ = pack_db.conn().execute("DROP TRIGGER IF EXISTS bookmarks_ai", []);
            let _ = pack_db.conn().execute("DROP TRIGGER IF EXISTS bookmarks_ad", []);
            let _ = pack_db.conn().execute("DROP TRIGGER IF EXISTS bookmarks_au", []);

            pack_db.conn().execute("VACUUM", [])?;
        }

        // 8. Compress the pack database with zstd
        let compressed_path = output_path.with_extension("db.zst");
        self.compress_file(&pack_db_path, &compressed_path)?;

        // 9. Cleanup the temporary SQLite file
        std::fs::remove_file(&pack_db_path)?;

        Ok(compressed_path)
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

    /// Copy collection-related data to the attached 'pack' database.
    fn copy_collection_data(&self, conn: &rusqlite::Connection, collection_id: &str) -> Result<()> {
        // Copy collection metadata
        conn.execute(
            "INSERT INTO pack.collections SELECT * FROM main.collections WHERE id = ?1",
            params![collection_id],
        )?;

        // Copy bookmarks in the collection
        conn.execute(
            "INSERT INTO pack.bookmarks
             SELECT * FROM main.bookmarks
             WHERE id IN (SELECT bookmark_id FROM main.collection_bookmarks WHERE collection_id = ?1)",
            params![collection_id],
        )?;

        // Copy collection_bookmarks mapping
        conn.execute(
            "INSERT INTO pack.collection_bookmarks SELECT * FROM main.collection_bookmarks WHERE collection_id = ?1",
            params![collection_id],
        )?;

        // Copy annotations
        conn.execute(
            "INSERT INTO pack.bookmark_annotations
             SELECT * FROM main.bookmark_annotations
             WHERE bookmark_id IN (SELECT bookmark_id FROM main.collection_bookmarks WHERE collection_id = ?1)",
            params![collection_id],
        )?;

        // Copy tags
        conn.execute(
            "INSERT INTO pack.bookmark_tags
             SELECT * FROM main.bookmark_tags
             WHERE bookmark_id IN (SELECT bookmark_id FROM main.collection_bookmarks WHERE collection_id = ?1)",
            params![collection_id],
        )?;

        // Copy comments (added in migration 10)
        conn.execute(
            "INSERT INTO pack.bookmark_comments
             SELECT * FROM main.bookmark_comments
             WHERE bookmark_id IN (SELECT bookmark_id FROM main.collection_bookmarks WHERE collection_id = ?1)",
            params![collection_id],
        )?;

        // Copy repos metadata (added in migration 8)
        conn.execute(
            "INSERT INTO pack.repos SELECT * FROM main.repos",
            [],
        )?;

        // Copy LATEST resolutions per bookmark
        conn.execute(
            "INSERT INTO pack.resolutions
             SELECT r.* FROM main.resolutions r
             INNER JOIN (
                 SELECT bookmark_id, MAX(resolved_at) as latest
                 FROM main.resolutions
                 GROUP BY bookmark_id
             ) latest_res ON r.bookmark_id = latest_res.bookmark_id AND r.resolved_at = latest_res.latest
             INNER JOIN main.collection_bookmarks cb ON r.bookmark_id = cb.bookmark_id
             WHERE cb.collection_id = ?1",
            params![collection_id],
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
    use crate::storage::db::Database;
    use tempfile::tempdir;

    #[test]
    fn test_create_pack() {
        let db = Database::open_in_memory().unwrap();
        let collection_id = Uuid::new_v4();
        let collection_id_str = collection_id.to_string();

        // 1. Seed some data
        db.conn().execute(
            "INSERT INTO collections (id, name, created_at) VALUES (?1, 'test-collection', '2023-01-01T00:00:00Z')",
            params![collection_id_str],
        ).unwrap();

        let bookmark_id = Uuid::new_v4().to_string();
        db.conn().execute(
            "INSERT INTO bookmarks (id, query, language, file_path, created_at) VALUES (?1, 'query', 'rust', 'src/main.rs', '2023-01-01T00:00:00Z')",
            params![bookmark_id],
        ).unwrap();

        db.conn().execute(
            "INSERT INTO collection_bookmarks (collection_id, bookmark_id, added_at) VALUES (?1, ?2, '2023-01-01T00:00:00Z')",
            params![collection_id_str, bookmark_id],
        ).unwrap();

        db.conn().execute(
            "INSERT INTO resolutions (id, bookmark_id, resolved_at, method) VALUES (?1, ?2, '2023-01-01T00:00:00Z', 'exact')",
            params![Uuid::new_v4().to_string(), bookmark_id],
        ).unwrap();

        db.conn().execute(
            "INSERT INTO bookmark_annotations (id, bookmark_id, notes, added_at) VALUES (?1, ?2, 'test note', '2023-01-01T00:00:00Z')",
            params![Uuid::new_v4().to_string(), bookmark_id],
        ).unwrap();

        db.conn().execute(
            "INSERT INTO repos (id, repo_owner, repo_name, repo_root, db_owner_email, detected_at) VALUES (?1, 'owner', 'repo', '/root', 'owner@example.com', '2023-01-01T00:00:00Z')",
            params![Uuid::new_v4().to_string()],
        ).unwrap();

        // 2. Create pack
        let temp_dir = tempdir().unwrap();
        let pack_path = temp_dir.path().join("test-pack");
        let packer = Packer::new(&db);
        let result_path = packer.create_pack(collection_id, &pack_path).unwrap();

        assert!(result_path.exists());
        assert_eq!(result_path.extension().unwrap(), "zst");

        // 3. Decompress and verify
        let decompressed_db_path = temp_dir.path().join("decompressed.db");
        {
            let compressed_file = File::open(&result_path).unwrap();
            let mut decoder = zstd::stream::read::Decoder::new(compressed_file).unwrap();
            let mut decompressed_file = File::create(&decompressed_db_path).unwrap();
            std::io::copy(&mut decoder, &mut decompressed_file).unwrap();
        }

        // Use raw rusqlite connection to verify WITHOUT running migrations/init_embeddings
        let pack_conn = rusqlite::Connection::open(&decompressed_db_path).unwrap();
        
        let count: i64 = pack_conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE id = ?1",
            params![collection_id_str],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 1);

        let bookmark_count: i64 = pack_conn.query_row(
            "SELECT COUNT(*) FROM bookmarks WHERE id = ?1",
            params![bookmark_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(bookmark_count, 1);

        let annotation_count: i64 = pack_conn.query_row(
            "SELECT COUNT(*) FROM bookmark_annotations WHERE bookmark_id = ?1",
            params![bookmark_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(annotation_count, 1);

        let repo_count: i64 = pack_conn.query_row(
            "SELECT COUNT(*) FROM repos",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(repo_count, 1);

        // Verify _pack_meta
        let meta_count: i64 = pack_conn.query_row(
            "SELECT COUNT(*) FROM _pack_meta",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(meta_count, 1);

        let purpose: String = pack_conn.query_row(
            "SELECT purpose FROM _pack_meta",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(purpose, "publish");

        // Verify thinning
        let tables: Vec<String> = pack_conn.prepare("SELECT name FROM sqlite_master WHERE type='table'").unwrap()
            .query_map([], |row| row.get(0)).unwrap()
            .filter_map(|r| r.ok()).collect();
        
        assert!(!tables.contains(&"bookmark_embeddings".to_string()), "Found bookmark_embeddings in tables: {:?}", tables);
        assert!(!tables.contains(&"bookmarks_fts".to_string()), "Found bookmarks_fts in tables: {:?}", tables);
        
        // Verify no shadow tables
        for table in &tables {
            assert!(!table.starts_with("bookmark_embeddings_"), "Found shadow table: {}", table);
            assert!(!table.starts_with("bookmarks_fts_"), "Found shadow table: {}", table);
        }
    }
}
