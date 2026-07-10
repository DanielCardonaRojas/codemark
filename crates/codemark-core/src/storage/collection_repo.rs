use crate::engine::bookmark::{Collection, CollectionHealth};
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    pub fn insert_collection(&self, collection: &Collection) -> Result<()> {
        tracing::debug!(target: "codemark::db", id = %collection.id, name = %collection.name, "inserting collection");
        self.conn().execute(
            "INSERT INTO collections (id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, repo_id, status, health, health_computed_at, updated_at, imported_from_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            rusqlite::params![
                collection.id,
                collection.name,
                collection.description,
                collection.visibility.to_string(),
                collection.created_at,
                collection.created_by,
                collection.created_branch,
                collection.published_at,
                collection.published_commit_sha,
                collection.repo_url,
                collection.repo_id,
                collection.status,
                collection.health.as_ref().map(|h| h.to_string()),
                collection.health_computed_at,
                collection.updated_at,
                collection.imported_from_url,
            ],
        )?;
        Ok(())
    }

    /// Get a collection by its exact name.
    pub fn get_collection_by_name(&self, name: &str) -> Result<Option<Collection>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, repo_id, status, health, health_computed_at, updated_at, imported_from_url
             FROM collections WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map([name], row_to_collection)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get every collection previously imported from the given source URL,
    /// ordered oldest-first.
    ///
    /// Used to make `pull` idempotent: a collection imported from a remote tour
    /// records that tour's URL in `imported_from_url`, so re-pulling the same
    /// tour finds and removes the prior copies instead of duplicating them.
    ///
    /// Returns *all* matches (not just one) so a database that accumulated
    /// duplicates under the old pull behavior is fully repaired in a single
    /// re-pull rather than one copy at a time.
    pub fn get_collections_by_imported_url(&self, url: &str) -> Result<Vec<Collection>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, repo_id, status, health, health_computed_at, updated_at, imported_from_url
             FROM collections WHERE imported_from_url = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([url], row_to_collection)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Get a collection by its unique ID.
    pub fn get_collection_by_id(&self, id: &str) -> Result<Option<Collection>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, repo_id, status, health, health_computed_at, updated_at, imported_from_url
             FROM collections WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([id], row_to_collection)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Get a collection by its ID prefix (at least 4 characters).
    pub fn get_collection_by_id_prefix(&self, prefix: &str) -> Result<Option<Collection>> {
        if prefix.len() < 4 {
            return Err(crate::error::Error::Input(
                "Collection ID prefix must be at least 4 characters".into(),
            ));
        }
        let mut stmt = self.conn().prepare(
            "SELECT id, name, description, visibility, created_at, created_by, created_branch, published_at, published_commit_sha, repo_url, repo_id, status, health, health_computed_at, updated_at, imported_from_url
             FROM collections WHERE id LIKE ?1",
        )?;
        let pattern = format!("{prefix}%");
        let results: Vec<Collection> =
            stmt.query_map([&pattern], row_to_collection)?.filter_map(|r| r.ok()).collect();

        match results.len() {
            0 => Ok(None),
            1 => Ok(Some(results.into_iter().next().unwrap())),
            _ => Err(crate::error::Error::Input(format!(
                "Ambiguous collection ID prefix '{prefix}': matches {} collections",
                results.len()
            ))),
        }
    }

    /// List all collections with their bookmark counts.
    pub fn list_collections(&self) -> Result<Vec<(Collection, usize)>> {
        let mut stmt = self.conn().prepare(
            "SELECT c.id, c.name, c.description, c.visibility, c.created_at, c.created_by, c.created_branch,
             c.published_at, c.published_commit_sha, c.repo_url, c.repo_id, c.status, c.health, c.health_computed_at, c.updated_at, c.imported_from_url,
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
                visibility: row.get::<_, String>(3)?.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                created_at: row.get(4)?,
                created_by: row.get(5)?,
                created_branch: row.get(6)?,
                published_at: row.get(7)?,
                published_commit_sha: row.get(8)?,
                repo_url: row.get(9)?,
                repo_id: row.get(10)?,
                status: row.get(11)?,
                health: row.get::<_, Option<String>>(12)?.and_then(|h| h.parse().ok()),
                health_computed_at: row.get(13)?,
                updated_at: row.get(14)?,
                imported_from_url: row.get(15)?,
            };
            let count: usize = row.get(16)?;
            Ok((collection, count))
        })?;

        let results: Vec<(Collection, usize)> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Search collections by text, returning matches with their bookmark counts.
    ///
    /// `query` matches (case-insensitively) against the collection name,
    /// description, and any of its tags. `tag` additionally restricts results to
    /// collections carrying that exact tag. With no filters this behaves like
    /// [`list_collections`].
    pub fn search_collections(
        &self,
        query: Option<&str>,
        tag: Option<&str>,
    ) -> Result<Vec<(Collection, usize)>> {
        let mut sql = String::from(
            "SELECT c.id, c.name, c.description, c.visibility, c.created_at, c.created_by, c.created_branch,
             c.published_at, c.published_commit_sha, c.repo_url, c.repo_id, c.status, c.health, c.health_computed_at, c.updated_at, c.imported_from_url,
             COUNT(DISTINCT cb.bookmark_id) AS bookmark_count
             FROM collections c
             LEFT JOIN collection_bookmarks cb ON c.id = cb.collection_id
             LEFT JOIN collection_tags ct ON c.id = ct.collection_id",
        );

        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(q) = query {
            let like = format!("%{q}%");
            conditions.push("(c.name LIKE ? OR c.description LIKE ? OR ct.tag LIKE ?)".to_string());
            params.push(Box::new(like.clone()));
            params.push(Box::new(like.clone()));
            params.push(Box::new(like));
        }
        if let Some(t) = tag {
            conditions.push(
                "c.id IN (SELECT collection_id FROM collection_tags WHERE tag = ?)".to_string(),
            );
            params.push(Box::new(t.to_string()));
        }

        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }

        sql.push_str(" GROUP BY c.id ORDER BY c.name");

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let collection = row_to_collection(row)?;
            let count: usize = row.get(16)?;
            Ok((collection, count))
        })?;

        let results: Vec<(Collection, usize)> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Delete a collection by ID, returning the number of bookmarks that were in it.
    pub fn delete_collection_by_id(&self, id: &str) -> Result<usize> {
        let count: usize = self
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        self.conn().execute("DELETE FROM collections WHERE id = ?1", [id])?;
        Ok(count)
    }

    /// Delete a collection and all its bookmarks atomically.
    /// Returns the number of bookmarks deleted.
    pub fn delete_collection_recursive(&mut self, id: &str) -> Result<usize> {
        let conn = self.conn_mut();
        let tx = conn.transaction()?;

        let count: usize = tx
            .query_row(
                "SELECT COUNT(*) FROM collection_bookmarks WHERE collection_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Delete bookmarks that are in this collection
        tx.execute(
            "DELETE FROM bookmarks WHERE id IN (SELECT bookmark_id FROM collection_bookmarks WHERE collection_id = ?1)",
            [id],
        )?;

        // Delete the collection itself (association records are deleted via ON DELETE CASCADE)
        tx.execute("DELETE FROM collections WHERE id = ?1", [id])?;

        tx.commit()?;
        Ok(count)
    }

    /// Delete a collection by name, returning the number of bookmarks that were in it.
    #[allow(dead_code)]
    pub fn delete_collection(&self, name: &str) -> Result<usize> {
        if let Some(c) = self.get_collection_by_name(name)? {
            self.delete_collection_by_id(&c.id)
        } else {
            Ok(0)
        }
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

    /// List all collections that contain a specific bookmark.
    pub fn list_collections_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<Collection>> {
        let mut stmt = self.conn().prepare(
            "SELECT c.id, c.name, c.description, c.visibility, c.created_at, c.created_by, c.created_branch,
             c.published_at, c.published_commit_sha, c.repo_url, c.repo_id, c.status, c.health, c.health_computed_at, c.updated_at, c.imported_from_url
             FROM collections c
             JOIN collection_bookmarks cb ON c.id = cb.collection_id
             WHERE cb.bookmark_id = ?1
             ORDER BY c.name",
        )?;
        let rows = stmt.query_map([bookmark_id], row_to_collection)?;
        let results: Vec<Collection> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    pub fn update_collection_health(
        &self,
        id: &str,
        health: Option<CollectionHealth>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn().execute(
            "UPDATE collections SET health = ?1, health_computed_at = ?2 WHERE id = ?3",
            rusqlite::params![health.map(|h| h.to_string()), now, id],
        )?;
        Ok(())
    }

    /// Recompute the health of a collection based on its bookmarks' health.
    /// Rule: stale > drifted > active. Archived bookmarks are excluded.
    pub fn recompute_collection_health(&self, id: &str) -> Result<Option<CollectionHealth>> {
        let health_str: Option<String> = self.conn().query_row(
            "WITH bookmark_health AS (
              SELECT r.health
              FROM collection_bookmarks cb
              JOIN bookmarks b ON b.id = cb.bookmark_id
              JOIN resolutions r ON b.current_resolution_id = r.id
              WHERE cb.collection_id = ?1 AND r.health != 'archived'
            )
            SELECT 
                   CASE
                     WHEN COUNT(*) = 0 THEN NULL
                     WHEN MAX(CASE WHEN health = 'stale'   THEN 1 ELSE 0 END) = 1 THEN 'stale'
                     WHEN MAX(CASE WHEN health = 'drifted' THEN 1 ELSE 0 END) = 1 THEN 'drifted'
                     ELSE 'active'
                   END AS health
            FROM bookmark_health",
            [id],
            |row| row.get(0),
        )?;

        let health = health_str.and_then(|s| s.parse().ok());
        self.update_collection_health(id, health)?;
        Ok(health)
    }

    /// List IDs of collections that contain a specific bookmark.
    pub fn list_collection_ids_for_bookmark(&self, bookmark_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn()
            .prepare("SELECT collection_id FROM collection_bookmarks WHERE bookmark_id = ?1")?;
        let rows = stmt.query_map([bookmark_id], |row| row.get(0))?;
        let results: Vec<String> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// List all unique branches used by collections.
    pub fn list_all_branches(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT created_branch FROM collections
             WHERE created_branch IS NOT NULL
             ORDER BY created_branch",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut branches = Vec::new();
        for branch in rows {
            branches.push(branch?);
        }
        Ok(branches)
    }
}

fn row_to_collection(row: &rusqlite::Row) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        visibility: row.get::<_, String>(3)?.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: row.get(4)?,
        created_by: row.get(5)?,
        created_branch: row.get(6)?,
        published_at: row.get(7)?,
        published_commit_sha: row.get(8)?,
        repo_url: row.get(9)?,
        repo_id: row.get(10)?,
        status: row.get(11)?,
        health: row.get::<_, Option<String>>(12)?.and_then(|h| h.parse().ok()),
        health_computed_at: row.get(13)?,
        updated_at: row.get(14)?,
        imported_from_url: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::{
        Bookmark, BookmarkHealth, Collection, CollectionHealth, CollectionTag,
    };
    use crate::storage::db::Database;

    fn test_bookmark(id: &str) -> Bookmark {
        // Use unique file_path and query to avoid UNIQUE constraint violations
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
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![],
        }
    }

    fn test_collection(name: &str) -> Collection {
        Collection {
            id: format!("col-{name}"),
            name: name.to_string(),
            description: Some(format!("Test collection {name}")),
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
        }
    }

    // Initialize test environment
    fn init_test_env() {}

    #[test]
    fn create_and_get_collection() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let col = test_collection("bugfix-auth");
        db.insert_collection(&col).unwrap();

        let fetched = db.get_collection_by_name("bugfix-auth").unwrap().unwrap();
        assert_eq!(fetched.name, "bugfix-auth");
        assert_eq!(fetched.description, Some("Test collection bugfix-auth".to_string()));
    }

    #[test]
    fn get_collections_by_imported_url_returns_all_ordered() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let url = "http://example.com/tours/dup";

        // Simulate a database that accumulated two collections sharing the same
        // imported URL (possible under the old, non-idempotent pull behavior).
        let mut older = test_collection("older");
        older.imported_from_url = Some(url.to_string());
        older.created_at = "2026-01-01T00:00:00Z".to_string();

        let mut newer = test_collection("newer");
        newer.imported_from_url = Some(url.to_string());
        newer.created_at = "2026-02-01T00:00:00Z".to_string();

        // Insert newest-first to prove the result is ordered by created_at, not
        // by row/insertion order.
        db.insert_collection(&newer).unwrap();
        db.insert_collection(&older).unwrap();

        // Every match is returned (so re-pull can repair legacy duplicates in
        // one pass), oldest-first.
        let found = db.get_collections_by_imported_url(url).unwrap();
        let ids: Vec<&str> = found.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec![older.id.as_str(), newer.id.as_str()]);

        assert!(db.get_collections_by_imported_url("http://example.com/none").unwrap().is_empty());
    }

    #[test]
    fn add_bookmarks_to_collection() {
        init_test_env();
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
        init_test_env();
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
    fn search_collections_matches_name_description_and_tag() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();
        db.insert_collection(&test_collection("auth-refactor")).unwrap();
        db.insert_collection(&test_collection("payments")).unwrap();
        db.add_to_collection("col-auth-refactor", &["bm-0001".into()]).unwrap();

        // Tag one collection so we can exercise the tag path too.
        db.insert_collection_tag(&CollectionTag {
            collection_id: "col-payments".to_string(),
            tag: "billing".to_string(),
            added_at: "2026-04-01T00:00:00Z".to_string(),
            added_by: None,
        })
        .unwrap();

        // Name match, with the bookmark count carried through.
        let by_name = db.search_collections(Some("auth"), None).unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].0.name, "auth-refactor");
        assert_eq!(by_name[0].1, 1);

        // Description match ("Test collection payments" contains "payments").
        let by_desc = db.search_collections(Some("payments"), None).unwrap();
        assert_eq!(by_desc.len(), 1);
        assert_eq!(by_desc[0].0.name, "payments");

        // Query matches a tag value.
        let by_tag_query = db.search_collections(Some("billing"), None).unwrap();
        assert_eq!(by_tag_query.len(), 1);
        assert_eq!(by_tag_query[0].0.name, "payments");

        // Exact tag filter.
        let by_tag = db.search_collections(None, Some("billing")).unwrap();
        assert_eq!(by_tag.len(), 1);
        assert_eq!(by_tag[0].0.name, "payments");

        // No filters lists everything.
        let all = db.search_collections(None, None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn delete_collection_preserves_bookmarks() {
        init_test_env();
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
        init_test_env();
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
    fn test_collection_health_aggregation() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let col = test_collection("health-test");
        db.insert_collection(&col).unwrap();

        let mut bm1 = test_bookmark("bm1");
        bm1.health = BookmarkHealth::Active;
        db.insert_bookmark(&bm1).unwrap();

        let mut bm2 = test_bookmark("bm2");
        bm2.health = BookmarkHealth::Drifted;
        db.insert_bookmark(&bm2).unwrap();

        db.add_to_collection(&col.id, &["bm1".into(), "bm2".into()]).unwrap();

        // Initial recompute should be 'drifted' (bm2 is drifted)
        let health = db.recompute_collection_health(&col.id).unwrap();
        assert_eq!(health, Some(CollectionHealth::Drifted));

        // Add a stale bookmark
        let mut bm3 = test_bookmark("bm3");
        bm3.health = BookmarkHealth::Stale;
        db.insert_bookmark(&bm3).unwrap();
        db.add_to_collection(&col.id, &["bm3".into()]).unwrap();

        let health = db.recompute_collection_health(&col.id).unwrap();
        assert_eq!(health, Some(CollectionHealth::Stale));

        // Archive the stale one, health should return to drifted
        let archive_res = crate::engine::bookmark::Resolution {
            is_dirty: false,
            id: "archive-res".to_string(),
            bookmark_id: "bm3".to_string(),
            resolved_at: chrono::Utc::now().to_rfc3339(),
            health: BookmarkHealth::Archived,
            commit_hash: None,
            method: crate::engine::bookmark::ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: None,
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        db.insert_resolution(&archive_res).unwrap();
        db.update_bookmark_resolution_id("bm3", "archive-res").unwrap();

        let health = db.recompute_collection_health(&col.id).unwrap();
        assert_eq!(health, Some(CollectionHealth::Drifted));
    }

    #[test]
    fn test_empty_collection_health() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        let col = test_collection("empty");
        db.insert_collection(&col).unwrap();

        let health = db.recompute_collection_health(&col.id).unwrap();
        assert_eq!(health, None);
    }
}
