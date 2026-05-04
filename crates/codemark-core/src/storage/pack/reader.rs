//! Logic for reading portable SQLite "packs".

use crate::engine::bookmark::{Bookmark, Collection, Resolution, ResolutionMethod, Visibility};
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
            "SELECT id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, status, updated_at, imported_from_url
             FROM collections WHERE visibility IS NOT NULL"
        )?;
        let rows = stmt.query_map([], |row| {
            let visibility_str: String = row.get(3)?;
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
                updated_at: row.get(11)?,
                imported_from_url: row.get(12)?,
            })
        })?;

        let mut collections = Vec::new();
        for row in rows {
            collections.push(row?);
        }
        Ok(collections)
    }

    pub fn bookmarks(&self) -> Result<Vec<Bookmark>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, query, language, file_path, content_hash, commit_hash, status, resolution_method, last_resolved_at, stale_since, created_at, created_by 
             FROM bookmarks"
        )?;
        let rows = stmt.query_map([], |row| {
            let status_str: String = row.get(6)?;
            let method_str: Option<String> = row.get(7)?;
            Ok(Bookmark {
                id: row.get(0)?,
                query: row.get(1)?,
                language: row.get(2)?,
                file_path: row.get(3)?,
                content_hash: row.get(4)?,
                commit_hash: row.get(5)?,
                status: crate::engine::bookmark::BookmarkStatus::from_str(&status_str)
                    .unwrap_or(crate::engine::bookmark::BookmarkStatus::Active),
                resolution_method: method_str.and_then(|s| ResolutionMethod::from_str(&s).ok()),
                last_resolved_at: row.get(8)?,
                stale_since: row.get(9)?,
                created_at: row.get(10)?,
                created_by: row.get(11)?,
                tags: Vec::new(),
                annotations: Vec::new(),
                comments: Vec::new(),
            })
        })?;

        let mut bookmarks = Vec::new();
        for row in rows {
            bookmarks.push(row?);
        }
        Ok(bookmarks)
    }

    pub fn resolutions(&self, bookmark_id: &str) -> Result<Vec<Resolution>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, bookmark_id, resolved_at, commit_hash, method, match_count, file_path, byte_range, line_range, content_hash, headline, preview_lines 
             FROM resolutions WHERE bookmark_id = ?1"
        )?;
        let rows = stmt.query_map([bookmark_id], |row| {
            let method_str: String = row.get(4)?;
            Ok(Resolution {
                id: row.get(0)?,
                bookmark_id: row.get(1)?,
                resolved_at: row.get(2)?,
                commit_hash: row.get(3)?,
                method: ResolutionMethod::from_str(&method_str).unwrap_or(ResolutionMethod::Exact),
                match_count: row.get(5)?,
                file_path: row.get(6)?,
                byte_range: row.get(7)?,
                line_range: row.get(8)?,
                content_hash: row.get(9)?,
                headline: row.get(10)?,
                preview_lines: row.get(11)?,
            })
        })?;

        let mut resolutions = Vec::new();
        for row in rows {
            resolutions.push(row?);
        }
        Ok(resolutions)
    }
}
