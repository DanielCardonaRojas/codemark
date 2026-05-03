use std::path::Path;
use rusqlite::{Connection, OpenFlags};
use thiserror::Error;
use serde::Serialize;

#[derive(Error, Debug, Serialize)]
#[serde(tag = "error", content = "reason", rename_all = "snake_case")]
pub enum PackError {
    #[error("Not a valid SQLite file")]
    NotSqlite,
    #[error("Invalid user version: {0}")]
    InvalidUserVersion(i64),
    #[error("Disallowed schema item: {0} ({1})")]
    DisallowedSchemaItem(String, String),
    #[error("Table {0} has invalid columns")]
    InvalidTableSchema(String),
    #[error("Invalid pack metadata: {0}")]
    InvalidMetadata(String),
    #[error("Tour count out of range: {0}")]
    TourCountOutOfRange(usize),
    #[error("Dangling bookmark reference: {0}")]
    DanglingBookmarkRef(String),
    #[error("Bookmark count exceeds limit: {0}")]
    BookmarkLimitExceeded(usize),
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("IO error: {0}")]
    IoError(String),
}

impl From<rusqlite::Error> for PackError {
    fn from(e: rusqlite::Error) -> Self {
        PackError::DatabaseError(e.to_string())
    }
}

#[derive(Debug, Serialize)]
pub struct PackInfo {
    pub user_version: i64,
    pub tour_count: usize,
    pub bookmark_count: usize,
    pub source_client: String,
}

/// Basic safety check: no views, no triggers, no virtual tables.
/// Returns user_version.
pub fn pre_inspect(pack_path: &Path) -> Result<i64, PackError> {
    let conn = Connection::open_with_flags(pack_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| PackError::NotSqlite)?;

    // 1. Pragma checks
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if user_version <= 0 {
        return Err(PackError::InvalidUserVersion(user_version));
    }

    // 2. Disallowed items check (views, triggers, etc.)
    let mut stmt = conn.prepare(
        "SELECT type, name, sql FROM sqlite_schema 
         WHERE type IN ('table', 'view', 'trigger', 'index') 
         AND name NOT LIKE 'sqlite_%'"
    )?;
    let mut rows = stmt.query([])?;
    
    let allowed_tables = [
        "collections",
        "collection_bookmarks",
        "bookmarks",
        "bookmark_annotations",
        "bookmark_tags",
        "bookmark_comments",
        "resolutions",
        "schema_meta",
        "_pack_meta",
        "repos",
    ];

    while let Some(row) = rows.next()? {
        let item_type: String = row.get(0)?;
        let name: String = row.get(1)?;
        
        if item_type != "table" && item_type != "index" {
            return Err(PackError::DisallowedSchemaItem(name, item_type));
        }
        
        if item_type == "table" && !allowed_tables.contains(&name.as_str()) {
             return Err(PackError::DisallowedSchemaItem(name, item_type));
        }
    }

    Ok(user_version)
}

/// Full inspection after migration (if any).
pub fn inspect(pack_path: &Path) -> Result<PackInfo, PackError> {
    let conn = Connection::open_with_flags(pack_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| PackError::NotSqlite)?;

    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    // 1. Metadata check
    let (source_client, purpose): (String, String) = conn.query_row(
        "SELECT source_client, purpose FROM _pack_meta LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?))
    ).map_err(|e| PackError::InvalidMetadata(e.to_string()))?;

    if purpose != "publish" && purpose != "mirror" && purpose != "export" {
        return Err(PackError::InvalidMetadata(format!("Invalid purpose: {}", purpose)));
    }

    // 2. Row counts and integrity
    let tour_count: usize = conn.query_row(
        "SELECT COUNT(*) FROM collections WHERE visibility IS NOT NULL",
        [],
        |row| row.get(0)
    )?;

    if purpose == "publish" && tour_count != 1 {
        return Err(PackError::TourCountOutOfRange(tour_count));
    }
    if tour_count == 0 {
        return Err(PackError::TourCountOutOfRange(tour_count));
    }

    let bookmark_count: usize = conn.query_row(
        "SELECT COUNT(*) FROM bookmarks",
        [],
        |row| row.get(0)
    )?;

    if bookmark_count > 500 {
        return Err(PackError::BookmarkLimitExceeded(bookmark_count));
    }

    // Integrity: collection_bookmarks.bookmark_id must exist in bookmarks
    let dangling_bookmarks: usize = conn.query_row(
        "SELECT COUNT(*) FROM collection_bookmarks 
         WHERE bookmark_id NOT IN (SELECT id FROM bookmarks)",
        [],
        |row| row.get(0)
    )?;
    if dangling_bookmarks > 0 {
        return Err(PackError::DanglingBookmarkRef(format!("{} dangling bookmarks", dangling_bookmarks)));
    }

    Ok(PackInfo {
        user_version,
        tour_count,
        bookmark_count,
        source_client,
    })
}
