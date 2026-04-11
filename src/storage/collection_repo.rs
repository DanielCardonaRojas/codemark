use crate::engine::bookmark::Collection;
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    pub fn insert_collection(&self, collection: &Collection) -> Result<()> {
        self.conn().execute(
            "INSERT INTO collections (id, name, description, created_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                collection.id,
                collection.name,
                collection.description,
                collection.created_at,
                collection.created_by,
            ],
        )?;
        Ok(())
    }

    pub fn get_collection_by_name(&self, name: &str) -> Result<Option<Collection>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, description, created_at, created_by
             FROM collections WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map([name], row_to_collection)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// List all collections with their bookmark counts.
    pub fn list_collections(&self) -> Result<Vec<(Collection, usize)>> {
        let mut stmt = self.conn().prepare(
            "SELECT c.id, c.name, c.description, c.created_at, c.created_by,
             COUNT(cb.bookmark_id) AS bookmark_count
             FROM collections c
             LEFT JOIN collection_bookmarks cb ON c.id = cb.collection_id
             GROUP BY c.id
             ORDER BY c.name",
        )?;
        let rows = stmt.query_map([], |row| {
            let collection = Collection {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
                created_by: row.get(4)?,
            };
            let count: usize = row.get(5)?;
            Ok((collection, count))
        })?;

        let results: Vec<(Collection, usize)> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Delete a collection, returning the number of bookmarks that were in it.
    pub fn delete_collection(&self, name: &str) -> Result<usize> {
        let count: usize = self
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM collection_bookmarks cb
                 JOIN collections c ON cb.collection_id = c.id
                 WHERE c.name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap_or(0);

        self.conn().execute("DELETE FROM collections WHERE name = ?1", [name])?;

        Ok(count)
    }

    /// Add bookmarks to a collection, appending at the end (or at a specific position).
    /// Returns the number actually added (skips duplicates).
    pub fn add_to_collection(&self, collection_id: &str, bookmark_ids: &[String]) -> Result<usize> {
        self.add_to_collection_at(collection_id, bookmark_ids, None)
    }

    /// Add bookmarks at a specific position (0-indexed). Existing items at >= position are shifted.
    /// If `at` is None, appends at the end.
    pub fn add_to_collection_at(
        &self,
        collection_id: &str,
        bookmark_ids: &[String],
        at: Option<usize>,
    ) -> Result<usize> {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

        let insert_pos = if let Some(pos) = at {
            // Shift existing items to make room
            self.conn().execute(
                "UPDATE collection_bookmarks SET position = position + ?1
                 WHERE collection_id = ?2 AND position >= ?3",
                rusqlite::params![bookmark_ids.len() as i64, collection_id, pos as i64],
            )?;
            pos
        } else {
            // Append: get current max position + 1
            let max_pos: i64 = self
                .conn()
                .query_row(
                    "SELECT COALESCE(MAX(position), -1) FROM collection_bookmarks WHERE collection_id = ?1",
                    [collection_id],
                    |row| row.get(0),
                )
                .unwrap_or(-1);
            (max_pos + 1) as usize
        };

        let mut added = 0;
        for (i, bm_id) in bookmark_ids.iter().enumerate() {
            let result = self.conn().execute(
                "INSERT OR IGNORE INTO collection_bookmarks (collection_id, bookmark_id, added_at, position)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![collection_id, bm_id, now, (insert_pos + i) as i64],
            );
            if let Ok(n) = result {
                added += n;
            }
        }
        Ok(added)
    }

    /// Reorder bookmarks in a collection. The `ordered_ids` list defines the new order.
    /// Bookmarks not in the list keep their relative order after the listed ones.
    pub fn reorder_collection(&self, collection_id: &str, ordered_ids: &[String]) -> Result<()> {
        for (i, bm_id) in ordered_ids.iter().enumerate() {
            self.conn().execute(
                "UPDATE collection_bookmarks SET position = ?1
                 WHERE collection_id = ?2 AND bookmark_id = ?3",
                rusqlite::params![i as i64, collection_id, bm_id],
            )?;
        }
        Ok(())
    }

    /// Remove bookmarks from a collection. Returns the number actually removed.
    pub fn remove_from_collection(
        &self,
        collection_id: &str,
        bookmark_ids: &[String],
    ) -> Result<usize> {
        let mut removed = 0;
        for bm_id in bookmark_ids {
            let n = self.conn().execute(
                "DELETE FROM collection_bookmarks
                 WHERE collection_id = ?1 AND bookmark_id = ?2",
                rusqlite::params![collection_id, bm_id],
            )?;
            removed += n;
        }
        Ok(removed)
    }

    pub fn list_collections_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<Collection>> {
        let mut stmt = self.conn().prepare(
            "SELECT c.id, c.name, c.description, c.created_at, c.created_by
             FROM collections c
             JOIN collection_bookmarks cb ON c.id = cb.collection_id
             WHERE cb.bookmark_id = ?1
             ORDER BY c.name",
        )?;
        let rows = stmt.query_map([bookmark_id], row_to_collection)?;
        let results: Vec<Collection> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }
}

fn row_to_collection(row: &rusqlite::Row) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        created_at: row.get(3)?,
        created_by: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::{Bookmark, BookmarkStatus, Collection};
    use crate::storage::db::Database;

    fn test_bookmark(id: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: "(function_declaration) @target".to_string(),
            language: "swift".to_string(),
            file_path: "src/main.swift".to_string(),
            content_hash: None,
            commit_hash: None,
            status: BookmarkStatus::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
            tags: vec![],
            notes: None,
            context: None,
        }
    }

    fn test_collection(name: &str) -> Collection {
        Collection {
            id: format!("col-{name}"),
            name: name.to_string(),
            description: Some(format!("Test collection {name}")),
            created_at: "2026-04-01T00:00:00Z".to_string(),
            created_by: None,
        }
    }

    #[test]
    fn create_and_get_collection() {
        let db = Database::open_in_memory().unwrap();
        let col = test_collection("bugfix-auth");
        db.insert_collection(&col).unwrap();

        let fetched = db.get_collection_by_name("bugfix-auth").unwrap().unwrap();
        assert_eq!(fetched.name, "bugfix-auth");
        assert_eq!(fetched.description, Some("Test collection bugfix-auth".to_string()));
    }

    #[test]
    fn add_bookmarks_to_collection() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0002")).unwrap();
        let col = test_collection("sprint-1");
        db.insert_collection(&col).unwrap();

        let added =
            db.add_to_collection("col-sprint-1", &["bm-0001".into(), "bm-0002".into()]).unwrap();
        assert_eq!(added, 2);

        // Duplicate add is silently skipped
        let added = db.add_to_collection("col-sprint-1", &["bm-0001".into()]).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn list_collections_with_counts() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_collection(&test_collection("col-a")).unwrap();
        db.insert_collection(&test_collection("col-b")).unwrap();
        db.add_to_collection("col-col-a", &["bm-0001".into()]).unwrap();

        let collections = db.list_collections().unwrap();
        assert_eq!(collections.len(), 2);
        // col-a has 1 bookmark, col-b has 0
        let (col_a, count_a) = collections.iter().find(|(c, _)| c.name == "col-a").unwrap();
        assert_eq!(*count_a, 1);
        assert_eq!(col_a.name, "col-a");
        let (_, count_b) = collections.iter().find(|(c, _)| c.name == "col-b").unwrap();
        assert_eq!(*count_b, 0);
    }

    #[test]
    fn delete_collection_preserves_bookmarks() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_collection(&test_collection("temp")).unwrap();
        db.add_to_collection("col-temp", &["bm-0001".into()]).unwrap();

        let removed_count = db.delete_collection("temp").unwrap();
        assert_eq!(removed_count, 1);

        // Collection is gone
        assert!(db.get_collection_by_name("temp").unwrap().is_none());
        // Bookmark still exists
        assert!(db.get_bookmark("bm-0001").unwrap().is_some());
    }

    #[test]
    fn remove_from_collection() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_collection(&test_collection("sprint-1")).unwrap();
        db.add_to_collection("col-sprint-1", &["bm-0001".into()]).unwrap();

        let removed = db.remove_from_collection("col-sprint-1", &["bm-0001".into()]).unwrap();
        assert_eq!(removed, 1);

        let collections = db.list_collections_for_bookmark("bm-0001").unwrap();
        assert!(collections.is_empty());
    }

    #[test]
    fn list_collections_for_bookmark() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_collection(&test_collection("col-a")).unwrap();
        db.insert_collection(&test_collection("col-b")).unwrap();
        db.add_to_collection("col-col-a", &["bm-0001".into()]).unwrap();
        db.add_to_collection("col-col-b", &["bm-0001".into()]).unwrap();

        let collections = db.list_collections_for_bookmark("bm-0001").unwrap();
        assert_eq!(collections.len(), 2);
    }

    #[test]
    fn add_preserves_insertion_order() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0002")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0003")).unwrap();
        db.insert_collection(&test_collection("ordered")).unwrap();

        db.add_to_collection(
            "col-ordered",
            &["bm-0001".into(), "bm-0002".into(), "bm-0003".into()],
        )
        .unwrap();

        let filter = crate::engine::bookmark::BookmarkFilter {
            collection: Some("ordered".to_string()),
            ..Default::default()
        };
        let bookmarks = db.list_bookmarks(&filter).unwrap();
        assert_eq!(bookmarks.len(), 3);
        assert_eq!(bookmarks[0].id, "bm-0001");
        assert_eq!(bookmarks[1].id, "bm-0002");
        assert_eq!(bookmarks[2].id, "bm-0003");
    }

    #[test]
    fn reorder_changes_order() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0002")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0003")).unwrap();
        db.insert_collection(&test_collection("reorder")).unwrap();

        db.add_to_collection(
            "col-reorder",
            &["bm-0001".into(), "bm-0002".into(), "bm-0003".into()],
        )
        .unwrap();

        // Reorder: 3, 1, 2
        db.reorder_collection(
            "col-reorder",
            &["bm-0003".into(), "bm-0001".into(), "bm-0002".into()],
        )
        .unwrap();

        let filter = crate::engine::bookmark::BookmarkFilter {
            collection: Some("reorder".to_string()),
            ..Default::default()
        };
        let bookmarks = db.list_bookmarks(&filter).unwrap();
        assert_eq!(bookmarks.len(), 3);
        assert_eq!(bookmarks[0].id, "bm-0003");
        assert_eq!(bookmarks[1].id, "bm-0001");
        assert_eq!(bookmarks[2].id, "bm-0002");
    }

    #[test]
    fn add_at_position_inserts_and_shifts() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0002")).unwrap();
        db.insert_bookmark(&test_bookmark("bm-0003")).unwrap();
        db.insert_collection(&test_collection("insert")).unwrap();

        // Add first two
        db.add_to_collection("col-insert", &["bm-0001".into(), "bm-0002".into()]).unwrap();

        // Insert bm-0003 at position 1 (between bm-0001 and bm-0002)
        db.add_to_collection_at("col-insert", &["bm-0003".into()], Some(1)).unwrap();

        let filter = crate::engine::bookmark::BookmarkFilter {
            collection: Some("insert".to_string()),
            ..Default::default()
        };
        let bookmarks = db.list_bookmarks(&filter).unwrap();
        assert_eq!(bookmarks.len(), 3);
        assert_eq!(bookmarks[0].id, "bm-0001");
        assert_eq!(bookmarks[1].id, "bm-0003");
        assert_eq!(bookmarks[2].id, "bm-0002");
    }
}
