//! UI status projection for bookmark resolutions.
//!
//! This module provides logic to project a resolution's stored health state
//! to a user-facing UI status based on:
//! 1. Whether the resolution is the current one (matches the bookmark's current pointer).
//! 2. Whether the resolution was created with uncommitted changes (is_anchored).
//! 3. The ancestry relationship between the resolution's commit and the current HEAD.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::engine::bookmark::{Bookmark, BookmarkHealth, Resolution};
use crate::error::Result;
use crate::git::context as git_context;
use crate::storage::db::Database;

/// Projected UI health labels for bookmarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UIStatus {
    /// 🟢 Perfect match, committed at HEAD (or ancestor).
    Healthy,
    /// 🟡 Perfect match, but currently uncommitted.
    UnanchoredHealthy,
    /// 🟡 Found at HEAD/ancestor, but code content has changed.
    Drifted,
    /// 🟠 Drifted found while uncommitted.
    UnanchoredDrifting,
    /// 🔴 Not found at the current HEAD.
    Broken,
    /// 🔴 Not found while uncommitted.
    BrokenUnanchored,
    /// ⚪ Was a perfect match in a previous commit (Historical/Unrelated).
    Verified,
    /// ⚪ Was a partial match in a previous commit (Historical/Unrelated).
    Outdated,
    /// 🔵 Recorded at a commit ahead of current HEAD.
    Future,
}

impl fmt::Display for UIStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            UIStatus::Healthy => "healthy",
            UIStatus::UnanchoredHealthy => "unanchored_healthy",
            UIStatus::Drifted => "drifted",
            UIStatus::UnanchoredDrifting => "unanchored_drifting",
            UIStatus::Broken => "broken",
            UIStatus::BrokenUnanchored => "broken_unanchored",
            UIStatus::Verified => "verified",
            UIStatus::Outdated => "outdated",
            UIStatus::Future => "future",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for UIStatus {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "healthy" => Ok(UIStatus::Healthy),
            "unanchored_healthy" => Ok(UIStatus::UnanchoredHealthy),
            "drifted" => Ok(UIStatus::Drifted),
            "unanchored_drifting" => Ok(UIStatus::UnanchoredDrifting),
            "broken" => Ok(UIStatus::Broken),
            "broken_unanchored" => Ok(UIStatus::BrokenUnanchored),
            "verified" => Ok(UIStatus::Verified),
            "outdated" => Ok(UIStatus::Outdated),
            "future" => Ok(UIStatus::Future),
            _ => Err(format!("unknown UI status: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ancestry {
    AtHead,
    Ancestor,
    Descendant,
    Unrelated,
}

/// Project the UI status for a resolution based on current HEAD.
pub fn project_resolution_status(
    resolution: &Resolution,
    bookmark: &Bookmark,
    current_head: Option<&str>,
    repo_path: &Path,
) -> Result<UIStatus> {
    // Ensure we have a directory path for git operations
    let repo_path = if repo_path.is_file() {
        repo_path.parent().unwrap_or(repo_path)
    } else {
        repo_path
    };

    // Determine the effective HEAD
    let head = if let Some(head) = current_head {
        Some(head.to_string())
    } else {
        git_context::detect_context(repo_path).and_then(|ctx| ctx.head_commit)
    };

    let head = match head {
        Some(h) => h,
        None => return Ok(UIStatus::Broken), // Can't determine HEAD
    };

    let resolution_commit = match &resolution.commit_hash {
        Some(c) => c.clone(),
        None => {
            if resolution.is_anchored {
                return Ok(UIStatus::Broken);
            } else {
                head.clone()
            }
        }
    };

    // Check if resolution is current
    let is_current = bookmark.current_resolution_id.as_ref() == Some(&resolution.id);
    let is_anchored = resolution.is_anchored;

    // Determine ancestry relation
    let ancestry = if head == resolution_commit {
        Ancestry::AtHead
    } else if git_context::is_ancestor(repo_path, &resolution_commit, &head).unwrap_or(false) {
        Ancestry::Ancestor
    } else if git_context::is_ancestor(repo_path, &head, &resolution_commit).unwrap_or(false) {
        Ancestry::Descendant
    } else {
        Ancestry::Unrelated
    };

    // Project to UI status
    match (is_current, ancestry, is_anchored, resolution.health) {
        // --- Current Pointer ---
        
        // At HEAD or Ancestor (Historical but still valid pointer)
        (true, Ancestry::AtHead | Ancestry::Ancestor, true, BookmarkHealth::Active) => Ok(UIStatus::Healthy),
        (true, Ancestry::AtHead | Ancestry::Ancestor, false, BookmarkHealth::Active) => Ok(UIStatus::UnanchoredHealthy),
        
        (true, Ancestry::AtHead | Ancestry::Ancestor, true, BookmarkHealth::Drifted) => Ok(UIStatus::Drifted),
        (true, Ancestry::AtHead | Ancestry::Ancestor, false, BookmarkHealth::Drifted) => Ok(UIStatus::UnanchoredDrifting),
        
        // Stale is always broken
        (true, _, true, BookmarkHealth::Stale | BookmarkHealth::Archived) => Ok(UIStatus::Broken),
        (true, _, false, BookmarkHealth::Stale | BookmarkHealth::Archived) => Ok(UIStatus::BrokenUnanchored),

        // Unrelated but Current Pointer (Likely from another branch)
        (true, Ancestry::Unrelated, _, BookmarkHealth::Active) => Ok(UIStatus::Verified),
        (true, Ancestry::Unrelated, _, BookmarkHealth::Drifted) => Ok(UIStatus::Outdated),

        // --- Non-Current (Historical) ---
        
        (false, Ancestry::AtHead | Ancestry::Ancestor, _, BookmarkHealth::Active) => Ok(UIStatus::Verified),
        (false, Ancestry::AtHead | Ancestry::Ancestor, _, BookmarkHealth::Drifted) => Ok(UIStatus::Outdated),

        // --- Descendant (The future) ---
        (_, Ancestry::Descendant, _, _) => Ok(UIStatus::Future),

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
    let repo_path = db.path();

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
    use std::fs;
    use std::path::PathBuf;
    use crate::engine::bookmark::{BookmarkHealth, ResolutionMethod};

    struct TestRepo {
        path: PathBuf,
        commit_a: String,
        commit_b: String,
    }

    fn create_test_repo() -> TestRepo {
        let tmp = std::env::temp_dir().join(format!("codemark-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(&tmp)
                .status()
                .unwrap();
            assert!(status.success());
        };

        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        
        fs::write(tmp.join("file.rs"), "fn main() {}").unwrap();
        run(&["add", "file.rs"]);
        run(&["commit", "-m", "initial"]);
        let commit_a = std::process::Command::new("git")
            .args(&["rev-parse", "HEAD"])
            .current_dir(&tmp)
            .output()
            .unwrap()
            .stdout;
        let commit_a = String::from_utf8(commit_a).unwrap().trim().to_string();

        fs::write(tmp.join("file.rs"), "fn main() { println!(\"hello\"); }").unwrap();
        run(&["add", "file.rs"]);
        run(&["commit", "-m", "second"]);
        let commit_b = std::process::Command::new("git")
            .args(&["rev-parse", "HEAD"])
            .current_dir(&tmp)
            .output()
            .unwrap()
            .stdout;
        let commit_b = String::from_utf8(commit_b).unwrap().trim().to_string();

        TestRepo { path: tmp, commit_a, commit_b }
    }

    fn create_test_resolution(commit: Option<String>, health: BookmarkHealth, anchored: bool) -> (Resolution, Bookmark) {
        let bookmark_id = uuid::Uuid::new_v4().to_string();
        let resolution_id = uuid::Uuid::new_v4().to_string();
        
        let bookmark = Bookmark {
            id: bookmark_id.clone(),
            query: "test".to_string(),
            language: "rust".to_string(),
            file_path: "file.rs".to_string(),
            content_hash: None,
            commit_hash: None,
            health,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "now".to_string(),
            created_by: None,
            current_resolution_id: Some(resolution_id.clone()),
            repo_id: None,
            ui_status: None,
            tags: vec![],
            annotations: vec![],
            comments: vec![],
        };

        let resolution = Resolution {
            id: resolution_id,
            bookmark_id,
            resolved_at: "now".to_string(),
            health,
            commit_hash: commit,
            method: ResolutionMethod::Exact,
            match_count: Some(1),
            file_path: Some("file.rs".to_string()),
            byte_range: None,
            line_range: None,
            content_hash: None,
            headline: None,
            snapshot: None,
            breadcrumbs: None,
            is_anchored: anchored,
        };

        (resolution, bookmark)
    }

    #[test]
    fn test_at_head_returns_healthy() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Active, true);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path).unwrap();
        assert_eq!(result, UIStatus::Healthy);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_ancestor_returns_healthy() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some(repo.commit_a), BookmarkHealth::Active, true);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_b), &repo.path).unwrap();
        assert_eq!(result, UIStatus::Healthy);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_unrelated_commit_returns_verified() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some("0123456789abcdef0123456789abcdef01234567".to_string()), BookmarkHealth::Active, true);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_b), &repo.path).unwrap();
        assert_eq!(result, UIStatus::Verified);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_current_drifted_at_head_returns_drifted() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Drifted, true);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path).unwrap();
        assert_eq!(result, UIStatus::Drifted);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_current_stale_at_head_returns_broken() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Stale, true);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path).unwrap();
        assert_eq!(result, UIStatus::Broken);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_future_commit_returns_future() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some(repo.commit_b), BookmarkHealth::Active, true);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path).unwrap();
        assert_eq!(result, UIStatus::Future);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_unanchored_returns_unanchored() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Active, false);
        
        let result = project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path).unwrap();
        assert_eq!(result, UIStatus::UnanchoredHealthy);
        
        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_no_git_context_returns_broken() {
        let tmp = std::env::temp_dir().join(format!("not-a-git-repo-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let (resolution, bookmark) = create_test_resolution(Some("abc".to_string()), BookmarkHealth::Active, true);
        let result = project_resolution_status(&resolution, &bookmark, None, &tmp).unwrap();
        assert_eq!(result, UIStatus::Broken);

        fs::remove_dir_all(&tmp).unwrap();
    }
}
