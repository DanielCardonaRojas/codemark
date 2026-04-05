use crate::engine::bookmark::{Resolution, ResolutionMethod};
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    pub fn insert_resolution(&self, resolution: &Resolution) -> Result<()> {
        self.conn().execute(
            "INSERT INTO resolutions (id, bookmark_id, resolved_at, commit_hash,
             method, match_count, file_path, byte_range, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                resolution.id,
                resolution.bookmark_id,
                resolution.resolved_at,
                resolution.commit_hash,
                resolution.method.to_string(),
                resolution.match_count,
                resolution.file_path,
                resolution.byte_range,
                resolution.content_hash,
            ],
        )?;
        Ok(())
    }

    /// Insert a resolution only if it differs from the most recent one for this bookmark.
    /// Skips if the latest resolution has the same commit_hash, byte_range, and method.
    /// Prunes old entries beyond `max_per_bookmark`.
    /// Returns true if a new resolution was recorded, false if it was a duplicate.
    pub fn insert_resolution_if_changed(
        &self,
        resolution: &Resolution,
        max_per_bookmark: usize,
    ) -> Result<bool> {
        let is_dup: bool = self.conn().query_row(
            "SELECT COUNT(*) > 0 FROM resolutions
             WHERE bookmark_id = ?1
               AND id = (
                   SELECT id FROM resolutions
                   WHERE bookmark_id = ?1
                   ORDER BY resolved_at DESC LIMIT 1
               )
               AND COALESCE(commit_hash, '') = COALESCE(?2, '')
               AND COALESCE(byte_range, '') = COALESCE(?3, '')
               AND method = ?4",
            rusqlite::params![
                resolution.bookmark_id,
                resolution.commit_hash,
                resolution.byte_range,
                resolution.method.to_string(),
            ],
            |row| row.get(0),
        ).unwrap_or(false);

        if is_dup {
            return Ok(false);
        }

        self.insert_resolution(resolution)?;

        // Prune old entries beyond the cap
        if max_per_bookmark > 0 {
            self.conn().execute(
                "DELETE FROM resolutions
                 WHERE bookmark_id = ?1
                   AND id NOT IN (
                       SELECT id FROM resolutions
                       WHERE bookmark_id = ?1
                       ORDER BY resolved_at DESC LIMIT ?2
                   )",
                rusqlite::params![resolution.bookmark_id, max_per_bookmark],
            )?;
        }

        Ok(true)
    }

    pub fn list_resolutions(&self, bookmark_id: &str, limit: usize) -> Result<Vec<Resolution>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, resolved_at, commit_hash, method,
             match_count, file_path, byte_range, content_hash
             FROM resolutions WHERE bookmark_id = ?1
             ORDER BY resolved_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![bookmark_id, limit], |row| {
            let method_str: String = row.get(4)?;
            Ok(Resolution {
                id: row.get(0)?,
                bookmark_id: row.get(1)?,
                resolved_at: row.get(2)?,
                commit_hash: row.get(3)?,
                method: method_str.parse().unwrap_or(ResolutionMethod::Failed),
                match_count: row.get(5)?,
                file_path: row.get(6)?,
                byte_range: row.get(7)?,
                content_hash: row.get(8)?,
            })
        })?;

        let results: Vec<Resolution> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Get a single resolution by ID or prefix.
    pub fn get_resolution(&self, id: &str) -> Result<Option<Resolution>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, resolved_at, commit_hash, method,
             match_count, file_path, byte_range, content_hash
             FROM resolutions WHERE id LIKE ?1 LIMIT 2",
        )?;
        let pattern = format!("{id}%");
        let results: Vec<Resolution> = stmt
            .query_map([&pattern], |row| {
                let method_str: String = row.get(4)?;
                Ok(Resolution {
                    id: row.get(0)?,
                    bookmark_id: row.get(1)?,
                    resolved_at: row.get(2)?,
                    commit_hash: row.get(3)?,
                    method: method_str.parse().unwrap_or(ResolutionMethod::Failed),
                    match_count: row.get(5)?,
                    file_path: row.get(6)?,
                    byte_range: row.get(7)?,
                    content_hash: row.get(8)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        match results.len() {
            0 => Ok(None),
            1 => Ok(Some(results.into_iter().next().unwrap())),
            _ => Err(crate::error::Error::Input(format!(
                "ambiguous resolution ID prefix '{id}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::{Bookmark, BookmarkStatus, Resolution, ResolutionMethod};
    use crate::storage::db::Database;

    fn test_bookmark() -> Bookmark {
        Bookmark {
            id: "bm-0001".to_string(),
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

    #[test]
    fn insert_and_list_resolutions() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark()).unwrap();

        let res = Resolution {
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
        };
        db.insert_resolution(&res).unwrap();

        let results = db.list_resolutions("bm-0001", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].method, ResolutionMethod::Exact);
        assert_eq!(results[0].match_count, Some(1));
    }

    #[test]
    fn insert_if_changed_deduplicates() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark()).unwrap();

        let res = Resolution {
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
        };
        let inserted = db.insert_resolution_if_changed(&res, 20).unwrap();
        assert!(inserted);

        // Same commit, same byte_range, same method — should be skipped
        let res2 = Resolution {
            id: "res-0002".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T02:00:00Z".to_string(),
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
        };
        let inserted = db.insert_resolution_if_changed(&res2, 20).unwrap();
        assert!(!inserted);

        // Different byte_range — should be recorded
        let res3 = Resolution {
            id: "res-0003".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T03:00:00Z".to_string(),
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("150:250".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
        };
        let inserted = db.insert_resolution_if_changed(&res3, 20).unwrap();
        assert!(inserted);

        // Different commit — should be recorded
        let res4 = Resolution {
            id: "res-0004".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T04:00:00Z".to_string(),
            commit_hash: Some("def456".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("150:250".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
        };
        let inserted = db.insert_resolution_if_changed(&res4, 20).unwrap();
        assert!(inserted);

        // Different method — should be recorded
        let res5 = Resolution {
            id: "res-0005".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T05:00:00Z".to_string(),
            commit_hash: Some("def456".to_string()),
            method: ResolutionMethod::Relaxed,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("150:250".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
        };
        let inserted = db.insert_resolution_if_changed(&res5, 20).unwrap();
        assert!(inserted);

        let all = db.list_resolutions("bm-0001", 100).unwrap();
        assert_eq!(all.len(), 4); // res1, res3, res4, res5 (res2 was deduped)
    }

    #[test]
    fn pruning_keeps_only_max_entries() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark()).unwrap();

        // Insert 5 resolutions with max_per_bookmark = 3
        for i in 1..=5 {
            let res = Resolution {
                id: format!("res-{i:04}"),
                bookmark_id: "bm-0001".to_string(),
                resolved_at: format!("2026-04-01T{i:02}:00:00Z"),
                commit_hash: Some(format!("commit-{i}")),
                method: ResolutionMethod::Exact,
                match_count: Some(1),
                file_path: Some("src/main.swift".to_string()),
                byte_range: Some("100:200".to_string()),
                content_hash: None,
            };
            db.insert_resolution_if_changed(&res, 3).unwrap();
        }

        let all = db.list_resolutions("bm-0001", 100).unwrap();
        assert_eq!(all.len(), 3);
        // Should keep the 3 most recent (commit-5, commit-4, commit-3)
        assert_eq!(all[0].commit_hash.as_deref(), Some("commit-5"));
        assert_eq!(all[1].commit_hash.as_deref(), Some("commit-4"));
        assert_eq!(all[2].commit_hash.as_deref(), Some("commit-3"));
    }

    #[test]
    fn resolution_cascade_on_bookmark_delete() {
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark()).unwrap();

        let res = Resolution {
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            commit_hash: None,
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: None,
            byte_range: None,
            content_hash: None,
        };
        db.insert_resolution(&res).unwrap();
        db.delete_bookmark("bm-0001").unwrap();

        let results = db.list_resolutions("bm-0001", 10).unwrap();
        assert!(results.is_empty());
    }
}
