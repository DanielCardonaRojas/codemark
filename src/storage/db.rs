//! Database connection and migrations.

#![allow(dead_code)]

use std::path::Path;

use rusqlite::Connection;

use crate::error::Result;

const MIGRATION_001: &str = include_str!("../../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_add_fts.sql");
const MIGRATION_003: &str = include_str!("../../migrations/003_collection_ordering.sql");
const MIGRATION_004: &str = include_str!("../../migrations/004_add_line_range.sql");
const MIGRATION_005: &str = include_str!("../../migrations/005_add_embeddings.sql");
const MIGRATION_006: &str = include_str!("../../migrations/006_add_fts_path_tags.sql");
const MIGRATION_007: &str = include_str!("../../migrations/007_append_only_metadata.sql");

/// SQLite database wrapper with automatic migrations.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the database at the given path, run migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            // Initialize default config file if it doesn't exist
            let _ = crate::config::Config::init_default(parent);
        }
        let conn = Connection::open(path)?;
        let mut db = Database { conn };
        db.init()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = Database { conn };
        db.init()?;
        Ok(db)
    }

    /// Get a reference to the underlying connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Get a mutable reference to the underlying connection.
    /// Use with caution - only for operations that require mutability.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    fn init(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;
        self.run_migrations()?;
        Ok(())
    }

    fn run_migrations(&mut self) -> Result<()> {
        let current_version = self.schema_version();

        if current_version < 1 {
            self.conn.execute_batch(MIGRATION_001)?;
            self.set_schema_version(1)?;
        }

        if current_version < 2 {
            self.conn.execute_batch(MIGRATION_002)?;
            self.set_schema_version(2)?;
        }

        if current_version < 3 {
            self.conn.execute_batch(MIGRATION_003)?;
            self.set_schema_version(3)?;
        }

        if current_version < 4 {
            self.conn.execute_batch(MIGRATION_004)?;
            self.set_schema_version(4)?;
        }

        if current_version < 5 {
            self.conn.execute_batch(MIGRATION_005)?;
            self.set_schema_version(5)?;
            // Initialize sqlite-vec extension and create embeddings table
            self.init_embeddings_table()?;
        } else if current_version >= 5 {
            // Ensure embeddings table exists even if migration was already run
            self.ensure_embeddings_table()?;
        }

        if current_version < 6 {
            self.conn.execute_batch(MIGRATION_006)?;
            self.set_schema_version(6)?;
        }

        if current_version < 7 {
            self.migrate_to_v7()?;
            self.set_schema_version(7)?;
        }

        Ok(())
    }

    /// Migrate to schema version 7: append-only metadata model
    /// This handles migrating existing data from the old schema to the new one.
    fn migrate_to_v7(&mut self) -> Result<()> {
        // First run the base migration which creates new tables and recreates bookmarks
        self.conn.execute_batch(MIGRATION_007)?;

        // Now migrate existing data if we're coming from schema 6
        // Check if the old bookmarks table had data (we can tell by checking if annotations are empty)
        // Actually, the SQL migration should have already copied the bookmarks table data
        // We just need to migrate the annotations and tags

        // Try to migrate annotations from old schema
        // This will fail if the old columns don't exist (fresh install), so we ignore errors
        let migrate_annotations = || -> Result<()> {
            self.conn.execute(
                "INSERT INTO bookmark_annotations (id, bookmark_id, added_at, added_by, notes, context, source)
                 SELECT lower(hex(randomblob(16))), id, created_at, created_by, notes, context, 'migration'
                 FROM (SELECT id, created_at, created_by, notes, context FROM bookmarks WHERE notes IS NOT NULL OR context IS NOT NULL LIMIT 1)
                 WHERE notes IS NOT NULL OR context IS NOT NULL",
                [],
            )?;
            Ok(())
        };

        // Try to migrate tags from old schema
        let migrate_tags = || -> Result<()> {
            self.conn.execute(
                "INSERT INTO bookmark_tags (bookmark_id, tag, added_at, added_by)
                 SELECT bm.id, TRIM(json_each.value), bm.created_at, bm.created_by
                 FROM bookmarks bm, json_each(bm.tags)
                 WHERE json_valid(bm.tags) AND TRIM(json_each.value) != ''",
                [],
            )?;
            Ok(())
        };

        // Ignore migration errors - they just mean we're on a fresh install
        let _ = migrate_annotations();
        let _ = migrate_tags();

        Ok(())
    }

    /// Initialize the sqlite-vec extension and create the embeddings table.
    /// This is called during migration 005.
    fn init_embeddings_table(&mut self) -> Result<()> {
        use crate::embeddings::VecStore;

        // Initialize the sqlite-vec extension once
        VecStore::init_extension();

        // Create the vec0 virtual table with 384 dimensions (all-MiniLM-L6-v2)
        let store = VecStore::new(384);
        store.create_table(&mut self.conn).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to create embeddings table: {}", e))
        })?;

        Ok(())
    }

    /// Ensure the embeddings table exists, create it if it doesn't.
    /// This handles the case where the schema version is 5 but the table wasn't created.
    fn ensure_embeddings_table(&mut self) -> Result<()> {
        use crate::embeddings::VecStore;

        // First, ensure the extension is loaded
        // This is safe to call multiple times
        VecStore::init_extension();

        // Check if table exists by trying to query it
        // vec0 virtual tables don't show up in sqlite_master like regular tables
        let exists = self
            .conn
            .query_row("SELECT COUNT(*) FROM bookmark_embeddings LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .is_ok();

        if !exists {
            // Create the vec0 virtual table with 384 dimensions (all-MiniLM-L6-v2)
            let store = VecStore::new(384);
            store.create_table(&mut self.conn).map_err(|e| {
                crate::error::Error::Operation(format!("Failed to create embeddings table: {}", e))
            })?;
        }

        Ok(())
    }

    fn schema_version(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                [],
                |row: &rusqlite::Row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|v: String| v.parse().ok())
            .unwrap_or(0)
    }

    fn set_schema_version(&mut self, version: i64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
            [version.to_string()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Initialize sqlite-vec extension for all tests
    // This must be called before any database operations
    fn init_test_env() {
        crate::embeddings::VecStore::init_extension();
    }

    #[test]
    fn open_in_memory_succeeds() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let version = db.schema_version();
        assert_eq!(version, 7);
    }

    #[test]
    fn migration_is_idempotent() {
        init_test_env();
        let mut db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        assert_eq!(db.schema_version(), 7);
    }

    #[test]
    fn tables_exist_after_migration() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let tables: Vec<String> = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(tables.contains(&"bookmarks".to_string()));
        assert!(tables.contains(&"resolutions".to_string()));
        assert!(tables.contains(&"collections".to_string()));
        assert!(tables.contains(&"collection_bookmarks".to_string()));
        assert!(tables.contains(&"schema_meta".to_string()));
    }

    #[test]
    fn foreign_keys_enabled() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let fk: i64 = db.conn().query_row("PRAGMA foreign_keys", [], |row| row.get(0)).unwrap();
        assert_eq!(fk, 1);
    }
}
