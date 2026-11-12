use crate::engine::bookmark::BookmarkComment;
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    /// Insert a new bookmark comment.
    pub fn insert_comment(&self, comment: &BookmarkComment) -> Result<()> {
        self.conn().execute(
            "INSERT INTO bookmark_comments
                (id, bookmark_id, author, body, created_at, parent_id, query, content_hash, anchor_hint)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                comment.id,
                comment.bookmark_id,
                comment.author,
                comment.body,
                comment.created_at,
                comment.parent_id,
                comment.query,
                comment.content_hash,
                comment.anchor_hint,
            ],
        )?;
        Ok(())
    }

    /// Insert multiple bookmark comments atomically.
    pub fn insert_comments(&self, comments: &[BookmarkComment]) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        for comment in comments {
            tx.execute(
                "INSERT INTO bookmark_comments
                    (id, bookmark_id, author, body, created_at, parent_id, query, content_hash, anchor_hint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    comment.id,
                    comment.bookmark_id,
                    comment.author,
                    comment.body,
                    comment.created_at,
                    comment.parent_id,
                    comment.query,
                    comment.content_hash,
                    comment.anchor_hint,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// List all comments for a specific bookmark.
    pub fn list_comments_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<BookmarkComment>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, author, body, created_at, parent_id, query, content_hash, anchor_hint
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
                query: row.get(6)?,
                content_hash: row.get(7)?,
                anchor_hint: row.get(8)?,
            })
        })?;

        let results: Vec<BookmarkComment> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Get a comment by its ID.
    pub fn get_comment(&self, id: &str) -> Result<Option<BookmarkComment>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, author, body, created_at, parent_id, query, content_hash, anchor_hint
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
                query: row.get(6)?,
                content_hash: row.get(7)?,
                anchor_hint: row.get(8)?,
            })
        })?;

        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get a comment by its full id or an unambiguous id prefix. The prefix
    /// is matched literally (`_` and `%` are not wildcards); an empty one is
    /// rejected rather than matching every comment.
    pub fn get_comment_by_prefix(&self, prefix: &str) -> Result<Option<BookmarkComment>> {
        if prefix.is_empty() {
            return Err(crate::error::Error::Input("comment id must not be empty".into()));
        }
        if let Some(comment) = self.get_comment(prefix)? {
            return Ok(Some(comment));
        }
        let ids: Vec<String> = self
            .conn()
            .prepare(
                "SELECT id FROM bookmark_comments WHERE substr(id, 1, length(?1)) = ?1 LIMIT 2",
            )?
            .query_map([prefix], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        match ids.as_slice() {
            [id] => self.get_comment(id),
            [] => Ok(None),
            _ => Err(crate::error::Error::Input(format!(
                "comment id prefix '{prefix}' is ambiguous; use more characters"
            ))),
        }
    }

    /// Replace a comment's anchor, or detach it with `None` to make it a
    /// whole-bookmark comment. Detaching keeps `anchor_hint`, so the comment
    /// can still say where it used to point.
    pub fn update_comment_anchor(
        &self,
        id: &str,
        anchor: Option<&crate::engine::comment_anchor::CommentAnchor>,
    ) -> Result<()> {
        self.conn().execute(
            "UPDATE bookmark_comments
             SET query = ?2, content_hash = ?3, anchor_hint = COALESCE(?4, anchor_hint)
             WHERE id = ?1",
            rusqlite::params![
                id,
                anchor.map(|a| a.query.as_str()),
                anchor.map(|a| a.content_hash.as_str()),
                anchor.map(|a| a.anchor_hint.as_str()),
            ],
        )?;
        Ok(())
    }

    /// Delete a comment and its replies, returning how many rows went.
    pub fn delete_comment_thread(&self, id: &str) -> Result<usize> {
        let tx = self.conn().unchecked_transaction()?;
        let replies = tx.execute("DELETE FROM bookmark_comments WHERE parent_id = ?1", [id])?;
        let root = tx.execute("DELETE FROM bookmark_comments WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(replies + root)
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
            grammar_source: None,
            forked_from: None,
            branch: None,
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
            query: None,
            content_hash: None,
            anchor_hint: None,
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

    #[test]
    fn anchor_fields_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-1")).unwrap();

        let anchored = BookmarkComment {
            query: Some("(call_expression) @target".into()),
            content_hash: Some("abc123".into()),
            anchor_hint: Some("L5-L5: helper(2)".into()),
            ..test_comment("c-1", "bm-1", None)
        };
        db.insert_comment(&anchored).unwrap();
        db.insert_comments(&[test_comment("c-2", "bm-1", Some("c-1"))]).unwrap();

        let comments = db.list_comments_for_bookmark("bm-1").unwrap();
        assert_eq!(comments[0], anchored);
        assert!(comments[0].is_anchored());
        assert!(!comments[1].is_anchored());
        assert_eq!(db.get_comment("c-1").unwrap().unwrap(), anchored);
    }

    #[test]
    fn parents_first_orders_nested_replies_after_their_parents() {
        // A reply to a reply listed before its parent, as a pack may hold.
        let deep = test_comment("c-3", "bm-1", Some("c-2"));
        let reply = test_comment("c-2", "bm-1", Some("c-1"));
        let root = test_comment("c-1", "bm-1", None);
        let outside = test_comment("c-4", "bm-1", Some("not-in-batch"));
        let ordered = BookmarkComment::parents_first(vec![deep, reply, outside, root]).unwrap();
        let ids: Vec<&str> = ordered.iter().map(|c| c.id.as_str()).collect();
        let pos = |id: &str| ids.iter().position(|i| *i == id).unwrap();
        assert!(pos("c-1") < pos("c-2") && pos("c-2") < pos("c-3"), "{ids:?}");
        assert!(ids.contains(&"c-4"), "a parent outside the batch counts as present");

        let cycle =
            vec![test_comment("a", "bm-1", Some("b")), test_comment("b", "bm-1", Some("a"))];
        assert!(BookmarkComment::parents_first(cycle).is_err());
    }

    #[test]
    fn prefix_lookup_and_anchor_update() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-1")).unwrap();
        db.insert_comment(&test_comment("abc-1", "bm-1", None)).unwrap();
        db.insert_comment(&test_comment("abd-2", "bm-1", None)).unwrap();

        assert_eq!(db.get_comment_by_prefix("abc").unwrap().unwrap().id, "abc-1");
        assert!(db.get_comment_by_prefix("ab").is_err(), "ambiguous prefix");
        assert!(db.get_comment_by_prefix("zz").unwrap().is_none());
        // Prefixes are literal: `_` and `%` are not LIKE wildcards here.
        assert!(db.get_comment_by_prefix("ab_").unwrap().is_none());
        assert!(db.get_comment_by_prefix("%").unwrap().is_none());
        assert!(db.get_comment_by_prefix("").is_err());

        let anchor = crate::engine::comment_anchor::CommentAnchor {
            query: "(call_expression) @target".into(),
            content_hash: "h".into(),
            anchor_hint: "L1-L1: f()".into(),
        };
        db.update_comment_anchor("abc-1", Some(&anchor)).unwrap();
        assert!(db.get_comment("abc-1").unwrap().unwrap().is_anchored());
        db.update_comment_anchor("abc-1", None).unwrap();
        let detached = db.get_comment("abc-1").unwrap().unwrap();
        assert!(!detached.is_anchored());
        assert_eq!(detached.anchor_hint.as_deref(), Some("L1-L1: f()"), "hint survives detach");
    }
}
