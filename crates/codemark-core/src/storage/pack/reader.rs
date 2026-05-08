//! Logic for reading portable SQLite "packs".

use crate::engine::bookmark::{
    Bookmark, BookmarkHealth, Collection, Resolution, ResolutionMethod, Visibility,
};
use crate::error::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::str::FromStr;

pub struct PackReader {
    conn: Connection,
}

impl PackReader {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn tours(&self) -> Result<Vec<Collection>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, status, health, health_computed_at, updated_at, imported_from_url
             FROM collections WHERE visibility IS NOT NULL"
        )?;
        let rows = stmt.query_map([], |row| {
            let visibility_str: String = row.get(3)?;
            let health: Option<String> = row.get(11)?;
            // Validate health value if present
            let health_parsed = if let Some(h) = &health {
                Some(h.parse::<crate::engine::bookmark::CollectionHealth>().map_err(|_| {
                    rusqlite::Error::InvalidColumnType(
                        11,
                        "invalid collection health value".into(),
                        rusqlite::types::Type::Text,
                    )
                })?)
            } else {
                None
            };
            Ok(Collection {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                visibility: Visibility::from_str(&visibility_str).unwrap_or(Visibility::Private),
                created_at: row.get(4)?,
                created_by: row.get(5)?,
                created_branch: row.get(6)?,
                published_at: row.get(7)?,
                published_commit_sha: row.get(8)?,
                repo_url: row.get(9)?,
                status: row.get(10)?,
                health: health_parsed,
                health_computed_at: row.get(12)?,
                updated_at: row.get(13)?,
                imported_from_url: row.get(14)?,
            })
        })?;

        let mut collections = Vec::new();
        for row in rows {
            collections.push(row?);
        }
        Ok(collections)
    }

    pub fn bookmarks(&self) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn().prepare(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
                    r.health, r.method, r.resolved_at, NULL as stale_since,
                    b.created_at, b.created_by, b.current_resolution_id
             FROM bookmarks b
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id",
        )?;
        let rows = stmt.query_map([], row_to_bookmark)?;

        let mut bookmarks = Vec::new();
        for row in rows {
            bookmarks.push(row?);
        }
        Ok(bookmarks)
    }

    pub fn bookmarks_for_collection(&self, collection_id: &str) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn().prepare(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
                    r.health, r.method, r.resolved_at, NULL as stale_since,
                    b.created_at, b.created_by, b.current_resolution_id
             FROM bookmarks b
             JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id
             WHERE cb.collection_id = ?1
             ORDER BY cb.position ASC",
        )?;
        let rows = stmt.query_map([collection_id], row_to_bookmark)?;

        let mut bookmarks = Vec::new();
        for row in rows {
            bookmarks.push(row?);
        }
        Ok(bookmarks)
    }

    pub fn resolutions(&self, bookmark_id: &str) -> Result<Vec<Resolution>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, bookmark_id, resolved_at, health, commit_hash, method, match_count, file_path, byte_range, line_range, content_hash, headline, snapshot, breadcrumbs
             FROM resolutions WHERE bookmark_id = ?1"
        )?;
        let rows = stmt.query_map([bookmark_id], |row| {
            let method_str: String = row.get(5)?;
            let health_str: String = row.get(3)?;
            let health = health_str.parse::<BookmarkHealth>().unwrap_or(BookmarkHealth::Active);
            Ok(Resolution {
                id: row.get(0)?,
                bookmark_id: row.get(1)?,
                resolved_at: row.get(2)?,
                health,
                commit_hash: row.get(4)?,
                method: ResolutionMethod::from_str(&method_str).unwrap_or(ResolutionMethod::Exact),
                match_count: row.get(6)?,
                file_path: row.get(7)?,
                byte_range: row.get(8)?,
                line_range: row.get(9)?,
                content_hash: row.get(10)?,
                headline: row.get(11)?,
                snapshot: row.get(12)?,
                breadcrumbs: row.get(13)?,
            })
        })?;

        let mut resolutions = Vec::new();
        for row in rows {
            resolutions.push(row?);
        }
        Ok(resolutions)
    }
}

fn row_to_bookmark(row: &rusqlite::Row) -> rusqlite::Result<Bookmark> {
    let health_str: Option<String> = row.get(6)?;
    let method_str: Option<String> = row.get(7)?;

    let health = health_str.and_then(|s| s.parse().ok()).unwrap_or(BookmarkHealth::Active);

    let resolution_method = method_str.and_then(|s| s.parse().ok());

    Ok(Bookmark {
        id: row.get(0)?,
        query: row.get(1)?,
        language: row.get(2)?,
        file_path: row.get(3)?,
        content_hash: row.get(4)?,
        commit_hash: row.get(5)?,
        health,
        resolution_method,
        last_resolved_at: row.get(8)?,
        stale_since: row.get(9)?,
        created_at: row.get(10)?,
        created_by: row.get(11)?,
        current_resolution_id: row.get(12)?,
        tags: vec![],
        annotations: vec![],
        comments: vec![],
    })
}
