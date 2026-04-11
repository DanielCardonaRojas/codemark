use std::collections::HashMap;

use crate::engine::bookmark::{
    Bookmark, BookmarkFilter, BookmarkStatus, ResolutionMethod, tags_from_json, tags_to_json,
};
use crate::error::{Error, Result};
use crate::storage::db::Database;

impl Database {
    pub fn insert_bookmark(&self, bookmark: &Bookmark) -> Result<()> {
        self.conn().execute(
            "INSERT INTO bookmarks (id, query, language, file_path, content_hash, commit_hash,
             status, resolution_method, last_resolved_at, stale_since, created_at, created_by,
             tags, notes, context)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                tags_to_json(&bookmark.tags),
                bookmark.notes,
                bookmark.context,
            ],
        )?;
        Ok(())
    }

    pub fn get_bookmark(&self, id: &str) -> Result<Option<Bookmark>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, query, language, file_path, content_hash, commit_hash,
             status, resolution_method, last_resolved_at, stale_since, created_at,
             created_by, tags, notes, context
             FROM bookmarks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_bookmark)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
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
             status, resolution_method, last_resolved_at, stale_since, created_at,
             created_by, tags, notes, context
             FROM bookmarks WHERE id LIKE ?1",
        )?;
        let results: Vec<Bookmark> =
            stmt.query_map([&pattern], row_to_bookmark)?.filter_map(|r| r.ok()).collect();

        match results.len() {
            0 => Ok(None),
            1 => Ok(Some(results.into_iter().next().unwrap())),
            _ => Err(Error::Input(format!(
                "ambiguous bookmark ID prefix '{prefix}': matches {} bookmarks",
                results.len()
            ))),
        }
    }

    pub fn list_bookmarks(&self, filter: &BookmarkFilter) -> Result<Vec<Bookmark>> {
        let mut sql = String::from(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             b.status, b.resolution_method, b.last_resolved_at, b.stale_since, b.created_at,
             b.created_by, b.tags, b.notes, b.context
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
            conditions.push("EXISTS (SELECT 1 FROM json_each(b.tags) WHERE value = ?)".to_string());
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
        let rows = stmt.query_map(param_refs.as_slice(), row_to_bookmark)?;
        let results: Vec<Bookmark> = rows.filter_map(|r| r.ok()).collect();
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
        let mut sql = String::from(
            "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
             b.status, b.resolution_method, b.last_resolved_at, b.stale_since, b.created_at,
             b.created_by, b.tags, b.notes, b.context
             FROM bookmarks b",
        );
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if collection.is_some() {
            sql.push_str(
                " JOIN collection_bookmarks cb ON b.id = cb.bookmark_id
                 JOIN collections c ON cb.collection_id = c.id",
            );
        }

        // Use FTS5 for text search — join on rowid
        let has_text_search = query.is_some() || note.is_some() || context.is_some();
        if has_text_search {
            sql.push_str(" JOIN bookmarks_fts fts ON b.rowid = fts.rowid");
        }

        if let Some(q) = query {
            // FTS5 MATCH searches both notes and context columns
            conditions.push("bookmarks_fts MATCH ?".to_string());
            params.push(Box::new(fts_escape(q)));
        }
        if let Some(n) = note {
            conditions.push("bookmarks_fts MATCH ?".to_string());
            params.push(Box::new(format!("notes:{}", fts_escape(n))));
        }
        if let Some(ctx) = context {
            conditions.push("bookmarks_fts MATCH ?".to_string());
            params.push(Box::new(format!("context:{}", fts_escape(ctx))));
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
        let rows = stmt.query_map(param_refs.as_slice(), row_to_bookmark)?;
        let results: Vec<Bookmark> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }
}

/// Escape a user query for FTS5 MATCH safety.
/// Wraps each term in double quotes to prevent FTS5 syntax injection.
fn fts_escape(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn row_to_bookmark(row: &rusqlite::Row) -> rusqlite::Result<Bookmark> {
    let status_str: String = row.get(6)?;
    let method_str: Option<String> = row.get(7)?;
    let tags_json: Option<String> = row.get(12)?;

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
        tags: tags_json.map(|j| tags_from_json(&j)).unwrap_or_default(),
        notes: row.get(13)?,
        context: row.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::BookmarkFilter;
    use crate::engine::bookmark::{Bookmark, BookmarkStatus, ResolutionMethod};
    use crate::storage::db::Database;

    fn test_bookmark(id: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: "(function_declaration) @target".to_string(),
            language: "swift".to_string(),
            file_path: "src/main.swift".to_string(),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            commit_hash: Some("abc123".to_string()),
            status: BookmarkStatus::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec!["auth".to_string(), "api".to_string()],
            notes: Some("test note".to_string()),
            context: None,
        }
    }

    #[test]
    fn insert_and_get_bookmark() {
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("aaaa-bbbb-cccc-dddd");
        db.insert_bookmark(&bm).unwrap();

        let fetched = db.get_bookmark("aaaa-bbbb-cccc-dddd").unwrap().unwrap();
        assert_eq!(fetched.id, bm.id);
        assert_eq!(fetched.query, bm.query);
        assert_eq!(fetched.status, BookmarkStatus::Active);
        assert_eq!(fetched.tags, vec!["auth", "api"]);
        assert_eq!(fetched.notes, Some("test note".to_string()));
    }

    #[test]
    fn get_bookmark_by_prefix() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-1111-2222-3333")).unwrap();
        db.insert_bookmark(&test_bookmark("bbbb-1111-2222-3333")).unwrap();

        let found = db.get_bookmark_by_prefix("aaaa").unwrap().unwrap();
        assert_eq!(found.id, "aaaa-1111-2222-3333");

        assert!(db.get_bookmark_by_prefix("cccc").unwrap().is_none());
    }

    #[test]
    fn prefix_too_short_errors() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_bookmark_by_prefix("aa").is_err());
    }

    #[test]
    fn ambiguous_prefix_errors() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-1111-0000-0000")).unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-2222-0000-0000")).unwrap();

        let result = db.get_bookmark_by_prefix("aaaa");
        assert!(result.is_err());
    }

    #[test]
    fn list_with_tag_filter() {
        let db = Database::open_in_memory().unwrap();
        let mut bm1 = test_bookmark("aaaa-0000-0000-0001");
        bm1.tags = vec!["auth".to_string()];
        let mut bm2 = test_bookmark("aaaa-0000-0000-0002");
        bm2.tags = vec!["api".to_string()];
        db.insert_bookmark(&bm1).unwrap();
        db.insert_bookmark(&bm2).unwrap();

        let filter = BookmarkFilter { tag: Some("auth".into()), ..Default::default() };
        let results = db.list_bookmarks(&filter).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "aaaa-0000-0000-0001");
    }

    #[test]
    fn list_with_status_filter() {
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
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("aaaa-0000-0000-0001")).unwrap();
        assert!(db.delete_bookmark("aaaa-0000-0000-0001").unwrap());
        assert!(db.get_bookmark("aaaa-0000-0000-0001").unwrap().is_none());
    }

    #[test]
    fn count_by_status() {
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
}
