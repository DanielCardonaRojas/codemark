use std::collections::HashMap;

use crate::engine::bookmark::{
    Annotation, Bookmark, BookmarkFilter, BookmarkStatus, ResolutionMethod, Tag,
};
use crate::error::{Error, Result};
use crate::storage::db::Database;

impl Database {
    /// Insert a bookmark, returning the bookmark ID.
    /// If a bookmark with the same file_path and query exists, returns its ID instead.
    pub fn insert_bookmark(&self, bookmark: &Bookmark) -> Result<String> {
        // Try to insert with OR FAIL to check for uniqueness constraint
        let result = self.conn().execute(
            "INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
             status, resolution_method, last_resolved_at, stale_since, created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                bookmark.id,
                bookmark.query,
                bookmark.language,
                bookmark.file_path,
                bookmark.content_hash,
                bookmark.commit_hash,
                bookmark.status.to_string(),
                bookmark.resolution_method.map(|m| m.to_string()),
                bookmark.last_resolved_at,
                bookmark.stale_since,
                bookmark.created_at,
                bookmark.created_by,
            ],
        );

        match result {
            Ok(_) => Ok(bookmark.id.clone()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                // Bookmark with same file_path and query exists, fetch its ID
                let existing_id: String = self.conn().query_row(
                    "SELECT id FROM bookmarks WHERE file_path = ?1 AND query = ?2",
                    rusqlite::params![bookmark.file_path, bookmark.query],
                    |row| row.get(0),
                )?;
                Ok(existing_id)
            }
            Err(e) => Err(e.into()),
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
            "SELECT id, query, language, file_path, content_hash, commit_hash,
             status, resolution_method, last_resolved_at, stale_since, created_at, created_by
             FROM bookmarks WHERE id = ?1",
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
            "SELECT id, query, language, file_path, content_hash, commit_hash,
             status, resolution_method, last_resolved_at, stale_since, created_at, created_by
             FROM bookmarks WHERE id LIKE ?1",
        )?;
        let mut results: Vec<Bookmark> =
            stmt.query_map([&pattern], row_to_bookmark_base)?.filter_map(|r| r.ok()).collect();

        // Load metadata for each result
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
            "SELECT id, query, language, file_path, content_hash, commit_hash,
             status, resolution_method, last_resolved_at, stale_since, created_at, created_by
             FROM bookmarks WHERE file_path = ?1 AND query = ?2",
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
             b.status, b.resolution_method, b.last_resolved_at, b.stale_since, b.created_at, b.created_by
             FROM bookmarks b",
        );
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if filter.collection.is_some() {
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

        if let Some(ref statuses) = filter.status {
            let placeholders: Vec<String> =
                statuses.iter().enumerate().map(|_| "?".to_string()).collect();
            conditions.push(format!("b.status IN ({})", placeholders.join(", ")));
            for s in statuses {
                params.push(Box::new(s.to_string()));
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

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        if filter.collection.is_some() {
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

        // Load metadata for all bookmarks
        for bm in &mut results {
            self.load_bookmark_metadata(bm)?;
        }

        Ok(results)
    }

    pub fn update_bookmark_status(
        &self,
        id: &str,
        status: BookmarkStatus,
        method: Option<ResolutionMethod>,
        last_resolved_at: Option<&str>,
        stale_since: Option<&str>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE bookmarks SET status = ?1, resolution_method = ?2,
             last_resolved_at = ?3, stale_since = ?4 WHERE id = ?5",
            rusqlite::params![
                status.to_string(),
                method.map(|m| m.to_string()),
                last_resolved_at,
                stale_since,
                id,
            ],
        )?;
        Ok(())
    }

    pub fn update_bookmark_query(
        &self,
        id: &str,
        query: &str,
        file_path: &str,
        content_hash: &str,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE bookmarks SET query = ?1, file_path = ?2, content_hash = ?3 WHERE id = ?4",
            rusqlite::params![query, file_path, content_hash, id],
        )?;
        Ok(())
    }

    pub fn delete_bookmark(&self, id: &str) -> Result<bool> {
        let count = self.conn().execute("DELETE FROM bookmarks WHERE id = ?1", [id])?;
        Ok(count > 0)
    }

    pub fn count_by_status(&self) -> Result<HashMap<BookmarkStatus, usize>> {
        let mut stmt =
            self.conn().prepare("SELECT status, COUNT(*) FROM bookmarks GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            let status: String = row.get(0)?;
            let count: usize = row.get(1)?;
            Ok((status, count))
        })?;

        let mut map = HashMap::new();
        for row in rows {
            let (status_str, count) = row?;
            if let Ok(status) = status_str.parse::<BookmarkStatus>() {
                map.insert(status, count);
            }
        }
        Ok(map)
    }

    pub fn delete_archived_before(&self, before: &str) -> Result<usize> {
        let count = self.conn().execute(
            "DELETE FROM bookmarks WHERE status = 'archived' AND created_at < ?1",
            [before],
        )?;
        Ok(count)
    }

    /// Search bookmarks by text in notes and/or context fields.
    pub fn search_bookmarks(
        &self,
        query: Option<&str>,
        note: Option<&str>,
        context: Option<&str>,
        language: Option<&str>,
        created_by: Option<&str>,
        collection: Option<&str>,
    ) -> Result<Vec<Bookmark>> {
        // Always need annotation join for general search
        // Always need tag join for tag search
        let mut sql = String::from(
            "SELECT DISTINCT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             b.status, b.resolution_method, b.last_resolved_at, b.stale_since, b.created_at, b.created_by
             FROM bookmarks b
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

        // Exclude archived by default
        conditions.push("b.status != 'archived'".to_string());

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

        // Load metadata for all bookmarks
        for bm in &mut results {
            self.load_bookmark_metadata(bm)?;
        }

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

        Ok(())
    }
}

fn row_to_bookmark_base(row: &rusqlite::Row) -> rusqlite::Result<Bookmark> {
    let status_str: String = row.get(6)?;
    let method_str: Option<String> = row.get(7)?;

    Ok(Bookmark {
        id: row.get(0)?,
        query: row.get(1)?,
        language: row.get(2)?,
        file_path: row.get(3)?,
        content_hash: row.get(4)?,
        commit_hash: row.get(5)?,
        status: status_str.parse().unwrap_or(BookmarkStatus::Active),
        resolution_method: method_str.and_then(|s| s.parse().ok()),
        last_resolved_at: row.get(8)?,
        stale_since: row.get(9)?,
        created_at: row.get(10)?,
        created_by: row.get(11)?,
        tags: Vec::new(),        // Loaded separately
        annotations: Vec::new(), // Loaded separately
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
    use crate::engine::bookmark::{Annotation, Bookmark, BookmarkStatus, ResolutionMethod, Tag};
    use crate::storage::db::Database;

    // Initialize sqlite-vec extension for all tests
    fn init_test_env() {
        crate::embeddings::VecStore::init_extension();
    }

    fn test_bookmark(id: &str) -> Bookmark {
        // Use unique file_path and query to avoid UNIQUE constraint violations
        Bookmark {
            id: id.to_string(),
            query: format!("(function_declaration) @{} /* {} */", "target", id),
            language: "swift".to_string(),
            file_path: format!("src/main_{}.swift", id),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            commit_hash: Some("abc123".to_string()),
            status: BookmarkStatus::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec![],
            annotations: vec![],
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
        assert_eq!(fetched.status, BookmarkStatus::Active);
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
    fn list_with_status_filter() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let mut bm1 = test_bookmark("aaaa-0000-0000-0001");
        bm1.status = BookmarkStatus::Active;
        let mut bm2 = test_bookmark("aaaa-0000-0000-0002");
        bm2.status = BookmarkStatus::Stale;
        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        let filter =
            BookmarkFilter { status: Some(vec![BookmarkStatus::Stale]), ..Default::default() };
        let results = db.list_bookmarks(&filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "aaaa-0000-0000-0002");
    }

    #[test]
    fn update_status() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-0000-0000-0001")).unwrap();

        db.update_bookmark_status(
            "aaaa-0000-0000-0001",
            BookmarkStatus::Drifted,
            Some(ResolutionMethod::Relaxed),
            Some("2026-04-01T01:00:00Z"),
            None,
        )
        .unwrap();

        let bm = db.get_bookmark("aaaa-0000-0000-0001").unwrap().unwrap();
        assert_eq!(bm.status, BookmarkStatus::Drifted);
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
    fn count_by_status() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let mut bm1 = test_bookmark("aaaa-0000-0000-0001");
        bm1.status = BookmarkStatus::Active;
        let mut bm2 = test_bookmark("aaaa-0000-0000-0002");
        bm2.status = BookmarkStatus::Stale;
        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        let counts = db.count_by_status().unwrap();
        assert_eq!(counts.get(&BookmarkStatus::Active), Some(&1));
        assert_eq!(counts.get(&BookmarkStatus::Stale), Some(&1));
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
}
