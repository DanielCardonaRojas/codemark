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

        if let (Some(ref head), Some(ref res)) = (current_head, previous_resolution.as_ref().and_then(|r| r.first())) {
            if let Some(ref res_commit) = res.commit_hash {
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
                        });
                    }
                    Ok(false) | Err(_) => {
                        // HEAD is ahead or unrelated, or git error - proceed with heal
                    }
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
            new_health: BookmarkHealth::Stale,
            resolution_method: ResolutionMethod::Failed,
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

    // Get current commit hash
    let commit_hash = git_context::detect_context(&std::env::current_dir()?)
        .and_then(|ctx| ctx.head_commit);

    // Create and insert resolution
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
    };

    let resolution_id = resolution.id.clone();
    let new_resolution_id = if db.insert_resolution_if_changed(&resolution, config.storage.max_resolutions())? {
        // New resolution recorded, update the bookmark's current pointer
        db.update_bookmark_resolution_id(&bookmark.id, &resolution_id)?;
        Some(resolution_id)
    } else {
        // Existing resolution updated, current_resolution_id remains correct
        bookmark.current_resolution_id.clone()
    };

    // Recompute collection health for affected collections
    if let Ok(ids) = db.list_collection_ids_for_bookmark(&bookmark.id) {
        for collection_id in ids {
            let _ = db.recompute_collection_health(&collection_id);
        }
    }

    Ok(HealResult {
        bookmark_id: bookmark.id.clone(),
        resolution_id: new_resolution_id,
        previous_health,
        new_health: final_status,
        resolution_method: result.method,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::Bookmark;

    #[test]
    fn test_heal_result_creation() {
        let result = HealResult {
            bookmark_id: "test-bm".to_string(),
            resolution_id: Some("res-1".to_string()),
            previous_health: BookmarkHealth::Drifted,
            new_health: BookmarkHealth::Active,
            resolution_method: ResolutionMethod::Exact,
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
        assert_eq!(options.archive_after, 0);
    }
}
