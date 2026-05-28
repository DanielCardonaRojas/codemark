//! Healing logic for bookmarks.
//!
//! This module provides the core healing functionality that can be reused
//! across different interfaces (CLI, TUI, etc.).

use crate::config::Config;
use crate::engine::bookmark::{Bookmark, BookmarkHealth, Resolution, ResolutionMethod};
use crate::engine::{health, resolution};
use crate::error::Result;
use crate::git::context as git_context;
use crate::parser::languages::{Language, ParseCache};
use crate::storage::db::Database;

/// Result of healing a single bookmark.
#[derive(Debug, Clone)]
pub struct HealResult {
    /// The bookmark ID
    pub bookmark_id: String,
    /// The new resolution ID (if created)
    pub resolution_id: Option<String>,
    /// Previous health status
    pub previous_health: BookmarkHealth,
    /// New health status
    pub new_health: BookmarkHealth,
    /// Resolution method used
    pub resolution_method: ResolutionMethod,
    /// The resolved file path (if resolution succeeded)
    pub file_path: Option<String>,
    /// The resolved byte range as "start-end" (if resolution succeeded)
    pub byte_range: Option<String>,
    /// Whether this bookmark was intentionally skipped (e.g., git ancestor check)
    pub was_skipped: bool,
}

/// Options for healing bookmarks.
#[derive(Debug, Clone, Default)]
pub struct HealOptions {
    /// Skip git ancestry checks
    pub force: bool,
    /// Auto-archive stale bookmarks past threshold
    pub auto_archive: bool,
    /// Days after which to auto-archive stale bookmarks
    pub archive_after: u32,
    /// Validate only - don't record resolutions to the database
    pub validate_only: bool,
}

/// Heal a single bookmark.
///
/// This function:
/// 1. Optionally checks git ancestry (unless force=true)
/// 2. Resolves the bookmark using tree-sitter
/// 3. Calculates the new health status
/// 4. Records a new resolution (if changed)
/// 5. Updates the bookmark's current resolution pointer
pub async fn heal_bookmark(
    db: &Database,
    bookmark: &Bookmark,
    config: &Config,
    options: &HealOptions,
) -> Result<HealResult> {
    let previous_health = bookmark.health;

    // Get previous resolution for location tracking
    let previous_resolution = db.list_resolutions(&bookmark.id, 1).ok();

    // Skip heal if HEAD is before latest resolution (unless --force is set)
    if !options.force {
        let cwd = std::env::current_dir()?;
        let current_head = git_context::detect_context(&cwd).and_then(|ctx| ctx.head_commit);

        if let (Some(ref head), Some(res)) =
            (current_head, previous_resolution.as_ref().and_then(|r| r.first()))
            && let Some(ref res_commit) = res.commit_hash
        {
            match git_context::is_ancestor(&cwd, head, res_commit) {
                Ok(true) => {
                    // HEAD is ancestor of resolution (resolution is ahead)
                    // Return early without healing
                    return Ok(HealResult {
                        bookmark_id: bookmark.id.clone(),
                        resolution_id: None,
                        previous_health,
                        new_health: previous_health,
                        resolution_method: ResolutionMethod::Failed,
                        file_path: None,
                        byte_range: None,
                        was_skipped: true,
                    });
                }
                Ok(false) | Err(_) => {
                    // HEAD is ahead or unrelated, or git error - proceed with heal
                }
            }
        }
    }

    // Parse the language
    let Ok(lang) = bookmark.language.parse::<Language>() else {
        return Ok(HealResult {
            bookmark_id: bookmark.id.clone(),
            resolution_id: None,
            previous_health,
            new_health: previous_health,
            resolution_method: ResolutionMethod::Failed,
            file_path: None,
            byte_range: None,
            was_skipped: false,
        });
    };

    // Resolve the bookmark
    let mut cache = ParseCache::new(lang)?;
    let ts_lang = lang.tree_sitter_language();
    let provider = crate::vfs::LocalFileProvider;
    let result = resolution::resolve(bookmark, &mut cache, &ts_lang, db.path(), &provider).await?;

    // Calculate new health status
    let days_since = health::days_since_resolution(bookmark.last_resolved_at.as_deref());
    let new_status = health::transition(
        bookmark.health,
        result.method,
        result.hash_matches,
        days_since,
        config.health.stale_days(),
    );

    // Auto-archive check
    let final_status = if options.auto_archive
        && new_status == BookmarkHealth::Stale
        && bookmark
            .stale_since
            .as_deref()
            .is_some_and(|s| health::should_auto_archive(s, options.archive_after))
    {
        BookmarkHealth::Archived
    } else {
        new_status
    };

    let breadcrumbs_json = if result.breadcrumbs.is_empty() {
        None
    } else {
        serde_json::to_string(&result.breadcrumbs).ok()
    };

    let new_resolution_id = if options.validate_only {
        // Skip database writes when validating
        None
    } else {
        // Get current commit hash from the DB's repo context (not CWD)
        let repo_base = db.path().parent().unwrap_or_else(|| db.path());
        let git_ctx = git_context::detect_context(repo_base);
        let commit_hash = git_ctx.as_ref().and_then(|ctx| ctx.head_commit.clone());
        let repo_root = git_ctx.map(|ctx| ctx.repo_root).unwrap_or_else(|| repo_base.to_path_buf());

        // Create and insert resolution — default to dirty on git errors (fail-closed)
        let is_dirty =
            !git_context::is_file_clean(&repo_root, &result.file_path).unwrap_or(true);
        let resolution = Resolution {
            id: uuid::Uuid::new_v4().to_string(),
            bookmark_id: bookmark.id.clone(),
            resolved_at: chrono::Utc::now().to_rfc3339(),
            health: final_status,
            commit_hash,
            method: result.method,
            match_count: Some(1),
            file_path: Some(result.file_path.clone()),
            byte_range: Some(format!("{}-{}", result.byte_range.0, result.byte_range.1)),
            line_range: Some(format!("{}-{}", result.start_line + 1, result.end_line + 1)),
            content_hash: Some(result.content_hash.clone()),
            headline: None,
            snapshot: Some(result.matched_text.clone()),
            breadcrumbs: breadcrumbs_json,
            is_dirty,
        };

        let resolution_id = resolution.id.clone();
        if db.insert_resolution_if_changed(&resolution, config.storage.max_resolutions())? {
            // New resolution recorded, update the bookmark's current pointer
            db.update_bookmark_resolution_id(&bookmark.id, &resolution_id)?;
            Some(resolution_id)
        } else {
            // Existing resolution was updated - we still need to update the bookmark's pointer
            // to ensure it points to the latest resolution (in case the duplicate was an older one)
            match db.list_resolutions(&bookmark.id, 1) {
                Ok(latest_res) => {
                    if let Some(latest) = latest_res.first() {
                        db.update_bookmark_resolution_id(&bookmark.id, &latest.id)?;
                        Some(latest.id.clone())
                    } else {
                        bookmark.current_resolution_id.clone()
                    }
                }
                Err(_) => bookmark.current_resolution_id.clone(),
            }
        }
    };

    // Recompute collection health for affected collections
    if !options.validate_only
        && let Ok(ids) = db.list_collection_ids_for_bookmark(&bookmark.id)
    {
        for collection_id in ids {
            if let Err(e) = db.recompute_collection_health(&collection_id) {
                eprintln!(
                    "codemark: warning: failed to recompute health for collection {}: {}",
                    collection_id, e
                );
            }
        }
    }

    // Include location info in result (for validate_only mode reporting)
    let (file_path, byte_range) = if result.method != ResolutionMethod::Failed {
        (Some(result.file_path), Some(format!("{}-{}", result.byte_range.0, result.byte_range.1)))
    } else {
        (None, None)
    };

    Ok(HealResult {
        bookmark_id: bookmark.id.clone(),
        resolution_id: new_resolution_id,
        previous_health,
        new_health: final_status,
        resolution_method: result.method,
        file_path,
        byte_range,
        was_skipped: false,
    })
}

/// Heal multiple bookmarks matching a filter.
pub async fn heal_bookmarks(
    db: &Database,
    filter: &crate::engine::bookmark::BookmarkFilter,
    config: &Config,
    options: &HealOptions,
) -> Result<Vec<HealResult>> {
    let bookmarks = db.list_bookmarks(filter)?;
    let mut results = Vec::new();

    for bookmark in &bookmarks {
        match heal_bookmark(db, bookmark, config, options).await {
            Ok(result) => results.push(result),
            Err(e) => {
                eprintln!("codemark: warning: failed to heal {}: {}", bookmark.id, e);
                // Continue with other bookmarks
            }
        }
    }

    Ok(results)
}

/// Heal all bookmarks in a collection.
///
/// Returns the number of bookmarks healed and any errors encountered.
pub async fn heal_collection(
    db: &Database,
    collection_id: &str,
    config: &Config,
    options: &HealOptions,
) -> Result<CollectionHealResult> {
    use crate::engine::bookmark::BookmarkFilter;

    let bookmarks = db.list_bookmarks(&BookmarkFilter {
        collection: Some(collection_id.to_string()),
        ..Default::default()
    })?;

    let mut healed = 0;
    let mut skipped = 0;
    let mut failed = 0;

    for bookmark in &bookmarks {
        match heal_bookmark(db, bookmark, config, options).await {
            Ok(result) => {
                if result.was_skipped {
                    // Intentionally skipped (e.g., git ancestor check)
                    skipped += 1;
                } else if result.resolution_method != ResolutionMethod::Failed {
                    // Successfully healed (or validated with no change needed)
                    healed += 1;
                } else {
                    // Failed to resolve (e.g., language parse error)
                    failed += 1;
                }
            }
            Err(_) => failed += 1,
        }
    }

    Ok(CollectionHealResult { healed, skipped, failed })
}

/// Result of healing a collection.
#[derive(Debug, Clone)]
pub struct CollectionHealResult {
    /// Number of bookmarks successfully healed (actual work done)
    pub healed: usize,
    /// Number of bookmarks that were skipped (e.g., git ancestor check)
    pub skipped: usize,
    /// Number of bookmarks that failed to heal
    pub failed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heal_result_creation() {
        let result = HealResult {
            bookmark_id: "test-bm".to_string(),
            resolution_id: Some("res-1".to_string()),
            previous_health: BookmarkHealth::Drifted,
            new_health: BookmarkHealth::Active,
            resolution_method: ResolutionMethod::Exact,
            file_path: Some("/path/to/file.rs".to_string()),
            byte_range: Some("100-200".to_string()),
            was_skipped: false,
        };

        assert_eq!(result.bookmark_id, "test-bm");
        assert_eq!(result.resolution_id, Some("res-1".to_string()));
        assert_eq!(result.previous_health, BookmarkHealth::Drifted);
        assert_eq!(result.new_health, BookmarkHealth::Active);
    }

    #[test]
    fn test_heal_options_default() {
        let options = HealOptions::default();
        assert!(!options.force);
        assert!(!options.auto_archive);
        assert!(!options.validate_only);
        assert_eq!(options.archive_after, 0);
    }

    /// Helper: create a temp git repo with `file.rs` containing `fn main() {}`,
    /// a `.codemark/` directory with a DB, and return (repo_path, db, head_commit).
    fn create_heal_test_env() -> (std::path::PathBuf, Database, String) {
        let tmp = std::env::temp_dir().join(format!("codemark_heal_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        let run = |args: &[&str]| {
            let status =
                std::process::Command::new("git").args(args).current_dir(&tmp).status().unwrap();
            assert!(status.success(), "git {:?} failed", args);
        };

        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(tmp.join("file.rs"), "fn main() {}").unwrap();
        run(&["add", "file.rs"]);
        run(&["commit", "-m", "initial"]);

        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&tmp)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let codemark_dir = tmp.join(".codemark");
        std::fs::create_dir_all(&codemark_dir).unwrap();
        let db_path = codemark_dir.join("codemark.db");
        let db = Database::open(&db_path).unwrap();

        (tmp, db, head)
    }

    /// Helper: create a minimal bookmark suitable for heal tests.
    /// Sets the content_hash to match what tree-sitter resolves for `fn main() {}`.
    fn make_test_bookmark(id: &str) -> Bookmark {
        use crate::engine::hash;
        Bookmark {
            id: id.to_string(),
            query: "(function_item) @target".to_string(),
            language: "rust".to_string(),
            file_path: "file.rs".to_string(),
            content_hash: Some(hash::content_hash("fn main() {}")),
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        }
    }

    #[tokio::test]
    async fn test_heal_creates_resolution() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();
        let bm = make_test_bookmark("heal-test-1");
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions { force: true, ..Default::default() };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        assert!(!result.was_skipped);
        assert!(result.resolution_id.is_some());
        assert_eq!(result.new_health, BookmarkHealth::Active);
        assert_eq!(result.resolution_method, ResolutionMethod::Exact);

        let _ = std::fs::remove_dir_all(&repo_path);
    }

    #[tokio::test]
    async fn test_heal_stores_resolution_in_db() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();
        let bm = make_test_bookmark("heal-test-2");
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions { force: true, ..Default::default() };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        // Verify the resolution was stored and retrievable
        let res_id = result.resolution_id.unwrap();
        let resolution = db.get_resolution(&res_id).unwrap().unwrap();
        assert_eq!(resolution.bookmark_id, bm.id);
        assert_eq!(resolution.method, ResolutionMethod::Exact);
        assert!(resolution.file_path.is_some());
        assert!(resolution.content_hash.is_some());

        let _ = std::fs::remove_dir_all(&repo_path);
    }

    #[tokio::test]
    async fn test_heal_updates_bookmark_health_in_db() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();
        let bm = make_test_bookmark("heal-test-3");
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions { force: true, ..Default::default() };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        // Verify the bookmark's health was updated in the DB
        let updated = db.get_bookmark(&bm.id).unwrap().unwrap();
        assert_eq!(updated.health, result.new_health);
        assert_eq!(updated.current_resolution_id, result.resolution_id);
        assert!(updated.last_resolved_at.is_some());

        let _ = std::fs::remove_dir_all(&repo_path);
    }

    #[tokio::test]
    async fn test_heal_validate_only_no_db_writes() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();
        let bm = make_test_bookmark("heal-test-4");
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions { validate_only: true, force: true, ..Default::default() };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        assert!(result.resolution_id.is_none(), "validate_only should not create a resolution");
        // Check that no resolutions exist for this bookmark
        let resolutions = db.list_resolutions(&bm.id, 100).unwrap();
        // The initial bookmark insertion creates one resolution, but no new ones from heal
        let initial_count = resolutions.len();
        assert!(initial_count <= 1, "only the initial resolution (if any) should exist");

        let _ = std::fs::remove_dir_all(&repo_path);
    }

    #[tokio::test]
    async fn test_heal_invalid_language_returns_failed() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();
        let mut bm = make_test_bookmark("heal-test-5");
        bm.language = "nonexistent".to_string();
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions { force: true, ..Default::default() };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        assert_eq!(result.resolution_method, ResolutionMethod::Failed);
        assert_eq!(result.new_health, bm.health);
        assert!(!result.was_skipped);

        let _ = std::fs::remove_dir_all(&repo_path);
    }

    #[tokio::test]
    async fn test_heal_auto_archive_stale() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();

        // Overwrite file.rs so (function_item) query no longer matches → Failed → Stale
        std::fs::write(repo_path.join("file.rs"), "// no functions here\n").unwrap();

        let mut bm = make_test_bookmark("heal-test-6");
        bm.health = BookmarkHealth::Stale;
        // Set stale_since to 60 days ago
        let sixty_days_ago = chrono::Utc::now() - chrono::Duration::days(60);
        bm.stale_since = Some(sixty_days_ago.to_rfc3339());
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions {
            force: true,
            auto_archive: true,
            archive_after: 30,
            ..Default::default()
        };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        assert_eq!(result.new_health, BookmarkHealth::Archived);

        let _ = std::fs::remove_dir_all(&repo_path);
    }

    #[tokio::test]
    async fn test_heal_auto_archive_not_triggered_when_active() {
        let (repo_path, db, _head) = create_heal_test_env();
        let config = Config::default();
        let bm = make_test_bookmark("heal-test-7");
        db.insert_bookmark(&bm).unwrap();

        let options = HealOptions {
            force: true,
            auto_archive: true,
            archive_after: 30,
            ..Default::default()
        };
        let result = heal_bookmark(&db, &bm, &config, &options).await.unwrap();

        assert_eq!(
            result.new_health,
            BookmarkHealth::Active,
            "active bookmarks should not be archived"
        );

        let _ = std::fs::remove_dir_all(&repo_path);
    }
}
