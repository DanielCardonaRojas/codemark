//! Database connection and migrations.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::Result;

const MIGRATION_001: &str = include_str!("../../../../migrations/V1__initial.sql");
const MIGRATION_002: &str = include_str!("../../../../migrations/V2__add_fts.sql");
const MIGRATION_003: &str = include_str!("../../../../migrations/V3__collection_ordering.sql");
const MIGRATION_004: &str = include_str!("../../../../migrations/V4__add_line_range.sql");
const MIGRATION_005: &str = include_str!("../../../../migrations/V5__add_embeddings.sql");
const MIGRATION_006: &str = include_str!("../../../../migrations/V6__add_fts_path_tags.sql");
const MIGRATION_007: &str = include_str!("../../../../migrations/V7__append_only_metadata.sql");
const MIGRATION_008: &str =
    include_str!("../../../../migrations/V8__add_identity_and_repo_metadata.sql");
const MIGRATION_009: &str = include_str!("../../../../migrations/V9__add_collection_branch.sql");
const MIGRATION_010: &str =
    include_str!("../../../../migrations/V10__add_comments_and_visibility.sql");
const MIGRATION_011: &str = include_str!("../../../../migrations/V11__add_publish_metadata.sql");
const MIGRATION_012: &str =
    include_str!("../../../../migrations/V12__add_resolution_display_fields.sql");
const MIGRATION_013: &str = include_str!("../../../../migrations/V13__add_published_tours.sql");
const MIGRATION_014: &str = include_str!("../../../../migrations/V14__add_imported_from_url.sql");

/// SQLite database wrapper with automatic migrations.
pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    /// Open the database at the given path, run migrations.
    /// Returns an error if the parent directory does not exist.
    pub fn open(path: &Path) -> Result<Self> {
        crate::embeddings::VecStore::load_extension()?;
        let conn = Connection::open(path)?;
        let mut db = Database { conn, path: path.to_path_buf() };
        db.init()?;
        Ok(db)
    }

    /// Create and open the database at the given path, creating parent directories if needed.
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            // Initialize global config file if it doesn't exist
            if let Err(e) = crate::config::Config::init_global_default() {
                eprintln!("codemark: warning: failed to initialize global config: {e}");
            }
        }
        Self::open(path)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self> {
        crate::embeddings::VecStore::load_extension()?;
        let conn = Connection::open_in_memory()?;
        let mut db = Database { conn, path: PathBuf::from(":memory:") };
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

    /// Get the path to this database.
    pub fn path(&self) -> &Path {
        &self.path
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
        Self::run_migrations_on(&mut self.conn)
    }

    pub fn run_migrations_on(conn: &mut Connection) -> Result<()> {
        let current_version = Self::get_schema_version(conn);

        let migrations = [
            (1, MIGRATION_001),
            (2, MIGRATION_002),
            (3, MIGRATION_003),
            (4, MIGRATION_004),
            (5, MIGRATION_005),
            (6, MIGRATION_006),
            (7, MIGRATION_007),
            (8, MIGRATION_008),
            (9, MIGRATION_009),
            (10, MIGRATION_010),
            (11, MIGRATION_011),
            (12, MIGRATION_012),
            (13, MIGRATION_013),
            (14, MIGRATION_014),
        ];

        for (version, sql) in migrations {
            if current_version < version {
                if version == 7 {
                    // Special case for migration 7 which has complex logic
                    Self::migrate_to_v7_on(conn)?;
                } else if version == 11 {
                    // Special case for migration 11 which requires disabling foreign keys
                    let original_state: i64 =
                        conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
                    conn.execute("PRAGMA foreign_keys = OFF", [])?;

                    let res = (|| -> Result<()> {
                        let tx = conn.transaction()?;
                        tx.execute_batch(sql)?;
                        Self::set_schema_version(&tx, 11)?;
                        tx.commit()?;
                        Ok(())
                    })();

                    // Restore foreign keys state
                    let _ = conn.execute(
                        &format!(
                            "PRAGMA foreign_keys = {}",
                            if original_state != 0 { "ON" } else { "OFF" }
                        ),
                        [],
                    );
                    res?;
                } else {
                    let tx = conn.transaction()?;
                    tx.execute_batch(sql)?;
                    Self::set_schema_version(&tx, version)?;
                    tx.commit()?;
                }

                // Post-migration hooks
                if version == 5 {
                    Self::init_embeddings_table_on(conn)?;
                }
            }
        }

        // Ensure embeddings table exists if we're at or past version 5
        if Self::get_schema_version(conn) >= 5 {
            Self::ensure_embeddings_table_on(conn)?;
        }

        Ok(())
    }

    /// Migrate to schema version 7: append-only metadata model
    /// This handles migrating existing data from the old schema to the new one.
    fn migrate_to_v7_on(conn: &mut Connection) -> Result<()> {
        // Run in a transaction for atomicity
        let tx = conn.transaction()?;

        // First run the base migration which creates new tables and recreates bookmarks
        tx.execute_batch(MIGRATION_007)?;

        // Now migrate existing data if we're coming from schema 6
        // Check if the old bookmarks table had data (we can tell by checking if annotations are empty)
        // Actually, the SQL migration should have already copied the bookmarks table data
        // We just need to migrate the annotations and tags

        // Try to migrate annotations from old schema
        // This will fail if the old columns don't exist (fresh install), so we ignore errors
        let migrate_annotations = |tx: &rusqlite::Transaction| -> Result<()> {
            tx.execute(
                "INSERT INTO bookmark_annotations (id, bookmark_id, added_at, added_by, notes, context, source)
                 SELECT lower(hex(randomblob(16))), id, created_at, created_by, notes, context, 'migration'
                 FROM (SELECT id, created_at, created_by, notes, context FROM bookmarks WHERE notes IS NOT NULL OR context IS NOT NULL)
                 WHERE notes IS NOT NULL OR context IS NOT NULL",
                [],
            )?;
            Ok(())
        };

        // Try to migrate tags from old schema
        let migrate_tags = |tx: &rusqlite::Transaction| -> Result<()> {
            tx.execute(
                "INSERT INTO bookmark_tags (bookmark_id, tag, added_at, added_by)
                 SELECT bm.id, TRIM(json_each.value), bm.created_at, bm.created_by
                 FROM bookmarks bm, json_each(bm.tags)
                 WHERE json_valid(bm.tags) AND TRIM(json_each.value) != ''",
                [],
            )?;
            Ok(())
        };

        // Ignore migration errors - they just mean we're on a fresh install or data already migrated
        let _ = migrate_annotations(&tx);
        let _ = migrate_tags(&tx);

        Self::set_schema_version(&tx, 7)?;

        tx.commit()?;
        Ok(())
    }

    /// Initialize the sqlite-vec extension and create the embeddings table.
    /// This is called during migration 005.
    fn init_embeddings_table_on(conn: &mut Connection) -> Result<()> {
        use crate::embeddings::VecStore;

        // Create the vec0 virtual table with 384 dimensions (all-MiniLM-L6-v2)
        let store = VecStore::new(384);
        store.create_table(conn).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to create embeddings table: {}", e))
        })?;

        Ok(())
    }

    /// Ensure the embeddings table exists, create it if it doesn't.
    /// This handles the case where the schema version is 5 but the table wasn't created.
    fn ensure_embeddings_table_on(conn: &mut Connection) -> Result<()> {
        use crate::embeddings::VecStore;

        // Check if table exists by trying to query it
        // vec0 virtual tables don't show up in sqlite_master like regular tables
        let exists = conn
            .query_row("SELECT COUNT(*) FROM bookmark_embeddings LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .is_ok();

        if !exists {
            // Create the vec0 virtual table with 384 dimensions (all-MiniLM-L6-v2)
            let store = VecStore::new(384);
            store.create_table(conn).map_err(|e| {
                crate::error::Error::Operation(format!("Failed to create embeddings table: {}", e))
            })?;
        }

        Ok(())
    }

    fn schema_version(&self) -> i64 {
        Self::get_schema_version(&self.conn)
    }

    fn get_schema_version(conn: &Connection) -> i64 {
        // First try PRAGMA user_version as it's the standard way
        if let Ok(v) = conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)) {
            if v > 0 {
                return v;
            }
        }

        // Fallback to schema_meta table (internal bookkeeping)
        conn.query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row: &rusqlite::Row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v: String| v.parse().ok())
        .unwrap_or(0)
    }

    fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', ?1)",
            [version.to_string()],
        )?;
        conn.execute(&format!("PRAGMA user_version = {version}"), [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Initialize sqlite-vec extension for all tests
    // This must be called before any database operations
    fn init_test_env() {}

    #[test]
    fn open_in_memory_succeeds() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let version = db.schema_version();
        assert_eq!(version, 14);
    }

    #[test]
    fn migration_is_idempotent() {
        init_test_env();
        let mut db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        assert_eq!(db.schema_version(), 14);
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
        assert!(tables.contains(&"bookmark_comments".to_string()));
        assert!(tables.contains(&"published_tours".to_string()));
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
