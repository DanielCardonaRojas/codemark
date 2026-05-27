use crate::engine::bookmark::BookmarkComment;
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    /// Insert a new bookmark comment.
    pub fn insert_comment(&self, comment: &BookmarkComment) -> Result<()> {
        self.conn().execute(
            "INSERT INTO bookmark_comments (id, bookmark_id, author, body, created_at, parent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                comment.id,
                comment.bookmark_id,
                comment.author,
                comment.body,
                comment.created_at,
                comment.parent_id,
            ],
        )?;
        Ok(())
    }

    /// List all comments for a specific bookmark.
    pub fn list_comments_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<BookmarkComment>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, author, body, created_at, parent_id
             FROM bookmark_comments
             WHERE bookmark_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([bookmark_id], |row| {
            Ok(BookmarkComment {
                id: row.get(0)?,
                bookmark_id: row.get(1)?,
                author: row.get(2)?,
                body: row.get(3)?,
                created_at: row.get(4)?,
                parent_id: row.get(5)?,
            })
        })?;

        let results: Vec<BookmarkComment> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Get a comment by its ID.
    pub fn get_comment(&self, id: &str) -> Result<Option<BookmarkComment>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, author, body, created_at, parent_id
             FROM bookmark_comments
             WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], |row| {
            Ok(BookmarkComment {
                id: row.get(0)?,
                bookmark_id: row.get(1)?,
                author: row.get(2)?,
                body: row.get(3)?,
                created_at: row.get(4)?,
                parent_id: row.get(5)?,
            })
        })?;

        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Delete a comment by its ID.
    pub fn delete_comment(&self, id: &str) -> Result<()> {
        self.conn().execute("DELETE FROM bookmark_comments WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::{Bookmark, BookmarkComment, BookmarkHealth};
    use crate::storage::db::Database;

    fn test_bookmark(id: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: format!("(function_declaration) @{} /* {} */", "target", id),
            language: "swift".to_string(),
            file_path: format!("src/main_{}.swift", id),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            ui_status: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![],
        }
    }

    fn test_comment(id: &str, bookmark_id: &str, parent_id: Option<&str>) -> BookmarkComment {
        BookmarkComment {
            id: id.to_string(),
            bookmark_id: bookmark_id.to_string(),
            author: "test-user".to_string(),
            body: "This is a test comment".to_string(),
            created_at: "2026-04-01T10:00:00Z".to_string(),
            parent_id: parent_id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn insert_and_list_comments() {
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("bm-1");
        db.insert_bookmark(&bm).unwrap();

        let c1 = test_comment("c-1", "bm-1", None);
        db.insert_comment(&c1).unwrap();

        let c2 = test_comment("c-2", "bm-1", Some("c-1"));
        db.insert_comment(&c2).unwrap();

        let comments = db.list_comments_for_bookmark("bm-1").unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].id, "c-1");
        assert_eq!(comments[1].id, "c-2");
        assert_eq!(comments[1].parent_id, Some("c-1".to_string()));
    }

    #[test]
    fn get_and_delete_comment() {
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("bm-1");
        db.insert_bookmark(&bm).unwrap();

        let c1 = test_comment("c-1", "bm-1", None);
        db.insert_comment(&c1).unwrap();

        let fetched = db.get_comment("c-1").unwrap().unwrap();
        assert_eq!(fetched.body, "This is a test comment");

        db.delete_comment("c-1").unwrap();
        assert!(db.get_comment("c-1").unwrap().is_none());
    }

    #[test]
    fn delete_bookmark_cascades_to_comments() {
        let db = Database::open_in_memory().unwrap();
        let bm = test_bookmark("bm-1");
        db.insert_bookmark(&bm).unwrap();

        let c1 = test_comment("c-1", "bm-1", None);
        db.insert_comment(&c1).unwrap();

        db.delete_bookmark("bm-1").unwrap();
        assert!(db.get_comment("c-1").unwrap().is_none());
    }
}
