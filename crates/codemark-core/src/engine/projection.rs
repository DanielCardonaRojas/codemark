//! UI status projection for bookmark resolutions.
//!
//! This module provides logic to project a resolution's stored health state
//! to a user-facing UI status based on:
//! 1. Whether the resolution is the current one (matches `bookmark.current_resolution_id`)
//! 2. The relationship between the resolution's commit and the current HEAD
//! 3. The resolution's stored health state

use crate::engine::bookmark::{Bookmark, BookmarkHealth, Resolution, UIStatus};
use crate::error::Result;
use crate::storage::db::Database;
use std::path::Path;

/// Project a resolution to a UI status based on current HEAD context.
///
/// # Arguments
///
/// * `resolution` - The resolution to project
/// * `bookmark` - The parent bookmark (to check if resolution is current)
/// * `current_head` - Optional override for HEAD commit (for time-travel queries)
/// * `repo_path` - Path to the git repository for ancestry checks
///
/// # Returns
///
/// A `UIStatus` enum value representing the projected UI status.
///
/// # Projection Logic
///
/// | Is Current | Is Ancestor of HEAD | Is Descendant of HEAD | Health    | UI Status  |
/// |------------|-------------------|----------------------|-----------|------------|
/// | true       | true              | false                | Active    | Healthy    |
/// | true       | true              | false                | Drifted   | Drifted    |
/// | true       | true              | false                | Stale     | Broken     |
/// | false      | true              | false                | Active    | Verified   |
/// | false      | true              | false                | Drifted   | Outdated   |
/// | any        | false             | true                 | any       | Future     |
/// | any        | false             | false                | any       | Broken     |
pub fn project_resolution_status(
    resolution: &Resolution,
    bookmark: &Bookmark,
    current_head: Option<&str>,
    repo_path: &Path,
) -> Result<UIStatus> {
    use crate::git::context as git_context;

    // Determine the effective HEAD
    let effective_head = if let Some(head) = current_head {
        Some(head.to_string())
    } else {
        git_context::detect_context(repo_path).and_then(|ctx| ctx.head_commit)
    };

    let effective_head = match effective_head {
        Some(h) => h,
        None => return Ok(UIStatus::Broken), // Can't determine HEAD
    };

    let resolution_commit = match &resolution.commit_hash {
        Some(c) => c.clone(),
        None => {
            if resolution.is_anchored {
                return Ok(UIStatus::Broken);
            } else {
                effective_head.clone()
            }
        }
    };

    // Check if resolution is current
    let is_current = bookmark.current_resolution_id.as_ref() == Some(&resolution.id);
    let is_anchored = resolution.is_anchored;

    // Check commit ancestry
    let is_at_head = resolution_commit == effective_head;
    let is_ancestor =
        is_at_head || git_context::is_ancestor(repo_path, &resolution_commit, &effective_head).unwrap_or(false);
    let is_descendant = !is_at_head
        && git_context::is_ancestor(repo_path, &effective_head, &resolution_commit).unwrap_or(false);

    // Project to UI status
    match (is_current, is_anchored, is_descendant, is_ancestor, resolution.health) {
        // Current, Active (must be at HEAD or ancestor)
        (true, true, false, true, BookmarkHealth::Active) => Ok(UIStatus::Healthy),
        (true, false, false, _, BookmarkHealth::Active) => Ok(UIStatus::UnanchoredHealthy),

        // Current, Drifted (must be at HEAD or ancestor)
        (true, true, false, true, BookmarkHealth::Drifted) => Ok(UIStatus::Drifted),
        (true, false, false, _, BookmarkHealth::Drifted) => Ok(UIStatus::UnanchoredDrifting),

        // Current, Stale/Broken
        (true, true, false, true, BookmarkHealth::Stale | BookmarkHealth::Archived) => {
            Ok(UIStatus::Broken)
        }
        (true, false, false, _, BookmarkHealth::Stale | BookmarkHealth::Archived) => {
            Ok(UIStatus::BrokenUnanchored)
        }

        // Past (not current, but in history)
        (false, _, false, true, BookmarkHealth::Active) => Ok(UIStatus::Verified),
        (false, _, false, true, BookmarkHealth::Drifted) => Ok(UIStatus::Outdated),

        // Future (commit is ahead of HEAD)
        (_, _, true, _, _) => Ok(UIStatus::Future),

        // Historical / Unrelated (Default to dim status if not anchored at HEAD/Ancestor)
        (_, _, false, false, BookmarkHealth::Active) => Ok(UIStatus::Verified),
        (_, _, false, false, BookmarkHealth::Drifted) => Ok(UIStatus::Outdated),

        _ => Ok(UIStatus::Broken),
    }
}

/// Project UI status for a single bookmark.
///
/// This function:
/// 1. Fetches the current resolution (if available)
/// 2. Projects the UI status based on current HEAD
/// 3. Adds the `ui_status` field to the bookmark
///
/// # Arguments
///
/// * `bookmark` - The bookmark to project
/// * `db` - Database reference for fetching resolutions
/// * `current_head` - Optional override for HEAD commit (for time-travel queries)
///
/// # Returns
///
/// The bookmark with `ui_status` populated.
pub fn project_ui_status_for_bookmark(
    mut bookmark: Bookmark,
    db: &Database,
    current_head: Option<&str>,
) -> Result<Bookmark> {
    let db_path = db.path();
    let repo_path = db_path.parent().unwrap_or(db_path);

    // Get the current resolution for this bookmark
    let ui_status = if let Some(ref resolution_id) = bookmark.current_resolution_id {
        match db.get_resolution(resolution_id) {
            Ok(Some(resolution)) => {
                match project_resolution_status(
                    &resolution,
                    &bookmark,
                    current_head,
                    repo_path,
                ) {
                    Ok(status) => Some(status.to_string()),
                    Err(e) => {
                        eprintln!("Warning: Failed to project UI status for bookmark {}: {:?}", bookmark.id, e);
                        None
                    }
                }
            }
            Ok(None) => {
                // Resolution ID exists but resolution not found - use raw health
                Some(bookmark.health.to_string())
            }
            Err(e) => {
                eprintln!("Warning: Failed to fetch resolution {} for bookmark {}: {:?}", resolution_id, bookmark.id, e);
                Some(bookmark.health.to_string())
            }
        }
    } else {
        // No current resolution - use raw health
        Some(bookmark.health.to_string())
    };

    bookmark.ui_status = ui_status;
    Ok(bookmark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{Bookmark, Resolution, ResolutionMethod};
    use git2::Repository;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Helper to create a temporary git repository with a commit chain.
    fn create_test_repo() -> Result<(PathBuf, String, String)> {
        let tmp = std::env::temp_dir().join(format!("codemark_test_projection_{}", Uuid::new_v4()));
        fs::create_dir_all(&tmp)?;

        let repo = Repository::init(&tmp)?;
        let mut index = repo.index()?;
        let sig = git2::Signature::now("Test User", "test@example.com")?;

        // Create initial file and commit
        let file_path = tmp.join("test.txt");
        fs::write(&file_path, "initial")?;
        index.add_path(Path::new("test.txt"))?;
        let tree_id = index.write_tree()?;
        {
            let tree = repo.find_tree(tree_id)?;
            let commit_a_oid = repo.commit(Some("HEAD"), &sig, &sig, "A", &tree, &[])?;
            let commit_a = commit_a_oid.to_string();

            // Create second commit
            fs::write(&file_path, "modified")?;
            let mut index = repo.index()?;
            index.update_all(vec![Path::new("test.txt")], None)?;
            let tree_id = index.write_tree()?;
            let tree = repo.find_tree(tree_id)?;
            let commit_b_oid = repo.commit(Some("HEAD"), &sig, &sig, "B", &tree, &[&repo.find_commit(commit_a_oid)?])?;
            let commit_b = commit_b_oid.to_string();

            // Drop tree before repo
            drop(tree);

            Ok((tmp, commit_a, commit_b))
        }
    }

    fn create_test_resolution(
        commit_hash: Option<String>,
        health: BookmarkHealth,
        is_current: bool,
    ) -> (Resolution, Bookmark) {
        let resolution_id = Uuid::new_v4().to_string();
        let current_resolution_id = if is_current { Some(resolution_id.clone()) } else { None };

        let resolution = Resolution {
            id: resolution_id,
            bookmark_id: Uuid::new_v4().to_string(),
            resolved_at: "2024-01-01T00:00:00Z".to_string(),
            health,
            commit_hash: commit_hash.clone(),
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("test.txt".to_string()),
            byte_range: Some("0-100".to_string()),
            line_range: Some("1-5".to_string()),
            content_hash: Some("abc123".to_string()),
            headline: None,
            snapshot: None,
            breadcrumbs: None,
            is_anchored: commit_hash.is_some(),
        };

        let bookmark = Bookmark {
            id: Uuid::new_v4().to_string(),
            query: "(function_declaration) @target".to_string(),
            language: "rust".to_string(),
            file_path: "test.txt".to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: Some(ResolutionMethod::Exact),
            last_resolved_at: Some("2024-01-01T00:00:00Z".to_string()),
            stale_since: None,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id,
            repo_id: None,
            ui_status: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        (resolution, bookmark)
    }

    #[test]
    fn test_current_active_unanchored_returns_unanchored_healthy() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (mut resolution, bookmark) = create_test_resolution(None, BookmarkHealth::Active, true);
        resolution.is_anchored = false;
        eprintln!("DEBUG test: is_anchored set to: {}", resolution.is_anchored);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::UnanchoredHealthy);
    }

    #[test]
    fn test_current_drifted_unanchored_returns_unanchored_drifting() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (mut resolution, bookmark) = create_test_resolution(None, BookmarkHealth::Drifted, true);
        resolution.is_anchored = false;

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::UnanchoredDrifting);
    }

    #[test]
    fn test_current_stale_unanchored_returns_broken_unanchored() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (mut resolution, bookmark) = create_test_resolution(None, BookmarkHealth::Stale, true);
        resolution.is_anchored = false;

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::BrokenUnanchored);
    }

    #[test]
    fn test_current_drifted_at_head_returns_drifted() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (resolution, bookmark) =
            create_test_resolution(Some(commit_b.clone()), BookmarkHealth::Drifted, true);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Drifted);
    }

    #[test]
    fn test_current_stale_at_head_returns_broken() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (resolution, bookmark) = create_test_resolution(Some(commit_b.clone()), BookmarkHealth::Stale, true);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Broken);
    }

    #[test]
    fn test_past_active_returns_verified() {
        let (_tmp, commit_a, commit_b) = create_test_repo().unwrap();

        let (resolution, bookmark) = create_test_resolution(Some(commit_a.clone()), BookmarkHealth::Active, false);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Verified);
    }

    #[test]
    fn test_past_drifted_returns_outdated() {
        let (_tmp, commit_a, commit_b) = create_test_repo().unwrap();

        let (resolution, bookmark) =
            create_test_resolution(Some(commit_a.clone()), BookmarkHealth::Drifted, false);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Outdated);
    }

    #[test]
    fn test_future_resolution_returns_future() {
        let (_tmp, commit_a, commit_b) = create_test_repo().unwrap();

        // Simulate a future resolution (commit_b is ahead of commit_a)
        let (resolution, bookmark) =
            create_test_resolution(Some(commit_b.clone()), BookmarkHealth::Active, false);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_a), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Future);
    }

    #[test]
    fn test_no_resolution_commit_returns_broken() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (mut resolution, bookmark) = create_test_resolution(None, BookmarkHealth::Active, true);
        resolution.is_anchored = true; // Anchored, but no commit hash -> Broken

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Broken);
    }

    #[test]
    fn test_no_git_context_returns_broken() {
        let tmp = std::env::temp_dir().join(format!("codemark_test_no_git_{}", Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let (resolution, bookmark) = create_test_resolution(Some("abc123".to_string()), BookmarkHealth::Active, true);

        // Empty directory has no git context
        let result = project_resolution_status(&resolution, &bookmark, None, &tmp).unwrap();
        assert_eq!(result, UIStatus::Broken);

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_unrelated_commit_returns_broken() {
        // Create two separate repos with unrelated commits
        let (tmp1, _commit_a, commit_b1) = create_test_repo().unwrap();
        let (tmp2, _commit_c, _commit_d2) = create_test_repo().unwrap();

        // Use a fake commit hash that doesn't exist in either repo
        let fake_commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

        let (resolution, bookmark) =
            create_test_resolution(Some(fake_commit.to_string()), BookmarkHealth::Active, true);

        // fake_commit is not in tmp1's history
        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b1), &tmp1).unwrap();
        assert_eq!(result, UIStatus::Broken);

        let _ = fs::remove_dir_all(&tmp1);
        let _ = fs::remove_dir_all(&tmp2);
    }

    #[test]
    fn test_current_archived_at_head_returns_broken() {
        let (_tmp, _commit_a, commit_b) = create_test_repo().unwrap();

        let (resolution, bookmark) =
            create_test_resolution(Some(commit_b.clone()), BookmarkHealth::Archived, true);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Broken);
    }

    #[test]
    fn test_current_active_ancestor_of_head_returns_healthy() {
        let (_tmp, commit_a, commit_b) = create_test_repo().unwrap();

        // Resolution at commit_a, but HEAD is at commit_b (commit_a is ancestor)
        let (resolution, bookmark) = create_test_resolution(Some(commit_a.clone()), BookmarkHealth::Active, true);

        let result = project_resolution_status(&resolution, &bookmark, Some(&commit_b), &_tmp).unwrap();
        assert_eq!(result, UIStatus::Healthy);
    }

    #[test]
    fn test_project_ui_status_for_bookmark_with_no_resolution() {
        use crate::storage::db::Database;
        use std::fs;
        use uuid::Uuid;

        let tmp = std::env::temp_dir().join(format!("codemark_test_ui_status_{}", Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let db = Database::open_in_memory().unwrap();
        let repo = git2::Repository::init(&tmp).unwrap();
        let sig = git2::Signature::now("Test User", "test@example.com").unwrap();

        // Create initial commit
        let file_path = tmp.join("test.txt");
        fs::write(&file_path, "initial").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let commit_oid = repo.commit(Some("HEAD"), &sig, &sig, "A", &tree, &[]).unwrap();
        let _commit = commit_oid.to_string();

        // Create a bookmark with no current resolution
        let bookmark = Bookmark {
            id: Uuid::new_v4().to_string(),
            query: "(function_declaration) @target".to_string(),
            language: "rust".to_string(),
            file_path: "test.txt".to_string(),
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
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let result = project_ui_status_for_bookmark(bookmark, &db, None).unwrap();
        assert_eq!(result.ui_status, Some("active".to_string()));

        let _ = fs::remove_dir_all(&tmp);
    }
}
