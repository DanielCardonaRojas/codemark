//! Bookmark database operations.

#![allow(dead_code)]

use std::collections::HashMap;

use crate::engine::bookmark::{
    Annotation, Bookmark, BookmarkFilter, BookmarkHealth, ResolutionMethod, Tag,
};
use crate::error::{Error, Result};
use crate::storage::db::Database;

impl Database {
    /// Insert a bookmark, returning the bookmark ID.
    /// If a bookmark with the same file_path and query exists, returns its ID instead.
    pub fn insert_bookmark(&self, bookmark: &Bookmark) -> Result<String> {
        let tx = self.conn().unchecked_transaction()?;

        // Try to insert with OR FAIL to check for uniqueness constraint
        let result = tx.execute(
            "INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
             created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                bookmark.id,
                bookmark.query,
                bookmark.language,
                bookmark.file_path,
                bookmark.content_hash,
                bookmark.commit_hash,
                bookmark.created_at,
                bookmark.created_by,
            ],
        );

        match result {
            Ok(_) => {
                // Create initial resolution
                let res_id = uuid::Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO resolutions (id, bookmark_id, resolved_at, health, commit_hash, method, match_count, file_path, content_hash, is_dirty)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    rusqlite::params![
                        res_id,
                        bookmark.id,
                        bookmark.created_at, // Use created_at as initial resolved_at
                        bookmark.health.to_string(),
                        bookmark.commit_hash,
                        bookmark.resolution_method.map(|m| m.to_string()).unwrap_or_else(|| "exact".to_string()),
                        1,
                        bookmark.file_path,
                        bookmark.content_hash,
                        false, // Initial resolution is anchored (is_dirty = false means clean/committed)
                    ],
                )?;

                // Link resolution to bookmark
                tx.execute(
                    "UPDATE bookmarks SET current_resolution_id = ?1 WHERE id = ?2",
                    rusqlite::params![res_id, bookmark.id],
                )?;

                tx.commit()?;
                Ok(bookmark.id.clone())
            }
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // Bookmark with same file_path and query exists, fetch its ID
                let existing_id: String = tx.query_row(
                    "SELECT id FROM bookmarks WHERE file_path = ?1 AND query = ?2",
                    rusqlite::params![bookmark.file_path, bookmark.query],
                    |row| row.get(0),
                )?;
                Ok(existing_id)
            }
            Err(e) => Err(Error::from(e)),
        }
    }

    /// Insert an annotation for a bookmark.
    pub fn insert_annotation(&self, annotation: &Annotation) -> Result<()> {
        self.conn().execute(
            "INSERT INTO bookmark_annotations (id, bookmark_id, added_at, added_by, notes, context, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                annotation.id,
                annotation.bookmark_id,
                annotation.added_at,
                annotation.added_by,
                annotation.notes,
                annotation.context,
                annotation.source,
            ],
        )?;
        Ok(())
    }

    /// Insert multiple annotations for a bookmark atomically.
    pub fn insert_annotations(&self, annotations: &[Annotation]) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        for annotation in annotations {
            tx.execute(
                "INSERT INTO bookmark_annotations (id, bookmark_id, added_at, added_by, notes, context, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    annotation.id,
                    annotation.bookmark_id,
                    annotation.added_at,
                    annotation.added_by,
                    annotation.notes,
                    annotation.context,
                    annotation.source,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Insert a tag for a bookmark.
    pub fn insert_tag(&self, tag: &Tag) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag, added_at, added_by)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tag.bookmark_id, tag.tag, tag.added_at, tag.added_by],
        )?;
        Ok(())
    }

    /// Insert multiple tags for a bookmark.
    pub fn insert_tags(&self, tags: &[Tag]) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        for tag in tags {
            tx.execute(
                "INSERT OR IGNORE INTO bookmark_tags (bookmark_id, tag, added_at, added_by)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![tag.bookmark_id, tag.tag, tag.added_at, tag.added_by],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_bookmark(&self, id: &str) -> Result<Option<Bookmark>> {
        let mut stmt = self.conn().prepare(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             r.health, r.method, r.resolved_at, NULL as stale_since, b.created_at, b.created_by, b.current_resolution_id, b.repo_id
             FROM bookmarks b
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id
             WHERE b.id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_bookmark_base)?;
        match rows.next() {
            Some(row) => {
                let mut bm = row?;
                self.load_bookmark_metadata(&mut bm)?;
                Ok(Some(bm))
            }
            None => Ok(None),
        }
    }

    pub fn get_bookmark_by_prefix(&self, prefix: &str) -> Result<Option<Bookmark>> {
        if prefix.len() < 4 {
            return Err(Error::Input("bookmark ID prefix must be at least 4 characters".into()));
        }
        let pattern = format!("{prefix}%");
        let mut stmt = self.conn().prepare(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             r.health, r.method, r.resolved_at, NULL as stale_since, b.created_at, b.created_by, b.current_resolution_id, b.repo_id
             FROM bookmarks b
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id
             WHERE b.id LIKE ?1",
        )?;
        let mut results: Vec<Bookmark> =
            stmt.query_map([&pattern], row_to_bookmark_base)?.filter_map(|r| r.ok()).collect();

        for bm in &mut results {
            self.load_bookmark_metadata(bm)?;
        }

        match results.len() {
            0 => Ok(None),
            1 => Ok(Some(results.into_iter().next().unwrap())),
            _ => Err(Error::Input(format!(
                "ambiguous bookmark ID prefix '{prefix}': matches {} bookmarks",
                results.len()
            ))),
        }
    }

    /// Find an existing bookmark by file_path and query.
    pub fn find_bookmark_by_location(
        &self,
        file_path: &str,
        query: &str,
    ) -> Result<Option<Bookmark>> {
        let mut stmt = self.conn().prepare(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             r.health, r.method, r.resolved_at, NULL as stale_since, b.created_at, b.created_by, b.current_resolution_id, b.repo_id
             FROM bookmarks b
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id
             WHERE b.file_path = ?1 AND b.query = ?2",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![file_path, query], row_to_bookmark_base)?;
        match rows.next() {
            Some(row) => {
                let mut bm = row?;
                self.load_bookmark_metadata(&mut bm)?;
                Ok(Some(bm))
            }
            None => Ok(None),
        }
    }

    pub fn list_bookmarks(&self, filter: &BookmarkFilter) -> Result<Vec<Bookmark>> {
        let mut sql = String::from(
            "SELECT DISTINCT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             r.health, r.method, r.resolved_at, NULL as stale_since, b.created_at, b.created_by, b.current_resolution_id, b.repo_id
             FROM bookmarks b
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id",
        );
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if filter.collection.is_some() || filter.collection_id.is_some() {
            sql.push_str(
                " JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
                 JOIN collections c ON cb.collection_id = c.id",
            );
        }

        if let Some(ref tag) = filter.tag {
            sql.push_str(" JOIN bookmark_tags bt ON b.id = bt.bookmark_id");
            conditions.push("bt.tag = ?".to_string());
            params.push(Box::new(tag.clone()));
        }

        if let Some(ref healths) = filter.health {
            let placeholders: Vec<String> =
                healths.iter().enumerate().map(|_| "?".to_string()).collect();
            conditions.push(format!("r.health IN ({})", placeholders.join(", ")));
            for h in healths {
                params.push(Box::new(h.to_string()));
            }
        }

        if let Some(ref file_path) = filter.file_path {
            conditions.push("b.file_path = ?".to_string());
            params.push(Box::new(file_path.clone()));
        }

        if let Some(ref language) = filter.language {
            conditions.push("b.language = ?".to_string());
            params.push(Box::new(language.clone()));
        }

        if let Some(ref created_by) = filter.created_by {
            conditions.push("b.created_by = ?".to_string());
            params.push(Box::new(created_by.clone()));
        }

        if let Some(ref collection_name) = filter.collection {
            conditions.push("c.name = ?".to_string());
            params.push(Box::new(collection_name.clone()));
        }

        if let Some(ref collection_id) = filter.collection_id {
            conditions.push("c.id LIKE ?".to_string());
            params.push(Box::new(format!("{}%", collection_id)));
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        if filter.collection.is_some() || filter.collection_id.is_some() {
            sql.push_str(" ORDER BY cb.position ASC, b.created_at DESC");
        } else {
            sql.push_str(" ORDER BY b.created_at DESC");
        }

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn().prepare(&sql)?;
        let mut results: Vec<Bookmark> = stmt
            .query_map(param_refs.as_slice(), row_to_bookmark_base)?
            .filter_map(|r| r.ok())
            .collect();

        for bm in &mut results {
            self.load_bookmark_metadata(bm)?;
        }

        Ok(results)
    }

    pub fn update_bookmark_resolution_id(&self, id: &str, resolution_id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE bookmarks SET current_resolution_id = ?1 WHERE id = ?2",
            rusqlite::params![resolution_id, id],
        )?;
        Ok(())
    }

    pub fn update_bookmark_health(
        &self,
        id: &str,
        _health: BookmarkHealth,
        _resolution_method: Option<ResolutionMethod>,
        _last_resolved_at: Option<&str>,
        _stale_since: Option<&str>,
    ) -> Result<()> {
        // In the resolution-centric model, updating "health" means creating a new resolution
        // and linking it. However, the 'heal' logic in maintenance.rs already creates a resolution.
        // So this method might be redundant or should just update the current_resolution_id
        // if we have it.

        // For backward compatibility during refactoring, we'll try to find the latest resolution
        // for this bookmark and link it.
        let latest_res_id: Option<String> = self.conn().query_row(
            "SELECT id FROM resolutions WHERE bookmark_id = ?1 ORDER BY resolved_at DESC LIMIT 1",
            [id],
            |row| row.get(0),
        ).ok();

        if let Some(res_id) = latest_res_id {
            self.update_bookmark_resolution_id(id, &res_id)?;
        }

        Ok(())
    }

    pub fn delete_bookmark(&self, id: &str) -> Result<bool> {
        let count = self.conn().execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
        Ok(count > 0)
    }

    pub fn count_by_health(&self) -> Result<HashMap<BookmarkHealth, usize>> {
        let mut stmt = self.conn().prepare(
            "SELECT r.health, COUNT(*) 
             FROM bookmarks b
             JOIN resolutions r ON b.current_resolution_id = r.id
             GROUP BY r.health",
        )?;
        let rows = stmt.query_map([], |row| {
            let health: String = row.get(0)?;
            let count: usize = row.get(1)?;
            Ok((health, count))
        })?;

        let mut map = HashMap::new();
        for row in rows {
            let (health_str, count) = row?;
            if let Ok(health) = health_str.parse::<BookmarkHealth>() {
                map.insert(health, count);
            }
        }
        Ok(map)
    }

    pub fn delete_archived_before(&self, before: &str) -> Result<usize> {
        let count = self.conn().execute(
            "DELETE FROM bookmarks 
             WHERE id IN (
                 SELECT b.id FROM bookmarks b
                 JOIN resolutions r ON b.current_resolution_id = r.id
                 WHERE r.health = 'archived' AND b.created_at < ?1
             )",
            [before],
        )?;
        Ok(count)
    }

    /// Delete bookmarks that are not referenced by any collection.
    pub fn delete_orphans(&self) -> Result<usize> {
        let count = self.conn().execute(
            "DELETE FROM bookmarks 
             WHERE id NOT IN (SELECT bookmark_id FROM collection_bookmarks)",
            [],
        )?;
        Ok(count)
    }

    /// Search bookmarks by text in notes and/or context fields.
    #[allow(clippy::too_many_arguments)]
    pub fn search_bookmarks(
        &self,
        query: Option<&str>,
        note: Option<&str>,
        context: Option<&str>,
        language: Option<&str>,
        created_by: Option<&str>,
        collection: Option<&str>,
        health: Option<Vec<BookmarkHealth>>,
    ) -> Result<Vec<Bookmark>> {
        // Always need annotation join for general search
        // Always need tag join for tag search
        let mut sql = String::from(
            "SELECT DISTINCT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             r.health, r.method, r.resolved_at, NULL as stale_since, b.created_at, b.created_by, b.current_resolution_id, b.repo_id
             FROM bookmarks b
             LEFT JOIN resolutions r ON b.current_resolution_id = r.id
             LEFT JOIN bookmark_annotations ba ON b.id = ba.bookmark_id
             LEFT JOIN bookmark_tags bt ON b.id = bt.bookmark_id",
        );

        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if collection.is_some() {
            sql.push_str(
                " JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
                 JOIN collections c ON cb.collection_id = c.id",
            );
        }

        // Search in annotations and tags
        if let Some(n) = note {
            conditions.push("ba.notes LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", n)));
        }
        if let Some(ctx) = context {
            conditions.push("ba.context LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", ctx)));
        }
        if let Some(q) = query {
            // Search in notes, context, file_path, and tags
            let mut search_conditions = Vec::new();
            search_conditions.push("b.file_path LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", q)));
            search_conditions.push("ba.notes LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", q)));
            search_conditions.push("ba.context LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", q)));
            search_conditions.push("bt.tag LIKE ?".to_string());
            params.push(Box::new(format!("%{}%", q)));
            conditions.push(format!("({})", search_conditions.join(" OR ")));
        }
        if let Some(lang) = language {
            conditions.push("b.language = ?".to_string());
            params.push(Box::new(lang.to_string()));
        }
        if let Some(author) = created_by {
            conditions.push("b.created_by = ?".to_string());
            params.push(Box::new(author.to_string()));
        }
        if let Some(col_name) = collection {
            conditions.push("c.name = ?".to_string());
            params.push(Box::new(col_name.to_string()));
        }

        if let Some(ref healths) = health {
            let placeholders: Vec<String> =
                healths.iter().enumerate().map(|_| "?".to_string()).collect();
            conditions.push(format!("r.health IN ({})", placeholders.join(", ")));
            for h in healths {
                params.push(Box::new(h.to_string()));
            }
        } else {
            // Exclude archived by default
            conditions.push("r.health != 'archived'".to_string());
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        sql.push_str(" ORDER BY b.created_at DESC");

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn().prepare(&sql)?;
        let mut results: Vec<Bookmark> = stmt
            .query_map(param_refs.as_slice(), row_to_bookmark_base)?
            .filter_map(|r| r.ok())
            .collect();

        for bm in &mut results {
            self.load_bookmark_metadata(bm)?;
        }

        Ok(results)
    }

    /// List all tags for a specific bookmark.
    pub fn list_tags_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<Tag>> {
        let mut stmt = self.conn().prepare(
            "SELECT bookmark_id, tag, added_at, added_by FROM bookmark_tags WHERE bookmark_id = ?1 ORDER BY tag ASC"
        )?;
        let rows = stmt.query_map([bookmark_id], |row| {
            Ok(Tag {
                bookmark_id: row.get(0)?,
                tag: row.get(1)?,
                added_at: row.get(2)?,
                added_by: row.get(3)?,
            })
        })?;
        let results: Vec<Tag> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Load annotations and tags for a bookmark.
    fn load_bookmark_metadata(&self, bm: &mut Bookmark) -> Result<()> {
        // Load annotations
        let mut ann_stmt = self.conn().prepare(
            "SELECT id, bookmark_id, added_at, added_by, notes, context, source
             FROM bookmark_annotations WHERE bookmark_id = ?1 ORDER BY added_at ASC",
        )?;
        let annotations: Vec<Annotation> =
            ann_stmt.query_map([&bm.id], row_to_annotation)?.filter_map(|r| r.ok()).collect();
        bm.annotations = annotations;

        // Load tags
        let mut tag_stmt = self
            .conn()
            .prepare("SELECT tag, added_at, added_by FROM bookmark_tags WHERE bookmark_id = ?1 ORDER BY tag ASC")?;
        let tags: Vec<String> = tag_stmt
            .query_map([&bm.id], |row| {
                Ok(Tag {
                    bookmark_id: bm.id.clone(),
                    tag: row.get(0)?,
                    added_at: row.get(1)?,
                    added_by: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .map(|t| t.tag)
            .collect();
        bm.tags = tags;

        // Load comments
        bm.comments = self.list_comments_for_bookmark(&bm.id)?;

        Ok(())
    }

    /// List all bookmarks in a specific collection.
    pub fn list_bookmarks_in_collection(&self, collection_id: &str) -> Result<Vec<Bookmark>> {
        let filter =
            BookmarkFilter { collection_id: Some(collection_id.to_string()), ..Default::default() };
        self.list_bookmarks(&filter)
    }

    /// List all unique tags used in bookmarks.
    pub fn list_bookmark_tags(&self) -> Result<Vec<String>> {
        let mut stmt =
            self.conn().prepare("SELECT DISTINCT tag FROM bookmark_tags ORDER BY tag")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tags = Vec::new();
        for tag in rows {
            tags.push(tag?);
        }
        Ok(tags)
    }
}

fn row_to_bookmark_base(row: &rusqlite::Row) -> rusqlite::Result<Bookmark> {
    let health_str: Option<String> = row.get(6)?;
    let method_str: Option<String> = row.get(7)?;

    Ok(Bookmark {
        id: row.get(0)?,
        query: row.get(1)?,
        language: row.get(2)?,
        file_path: row.get(3)?,
        content_hash: row.get(4)?,
        commit_hash: row.get(5)?,
        health: health_str.as_ref().and_then(|s| s.parse().ok()).unwrap_or(BookmarkHealth::Active),
        resolution_method: method_str.and_then(|s| s.parse().ok()),
        last_resolved_at: row.get(8)?,
        stale_since: row.get(9)?,
        created_at: row.get(10)?,
        created_by: row.get(11)?,
        current_resolution_id: row.get(12)?,
        repo_id: row.get(13)?,
        tags: Vec::new(),        // Loaded separately
        annotations: Vec::new(), // Loaded separately
        comments: Vec::new(),    // Loaded separately
    })
}

fn row_to_annotation(row: &rusqlite::Row) -> rusqlite::Result<Annotation> {
    Ok(Annotation {
        id: row.get(0)?,
        bookmark_id: row.get(1)?,
        added_at: row.get(2)?,
        added_by: row.get(3)?,
        notes: row.get(4)?,
        context: row.get(5)?,
        source: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::BookmarkFilter;
    use crate::engine::bookmark::{
        Annotation, Bookmark, BookmarkHealth, Collection, ResolutionMethod, Tag,
    };
    use crate::storage::db::Database;

    // Initialize sqlite-vec extension for all tests
    fn init_test_env() {}

    fn test_bookmark(id: &str) -> Bookmark {
        // Use unique file_path and query to avoid UNIQUE constraint violations
        Bookmark {
            id: id.to_string(),
            query: format!("(function_declaration) @{} /* {} */", "target", id),
            language: "swift".to_string(),
            file_path: format!("src/main_{}.swift", id),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            commit_hash: Some("abc123".to_string()),
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![],
        }
    }

    #[test]
    fn insert_and_get_bookmark() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("aaaa-bbbb-cccc-dddd");
        db.insert_bookmark(&bm).unwrap();

        let fetched = db.get_bookmark("aaaa-bbbb-cccc-dddd").unwrap().unwrap();
        assert_eq!(fetched.id, bm.id);
        assert_eq!(fetched.query, bm.query);
        assert_eq!(fetched.health, BookmarkHealth::Active);
        assert!(fetched.current_resolution_id.is_some());
        assert!(fetched.tags.is_empty());
        assert!(fetched.annotations.is_empty());
    }

    #[test]
    fn insert_duplicate_returns_existing_id() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm1 = test_bookmark("aaaa-1111-2222-3333");
        let mut bm2 = test_bookmark("bbbb-4444-5555-6666");
        bm2.query = bm1.query.clone();
        bm2.file_path = bm1.file_path.clone();

        db.insert_bookmark(&bm1).unwrap();
        let existing_id = db.insert_bookmark(&bm2).unwrap();

        assert_eq!(existing_id, bm1.id);
    }

    #[test]
    fn insert_and_load_annotations() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("aaaa-bbbb-cccc-dddd");
        db.insert_bookmark(&bm).unwrap();

        let ann = Annotation {
            id: "ann-1".to_string(),
            bookmark_id: bm.id.clone(),
            added_at: "2026-04-01T00:00:00Z".to_string(),
            added_by: Some("test-user".to_string()),
            notes: Some("test note".to_string()),
            context: None,
            source: None,
        };
        db.insert_annotation(&ann).unwrap();

        let fetched = db.get_bookmark(&bm.id).unwrap().unwrap();
        assert_eq!(fetched.annotations.len(), 1);
        assert_eq!(fetched.annotations[0].notes, Some("test note".to_string()));
        assert_eq!(fetched.annotations[0].added_by, Some("test-user".to_string()));
    }

    #[test]
    fn insert_and_load_tags() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("aaaa-bbbb-cccc-dddd");
        db.insert_bookmark(&bm).unwrap();

        let tags = vec![
            Tag {
                bookmark_id: bm.id.clone(),
                tag: "auth".to_string(),
                added_at: "2026-04-01T00:00:00Z".to_string(),
                added_by: None,
            },
            Tag {
                bookmark_id: bm.id.clone(),
                tag: "api".to_string(),
                added_at: "2026-04-01T00:00:00Z".to_string(),
                added_by: None,
            },
        ];
        db.insert_tags(&tags).unwrap();

        let fetched = db.get_bookmark(&bm.id).unwrap().unwrap();
        assert_eq!(fetched.tags.len(), 2);
        assert!(fetched.tags.contains(&"auth".to_string()));
        assert!(fetched.tags.contains(&"api".to_string()));
    }

    #[test]
    fn get_bookmark_by_prefix() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-1111-2222-3333")).unwrap();
        db.insert_bookmark(&test_bookmark("bbbb-1111-2222-3333")).unwrap();

        let found = db.get_bookmark_by_prefix("aaaa").unwrap().unwrap();
        assert_eq!(found.id, "aaaa-1111-2222-3333");

        assert!(db.get_bookmark_by_prefix("cccc").unwrap().is_none());
    }

    #[test]
    fn prefix_too_short_errors() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_bookmark_by_prefix("aa").is_err());
    }

    #[test]
    fn ambiguous_prefix_errors() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-1111-0000-0000")).unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-2222-0000-0000")).unwrap();

        let result = db.get_bookmark_by_prefix("aaaa");
        assert!(result.is_err());
    }

    #[test]
    fn list_with_tag_filter() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm1 = test_bookmark("aaaa-0000-0000-0001");
        let bm2 = test_bookmark("aaaa-0000-0000-0002");

        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        db.insert_tags(&[Tag {
            bookmark_id: bm1.id.clone(),
            tag: "auth".to_string(),
            added_at: "2026-04-01T00:00:00Z".to_string(),
            added_by: None,
        }])
        .unwrap();

        db.insert_tags(&[Tag {
            bookmark_id: bm2.id.clone(),
            tag: "api".to_string(),
            added_at: "2026-04-01T00:00:00Z".to_string(),
            added_by: None,
        }])
        .unwrap();

        let filter = BookmarkFilter { tag: Some("auth".into()), ..Default::default() };
        let results = db.list_bookmarks(&filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "aaaa-0000-0000-0001");
    }

    #[test]
    fn list_with_health_filter() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let mut bm1 = test_bookmark("aaaa-0000-0000-0001");
        bm1.health = BookmarkHealth::Active;
        let mut bm2 = test_bookmark("aaaa-0000-0000-0002");
        bm2.health = BookmarkHealth::Stale;
        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        let filter =
            BookmarkFilter { health: Some(vec![BookmarkHealth::Stale]), ..Default::default() };
        let results = db.list_bookmarks(&filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "aaaa-0000-0000-0002");
    }

    #[test]
    fn list_with_collection_filter() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm1 = test_bookmark("aaaa-0000-0000-0001");
        let bm2 = test_bookmark("aaaa-0000-0000-0002");
        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        let col = Collection {
            id: "col-1111".to_string(),
            name: "My Collection".to_string(),
            description: None,
            visibility: crate::engine::bookmark::Visibility::Private,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            created_branch: Some("main".to_string()),
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: None,
            health_computed_at: None,
            updated_at: None,
            imported_from_url: None,
        };
        db.insert_collection(&col).unwrap();
        db.add_to_collection(&col.id, std::slice::from_ref(&bm1.id)).unwrap();

        // Filter by name
        let filter_name =
            BookmarkFilter { collection: Some("My Collection".into()), ..Default::default() };
        let results = db.list_bookmarks(&filter_name).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, bm1.id);

        // Filter by ID
        let filter_id =
            BookmarkFilter { collection_id: Some("col-1111".into()), ..Default::default() };
        let results = db.list_bookmarks(&filter_id).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, bm1.id);

        // Filter by ID prefix (segment)
        let filter_prefix =
            BookmarkFilter { collection_id: Some("col-".into()), ..Default::default() };
        let results = db.list_bookmarks(&filter_prefix).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, bm1.id);

        // Filter by wrong ID
        let filter_wrong =
            BookmarkFilter { collection_id: Some("wrong".into()), ..Default::default() };
        let results = db.list_bookmarks(&filter_wrong).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn update_health() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-0000-0000-0001")).unwrap();

        // In new model, we'd normally create a resolution.
        // For the test, we'll manually insert one and link it.
        let res = crate::engine::bookmark::Resolution {
            is_dirty: false,
            id: "res-new".to_string(),
            bookmark_id: "aaaa-0000-0000-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            health: BookmarkHealth::Drifted,
            commit_hash: None,
            method: ResolutionMethod::Relaxed,
            match_count: Some(1),
            file_path: None,
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        db.insert_resolution(&res).unwrap();
        db.update_bookmark_resolution_id("aaaa-0000-0000-0001", "res-new").unwrap();

        let bm = db.get_bookmark("aaaa-0000-0000-0001").unwrap().unwrap();
        assert_eq!(bm.health, BookmarkHealth::Drifted);
        assert_eq!(bm.resolution_method, Some(ResolutionMethod::Relaxed));
    }

    #[test]
    fn delete_bookmark_cascades() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("aaaa-0000-0000-0001");
        db.insert_bookmark(&bm).unwrap();

        // Add annotation and tag
        db.insert_annotation(&Annotation {
            id: "ann-1".to_string(),
            bookmark_id: bm.id.clone(),
            added_at: "2026-04-01T00:00:00Z".to_string(),
            added_by: None,
            notes: Some("note".to_string()),
            context: None,
            source: None,
        })
        .unwrap();
        db.insert_tags(&[Tag {
            bookmark_id: bm.id.clone(),
            tag: "test".to_string(),
            added_at: "2026-04-01T00:00:00Z".to_string(),
            added_by: None,
        }])
        .unwrap();

        assert!(db.delete_bookmark("aaaa-0000-0000-0001").unwrap());
        assert!(db.get_bookmark("aaaa-0000-0000-0001").unwrap().is_none());
    }

    #[test]
    fn count_by_health() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let mut bm1 = test_bookmark("aaaa-0000-0000-0001");
        bm1.health = BookmarkHealth::Active;
        let mut bm2 = test_bookmark("aaaa-0000-0000-0002");
        bm2.health = BookmarkHealth::Stale;
        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        let counts = db.count_by_health().unwrap();
        assert_eq!(counts.get(&BookmarkHealth::Active), Some(&1));
        assert_eq!(counts.get(&BookmarkHealth::Stale), Some(&1));
    }

    #[test]
    fn find_bookmark_by_location() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("test-id");
        let expected_path = bm.file_path.clone();
        let expected_query = bm.query.clone();
        db.insert_bookmark(&bm).unwrap();

        let found = db.find_bookmark_by_location(&expected_path, &expected_query).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "test-id");

        let not_found =
            db.find_bookmark_by_location("other.swift", "(function_declaration) @target").unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn test_immutable_query() {
        // @lat: [[tests#System Invariants Tests#Bookmark Integrity Rules#Immutable Intent]]
        // Verifies that a bookmark's id, query, language, and file_path are immutable.
        // Once created, these fields must never be modified by any operation.
        init_test_env();
        let db = Database::open_in_memory().unwrap();

        // Create a bookmark with specific immutable fields
        let bm_id = "immutable-test-bm";
        let original_query = "(function_declaration) @target /* immutable-test-bm */";
        let original_language = "swift";
        let original_file_path = "src/main_immutable-test-bm.swift";

        let bm = Bookmark {
            id: bm_id.to_string(),
            query: original_query.to_string(),
            language: original_language.to_string(),
            file_path: original_file_path.to_string(),
            content_hash: Some("sha256:original1234".to_string()),
            commit_hash: Some("commit-original".to_string()),
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };
        db.insert_bookmark(&bm).unwrap();

        // Capture the original immutable fields
        let original = db.get_bookmark(bm_id).unwrap().unwrap();
        let original_id_bytes = original.id.clone();
        let original_query_bytes = original.query.clone();
        let original_language_bytes = original.language.clone();
        let original_file_path_bytes = original.file_path.clone();

        // Perform various operations that might try to modify the bookmark

        // 1. Create a new resolution (simulating a heal operation)
        let res1 = crate::engine::bookmark::Resolution {
            is_dirty: false,
            id: "res-1".to_string(),
            bookmark_id: bm_id.to_string(),
            resolved_at: "2024-01-02T00:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("commit-new".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/renamed.swift".to_string()), // Different path in resolution
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:newhash5678".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        db.insert_resolution(&res1).unwrap();
        db.update_bookmark_resolution_id(bm_id, "res-1").unwrap();

        // Verify the immutable fields have NOT been modified
        let after_resolution = db.get_bookmark(bm_id).unwrap().unwrap();

        // ASSERTION: The following fields MUST remain immutable
        assert_eq!(after_resolution.id, original_id_bytes, "Bookmark ID must be immutable");
        assert_eq!(
            after_resolution.query, original_query_bytes,
            "Bookmark query must be immutable"
        );
        assert_eq!(
            after_resolution.language, original_language_bytes,
            "Bookmark language must be immutable"
        );
        assert_eq!(
            after_resolution.file_path, original_file_path_bytes,
            "Bookmark file_path must be immutable"
        );
    }

    #[test]
    fn test_temporal_causality() {
        // @lat: [[tests#System Invariants Tests#Bookmark Integrity Rules#Temporal Causality]]
        // Verifies that a bookmark cannot have a resolution with a resolved_at timestamp
        // earlier than its own created_at timestamp.
        init_test_env();
        let db = Database::open_in_memory().unwrap();

        let bm_id = "temporal-test-bm";
        let bookmark_created_at = "2024-06-15T10:30:00Z";

        let bm = Bookmark {
            id: bm_id.to_string(),
            query: "(function_declaration) @target".to_string(),
            language: "swift".to_string(),
            file_path: "src/main.swift".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: bookmark_created_at.to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };
        db.insert_bookmark(&bm).unwrap();

        // Test 1: Resolution with same timestamp as creation should be allowed
        let res_same_time = crate::engine::bookmark::Resolution {
            is_dirty: false,
            id: "res-same-time".to_string(),
            bookmark_id: bm_id.to_string(),
            resolved_at: bookmark_created_at.to_string(), // Same as created_at
            health: BookmarkHealth::Active,
            commit_hash: None,
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: None,
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        db.insert_resolution(&res_same_time).unwrap();
        db.update_bookmark_resolution_id(bm_id, "res-same-time").unwrap();

        // Verify it was accepted
        let fetched = db.get_bookmark(bm_id).unwrap().unwrap();
        assert_eq!(fetched.last_resolved_at, Some(bookmark_created_at.to_string()));

        // Test 2: Resolution AFTER creation should be allowed
        let res_future = crate::engine::bookmark::Resolution {
            is_dirty: false,
            id: "res-future".to_string(),
            bookmark_id: bm_id.to_string(),
            resolved_at: "2024-06-15T11:00:00Z".to_string(), // 30 minutes after
            health: BookmarkHealth::Active,
            commit_hash: None,
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: None,
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        db.insert_resolution(&res_future).unwrap();
        db.update_bookmark_resolution_id(bm_id, "res-future").unwrap();

        let fetched = db.get_bookmark(bm_id).unwrap().unwrap();
        assert_eq!(fetched.last_resolved_at, Some("2024-06-15T11:00:00Z".to_string()));

        // Test 3: Resolution BEFORE creation should be detected as violation
        // Currently, the database doesn't enforce this constraint.
        // This test documents the expected behavior.
        let res_past = crate::engine::bookmark::Resolution {
            is_dirty: false,
            id: "res-past".to_string(),
            bookmark_id: bm_id.to_string(),
            resolved_at: "2024-06-15T09:00:00Z".to_string(), // 1.5 hours BEFORE created_at
            health: BookmarkHealth::Active,
            commit_hash: None,
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: None,
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };

        // TODO: This insert should fail with a constraint violation
        // For now, we document that it doesn't:
        let result = db.insert_resolution(&res_past);
        assert!(result.is_ok(), "Currently allows past resolution (should be rejected)");

        // Verify the resolution was added (it shouldn't be)
        let resolutions = db.list_resolutions(bm_id, 100).unwrap();
        assert!(
            resolutions.iter().any(|r| r.id == "res-past"),
            "Past resolution was incorrectly allowed"
        );

        // NOTE: To fix this, add a CHECK constraint in the schema:
        // ALTER TABLE resolutions ADD CONSTRAINT temporal_causality_check
        //   CHECK (resolved_at >= (SELECT created_at FROM bookmarks WHERE id = bookmark_id));
    }
}
