use crate::engine::bookmark::{BookmarkHealth, Resolution};
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    pub fn insert_resolution(&self, resolution: &Resolution) -> Result<()> {
        self.conn().execute(
            "INSERT INTO resolutions (id, bookmark_id, resolved_at, health, commit_hash,
             method, match_count, file_path, byte_range, line_range, content_hash, headline, snapshot, breadcrumbs, is_anchored)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            rusqlite::params![
                resolution.id,
                resolution.bookmark_id,
                resolution.resolved_at,
                resolution.health.to_string(),
                resolution.commit_hash,
                resolution.method.to_string(),
                resolution.match_count,
                resolution.file_path,
                resolution.byte_range,
                resolution.line_range,
                resolution.content_hash,
                resolution.headline,
                resolution.snapshot,
                resolution.breadcrumbs,
                resolution.is_anchored,
            ],
        )?;
        Ok(())
    }

    /// Insert a resolution only if it differs from the most recent one for this bookmark.
    ///
    /// Deduplication logic: A resolution is considered a duplicate if the latest resolution
    /// has the same byte_range, line_range, and method — regardless of commit_hash.
    ///
    /// This means if you heal at commit A, then make an unrelated change at commit B
    /// and heal again, we won't create a duplicate resolution since the code is at
    /// the exact same location.
    ///
    /// When a duplicate is detected, updates the existing resolution's commit_hash and
    /// resolved_at instead of creating a new entry.
    ///
    /// Prunes old entries beyond `max_per_bookmark`.
    /// Returns true if a new resolution was recorded, false if an existing one was updated.
    pub fn insert_resolution_if_changed(
        &self,
        resolution: &Resolution,
        max_per_bookmark: usize,
    ) -> Result<bool> {
        // Check if the SINGLE absolute latest resolution has the same byte_range, line_range, method, and health
        // Use id DESC as tiebreaker for same timestamp to ensure deterministic ordering
        let latest_res: Option<(String, String, String, String, String)> = self
            .conn()
            .query_row(
                "SELECT id, COALESCE(byte_range, ''), COALESCE(line_range, ''), method, health
                 FROM resolutions
                 WHERE bookmark_id = ?1
                 ORDER BY resolved_at DESC, id DESC
                 LIMIT 1",
                rusqlite::params![resolution.bookmark_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .ok();

        let is_duplicate = if let Some((_id, br, lr, m, h)) = latest_res {
            br == resolution.byte_range.as_deref().unwrap_or("")
                && lr == resolution.line_range.as_deref().unwrap_or("")
                && m == resolution.method.to_string()
                && h == resolution.health.to_string()
        } else {
            false
        };

        if is_duplicate {
            // Duplicate detected — update the existing latest resolution with new metadata
            let id: String = self.conn().query_row(
                "SELECT id FROM resolutions WHERE bookmark_id = ?1 ORDER BY resolved_at DESC, id DESC LIMIT 1",
                rusqlite::params![resolution.bookmark_id],
                |row| row.get(0),
            )?;
            self.conn().execute(
                "UPDATE resolutions SET commit_hash = ?1, resolved_at = ?2, headline = ?3, snapshot = ?4, breadcrumbs = ?5, is_anchored = ?6 WHERE id = ?7",
                rusqlite::params![
                    resolution.commit_hash,
                    resolution.resolved_at,
                    resolution.headline,
                    resolution.snapshot,
                    resolution.breadcrumbs,
                    resolution.is_anchored,
                    id,
                ],
            )?;
            return Ok(false); // false = no new resolution created
        }

        self.insert_resolution(resolution)?;

        // Prune old entries beyond the cap (now also handled by DB trigger, but kept for safety)
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
            "SELECT id, bookmark_id, resolved_at, health, commit_hash, method,
             match_count, file_path, byte_range, line_range, content_hash, headline, snapshot, breadcrumbs, is_anchored
             FROM resolutions WHERE bookmark_id = ?1
             ORDER BY resolved_at DESC, id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![bookmark_id, limit], |row| {
            let health_str: String = row.get(3)?;
            let method_str: String = row.get(5)?;
            let health = health_str.parse().unwrap_or(BookmarkHealth::Active);
            let method = method_str.parse().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(Resolution {
                id: row.get(0)?,
                bookmark_id: row.get(1)?,
                resolved_at: row.get(2)?,
                health,
                commit_hash: row.get(4)?,
                method,
                match_count: row.get(6)?,
                file_path: row.get(7)?,
                byte_range: row.get(8)?,
                line_range: row.get(9)?,
                content_hash: row.get(10)?,
                headline: row.get(11)?,
                snapshot: row.get(12)?,
                breadcrumbs: row.get(13)?,
                is_anchored: row.get(14)?,
            })
        })?;

        let results: Vec<Resolution> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// Get a single resolution by ID or prefix.
    pub fn get_resolution(&self, id: &str) -> Result<Option<Resolution>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, bookmark_id, resolved_at, health, commit_hash, method,
             match_count, file_path, byte_range, line_range, content_hash, headline, snapshot, breadcrumbs, is_anchored
             FROM resolutions WHERE id LIKE ?1 LIMIT 2",
        )?;
        let pattern = format!("{id}%");
        let results: Vec<Resolution> = stmt
            .query_map([&pattern], |row| {
                let health_str: String = row.get(3)?;
                let method_str: String = row.get(5)?;
                let health = health_str.parse().unwrap_or(BookmarkHealth::Active);
                let method = method_str.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;
                Ok(Resolution {
                    id: row.get(0)?,
                    bookmark_id: row.get(1)?,
                    resolved_at: row.get(2)?,
                    health,
                    commit_hash: row.get(4)?,
                    method,
                    match_count: row.get(6)?,
                    file_path: row.get(7)?,
                    byte_range: row.get(8)?,
                    line_range: row.get(9)?,
                    content_hash: row.get(10)?,
                    headline: row.get(11)?,
                    snapshot: row.get(12)?,
                    breadcrumbs: row.get(13)?,
                    is_anchored: row.get(14)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        match results.len() {
            0 => Ok(None),
            1 => Ok(Some(results.into_iter().next().unwrap())),
            _ => Err(crate::error::Error::Input(format!("ambiguous resolution ID prefix '{id}'"))),
        }
    }

    /// Get the best resolution for previewing a bookmark.
    /// This finds the resolution from the nearest ancestor commit, matching the behavior
    /// of `codemark preview`. This is useful for TUI and other preview contexts.
    pub fn get_preview_resolution(&self, bookmark_id: &str) -> Result<Option<Resolution>> {
        let all_resolutions = self.list_resolutions(bookmark_id, 100)?;

        if all_resolutions.is_empty() {
            return Ok(None);
        }

        // Try to find resolution from nearest ancestor commit
        // Derive the repository root from the database path instead of using current_dir()
        // to ensure correct behavior in multi-repo workflows
        let repo_ctx_path = self
            .path()
            .parent() // .codemark/
            .and_then(|codemark_dir| {
                // Check if the parent directory is named ".codemark"
                if codemark_dir.file_name() == Some(std::ffi::OsStr::new(".codemark")) {
                    // db is at repo/.codemark/codemark.db, return the repo root
                    codemark_dir.parent().map(|p| p.to_path_buf())
                } else {
                    None
                }
            });

        if let Some(repo_path) = repo_ctx_path {
            let commit_hashes: Vec<String> =
                all_resolutions.iter().filter_map(|r| r.commit_hash.clone()).collect();

            if let Ok(Some(nearest_commit)) =
                crate::git::context::find_nearest_ancestor(&repo_path, &commit_hashes)
            {
                // Find the resolution with this commit hash
                if let Some(res) = all_resolutions
                    .iter()
                    .find(|r| r.commit_hash.as_deref() == Some(&nearest_commit))
                {
                    return Ok(Some(res.clone()));
                }
            }
        }

        // Fall back to most recent resolution
        Ok(all_resolutions.first().cloned())
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::bookmark::{Bookmark, BookmarkHealth, Resolution, ResolutionMethod};
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
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: vec![],
        }
    }

    // Initialize test environment
    fn init_test_env() {}

    #[test]
    fn insert_and_list_resolutions() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();

        let res = Resolution {
            is_anchored: true,
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        db.insert_resolution(&res).unwrap();

        let results = db.list_resolutions("bm-0001", 10).unwrap();
        // Expect 2: 1 auto-created by insert_bookmark + 1 manually inserted here
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].method, ResolutionMethod::Exact);
        assert_eq!(results[0].match_count, Some(1));
    }

    #[test]
    fn resolution_metadata_roundtrip() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();

        let res = Resolution {
            is_anchored: true,
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: Some("func test()".to_string()),
            snapshot: Some("line 1\nline 2".to_string()),
            breadcrumbs: None,
        };
        db.insert_resolution(&res).unwrap();

        let results = db.list_resolutions("bm-0001", 10).unwrap();
        // results[0] is the manual insert (sorted by resolved_at DESC)
        assert_eq!(results[0].headline.as_deref(), Some("func test()"));
        assert_eq!(results[0].snapshot.as_deref(), Some("line 1\nline 2"));

        // Test update in insert_resolution_if_changed
        let res_update = Resolution {
            is_anchored: true,
            id: "res-0002".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T02:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("def456".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: Some("func test_updated()".to_string()),
            snapshot: Some("line 1 updated\nline 2 updated".to_string()),
            breadcrumbs: None,
        };
        db.insert_resolution_if_changed(&res_update, 10).unwrap();

        let results = db.list_resolutions("bm-0001", 10).unwrap();
        assert_eq!(results.len(), 2); // Manual + Auto (res-0001 was updated to res-0002 meta)
        assert_eq!(results[0].headline.as_deref(), Some("func test_updated()"));
        assert_eq!(results[0].snapshot.as_deref(), Some("line 1 updated\nline 2 updated"));
        assert_eq!(results[0].commit_hash.as_deref(), Some("def456"));
    }

    #[test]
    fn insert_if_changed_deduplicates() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();

        let res = Resolution {
            is_anchored: true,
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        let inserted = db.insert_resolution_if_changed(&res, 20).unwrap();
        assert!(inserted);

        // Same byte_range, line_range, method but different commit — should UPDATE existing
        let res2 = Resolution {
            is_anchored: true,
            id: "res-0002".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T02:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("def456".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        let inserted = db.insert_resolution_if_changed(&res2, 20).unwrap();
        assert!(!inserted); // Should return false (updated, not inserted)

        // Verify the existing resolution was updated with new commit_hash and resolved_at
        let all = db.list_resolutions("bm-0001", 100).unwrap();
        assert_eq!(all.len(), 2); // Still just 2 resolutions (Auto + res-0001 updated)
        assert_eq!(all[0].id, "res-0001"); // Same ID
        assert_eq!(all[0].commit_hash, Some("def456".to_string())); // Updated commit
        assert_eq!(all[0].resolved_at, "2026-04-01T02:00:00Z"); // Updated timestamp

        // Different byte_range — should be recorded
        let res3 = Resolution {
            is_anchored: true,
            id: "res-0003".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T03:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("150:250".to_string()),
            line_range: Some("15:25".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        let inserted = db.insert_resolution_if_changed(&res3, 20).unwrap();
        assert!(inserted);

        // Different method — should be recorded
        let res4 = Resolution {
            is_anchored: true,
            id: "res-0004".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T04:00:00Z".to_string(),
            health: BookmarkHealth::Drifted,
            commit_hash: Some("abc123".to_string()),
            method: ResolutionMethod::Relaxed,
            match_count: Some(1),
            file_path: Some("src/main.swift".to_string()),
            byte_range: Some("150:250".to_string()),
            line_range: Some("15:25".to_string()),
            content_hash: Some("sha256:abcd1234abcd1234".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
        };
        let inserted = db.insert_resolution_if_changed(&res4, 20).unwrap();
        assert!(inserted);

        let all = db.list_resolutions("bm-0001", 100).unwrap();
        assert_eq!(all.len(), 4); // Auto + res1 (updated) + res3 + res4 (res2 was merged into res1)
    }

    #[test]
    fn pruning_keeps_only_max_entries() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();

        // Insert 5 resolutions with max_per_bookmark = 3
        // Each with different byte_ranges so they create distinct entries
        for i in 1..=5 {
            let byte_start = 100 + (i * 10);
            let byte_end = 200 + (i * 10);
            let line_start = 10 + i;
            let line_end = 20 + i;
            let res = Resolution {
                is_anchored: true,
                id: format!("res-{i:04}"),
                bookmark_id: "bm-0001".to_string(),
                resolved_at: format!("2026-04-01T{i:02}:00:00Z"),
                health: BookmarkHealth::Active,
                commit_hash: Some(format!("commit-{i}")),
                method: ResolutionMethod::Exact,
                match_count: Some(1),
                file_path: Some("src/main.swift".to_string()),
                byte_range: Some(format!("{byte_start}:{byte_end}")),
                line_range: Some(format!("{line_start}:{line_end}")),
                content_hash: None,
                headline: None,
                snapshot: None,
                breadcrumbs: None,
            };
            let res_id = res.id.clone();
            if db.insert_resolution_if_changed(&res, 3).unwrap() {
                db.update_bookmark_resolution_id("bm-0001", &res_id).unwrap();
            }
        }

        let all = db.list_resolutions("bm-0001", 100).unwrap();
        assert_eq!(all.len(), 3);
        // Should keep the 3 most recent (by byte_range, which correlates with insertion order)
        assert_eq!(all[0].commit_hash.as_deref(), Some("commit-5"));
        assert_eq!(all[1].commit_hash.as_deref(), Some("commit-4"));
        assert_eq!(all[2].commit_hash.as_deref(), Some("commit-3"));
    }

    #[test]
    fn resolution_cascade_on_bookmark_delete() {
        init_test_env();
        let db = Database::open_in_memory().unwrap();
        db.insert_bookmark(&test_bookmark("bm-0001")).unwrap();

        let res = Resolution {
            is_anchored: true,
            id: "res-0001".to_string(),
            bookmark_id: "bm-0001".to_string(),
            resolved_at: "2026-04-01T01:00:00Z".to_string(),
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
        db.insert_resolution(&res).unwrap();
        db.delete_bookmark("bm-0001").unwrap();

        let results = db.list_resolutions("bm-0001", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_healing_is_appending() {
        // @lat: [[tests#System Invariants Tests#Health & Resolution Rules#Healing is Appending]]
        // Verifies that healing or resolving a bookmark creates a new resolutions record
        // and updates the current_resolution_id pointer, without modifying the original
        // bookmarks row data.
        init_test_env();
        let db = Database::open_in_memory().unwrap();

        let bm_id = "heal-test-bm";
        let original_query = "(function_declaration) @target /* original */";
        let original_language = "swift";
        let original_file_path = "src/original.swift";
        let original_created_at = "2024-01-01T00:00:00Z".to_string();

        // Create initial bookmark
        let bm = crate::engine::bookmark::Bookmark {
            id: bm_id.to_string(),
            query: original_query.to_string(),
            language: original_language.to_string(),
            file_path: original_file_path.to_string(),
            content_hash: Some("sha256:original".to_string()),
            commit_hash: Some("commit-1".to_string()),
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: Some(original_created_at.clone()),
            stale_since: None,
            created_at: original_created_at.clone(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };
        db.insert_bookmark(&bm).unwrap();

        // Capture the initial bookmark state from DB
        let initial_bm = db.get_bookmark(bm_id).unwrap().unwrap();
        let initial_res_id = initial_bm.current_resolution_id.clone().unwrap();

        // Verify the initial resolution was created
        let initial_resolutions = db.list_resolutions(bm_id, 10).unwrap();
        assert_eq!(initial_resolutions.len(), 1, "Should have 1 initial resolution");
        assert_eq!(initial_resolutions[0].id, initial_res_id);

        // Simulate a "heal" operation - create a new resolution
        let heal_resolution = Resolution {
            is_anchored: true,
            id: "res-heal-1".to_string(),
            bookmark_id: bm_id.to_string(),
            resolved_at: "2024-01-02T00:00:00Z".to_string(),
            health: BookmarkHealth::Active,
            commit_hash: Some("commit-2".to_string()),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("src/original.swift".to_string()),
            byte_range: Some("100:200".to_string()),
            line_range: Some("10:20".to_string()),
            content_hash: Some("sha256:newhash".to_string()),
            headline: Some("Updated headline".to_string()),
            snapshot: Some("Updated snapshot".to_string()),
            breadcrumbs: Some("[]".to_string()),
        };

        // Insert the new resolution (simulating heal)
        let inserted = db.insert_resolution_if_changed(&heal_resolution, 20).unwrap();
        assert!(inserted, "Heal should create a new resolution when location changes");

        // Update the bookmark's current_resolution_id pointer
        db.update_bookmark_resolution_id(bm_id, "res-heal-1").unwrap();

        // Verify the healing behavior
        let healed_bm = db.get_bookmark(bm_id).unwrap().unwrap();
        let all_resolutions = db.list_resolutions(bm_id, 100).unwrap();

        // 1. A new resolution was created (not replaced)
        assert_eq!(all_resolutions.len(), 2, "Healing should append a new resolution");
        assert!(
            all_resolutions.iter().any(|r| r.id == "res-heal-1"),
            "New resolution should exist"
        );
        assert!(
            all_resolutions.iter().any(|r| r.id == initial_res_id),
            "Old resolution should still exist"
        );

        // 2. The current_resolution_id pointer was updated
        assert_eq!(healed_bm.current_resolution_id, Some("res-heal-1".to_string()));

        // 3. The original bookmarks row immutable fields were NOT modified
        assert_eq!(healed_bm.id, initial_bm.id, "Bookmark ID must never change");
        assert_eq!(
            healed_bm.created_at, initial_bm.created_at,
            "Bookmark created_at must never change"
        );

        // 4. The mutable fields (via resolution) were updated
        assert_eq!(healed_bm.last_resolved_at, Some("2024-01-02T00:00:00Z".to_string()));

        // 5. Verify the most recent resolution has the new data
        let latest_res = &all_resolutions[0]; // Ordered by resolved_at DESC
        assert_eq!(latest_res.id, "res-heal-1");
        assert_eq!(latest_res.commit_hash, Some("commit-2".to_string()));
        assert_eq!(latest_res.headline, Some("Updated headline".to_string()));
    }
}
