//! UI status projection for bookmark resolutions.
//!
//! This module provides logic to project a resolution's stored health state
//! to a user-facing UI status based on:
//! 1. Whether the resolution is the current one (matches the bookmark's current pointer).
//! 2. Whether the resolution was created with uncommitted changes (is_dirty).
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
    let repo_path =
        if repo_path.is_file() { repo_path.parent().unwrap_or(repo_path) } else { repo_path };

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
            if !resolution.is_dirty {
                return Ok(UIStatus::Broken);
            } else {
                head.clone()
            }
        }
    };

    // Check if resolution is current
    let is_current = bookmark.current_resolution_id.as_ref() == Some(&resolution.id);
    let is_anchored = !resolution.is_dirty;

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
        // --- Current Pointer at HEAD (100% confidence) ---
        (true, Ancestry::AtHead, true, BookmarkHealth::Active) => Ok(UIStatus::Healthy),
        (true, Ancestry::AtHead, false, BookmarkHealth::Active) => Ok(UIStatus::UnanchoredHealthy),

        (true, Ancestry::AtHead, true, BookmarkHealth::Drifted) => Ok(UIStatus::Drifted),
        (true, Ancestry::AtHead, false, BookmarkHealth::Drifted) => {
            Ok(UIStatus::UnanchoredDrifting)
        }

        // --- Current Pointer at Ancestor (was correct, HEAD has moved on) ---
        // The anchored/unanchored distinction is irrelevant here: the resolution is
        // at a past commit, so what matters is that HEAD has moved on. Both anchored
        // and unanchored resolutions at an ancestor are historical facts.
        (true, Ancestry::Ancestor, _, BookmarkHealth::Active) => Ok(UIStatus::Verified),
        (true, Ancestry::Ancestor, _, BookmarkHealth::Drifted) => Ok(UIStatus::Outdated),

        // --- Stale / Archived are always broken ---
        (true, _, true, BookmarkHealth::Stale | BookmarkHealth::Archived) => Ok(UIStatus::Broken),
        (true, _, false, BookmarkHealth::Stale | BookmarkHealth::Archived) => {
            Ok(UIStatus::BrokenUnanchored)
        }

        // --- Current Pointer but Unrelated (likely from another branch) ---
        (true, Ancestry::Unrelated, _, BookmarkHealth::Active) => Ok(UIStatus::Verified),
        (true, Ancestry::Unrelated, _, BookmarkHealth::Drifted) => Ok(UIStatus::Outdated),

        // --- Non-Current (Historical) ---
        (false, Ancestry::AtHead | Ancestry::Ancestor, _, BookmarkHealth::Active) => {
            Ok(UIStatus::Verified)
        }
        (false, Ancestry::AtHead | Ancestry::Ancestor, _, BookmarkHealth::Drifted) => {
            Ok(UIStatus::Outdated)
        }

        // --- Descendant (The future) ---
        (_, Ancestry::Descendant, _, _) => Ok(UIStatus::Future),

        _ => Ok(UIStatus::Broken),
    }
}

/// Compute the projected UI status for a bookmark.
///
/// Pure computation that fetches the current resolution from the database
/// and projects the status. Returns a typed `UIStatus` without mutating the bookmark.
///
/// # Arguments
///
/// * `bookmark` - The bookmark to project
/// * `db` - Database reference for fetching the current resolution
/// * `current_head` - Optional override for HEAD commit (for time-travel queries)
pub fn compute_bookmark_ui_status(
    bookmark: &Bookmark,
    db: &Database,
    current_head: Option<&str>,
) -> Result<UIStatus> {
    let repo_path = db.path();
    if let Some(ref resolution_id) = bookmark.current_resolution_id {
        match db.get_resolution(resolution_id) {
            Ok(Some(resolution)) => {
                project_resolution_status(&resolution, bookmark, current_head, repo_path)
            }
            Ok(None) => Ok(UIStatus::from(bookmark.health)),
            Err(_) => Ok(UIStatus::from(bookmark.health)),
        }
    } else {
        Ok(UIStatus::from(bookmark.health))
    }
}

impl From<BookmarkHealth> for UIStatus {
    fn from(health: BookmarkHealth) -> Self {
        match health {
            BookmarkHealth::Active => UIStatus::Healthy,
            BookmarkHealth::Drifted => UIStatus::Drifted,
            BookmarkHealth::Stale | BookmarkHealth::Archived => UIStatus::Broken,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::bookmark::{BookmarkHealth, ResolutionMethod};
    use std::fs;
    use std::path::PathBuf;

    struct TestRepo {
        path: PathBuf,
        commit_a: String,
        commit_b: String,
    }

    fn create_test_repo() -> TestRepo {
        let tmp = std::env::temp_dir().join(format!("codemark-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let run = |args: &[&str]| {
            let status =
                std::process::Command::new("git").args(args).current_dir(&tmp).status().unwrap();
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

    fn create_test_resolution(
        commit: Option<String>,
        health: BookmarkHealth,
        anchored: bool,
    ) -> (Resolution, Bookmark) {
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
            is_dirty: !anchored,
        };

        (resolution, bookmark)
    }

    #[test]
    fn test_at_head_returns_healthy() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Active, true);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Healthy);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_ancestor_returns_verified() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a), BookmarkHealth::Active, true);

        // Ancestor means HEAD has moved on — was correct at that commit but no guarantee now
        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_b), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Verified);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_unrelated_commit_returns_verified() {
        let repo = create_test_repo();
        let (resolution, bookmark) = create_test_resolution(
            Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            BookmarkHealth::Active,
            true,
        );

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_b), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Verified);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_current_drifted_at_head_returns_drifted() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Drifted, true);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Drifted);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_current_stale_at_head_returns_broken() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Stale, true);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Broken);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_future_commit_returns_future() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_b), BookmarkHealth::Active, true);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Future);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_unanchored_returns_unanchored() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Active, false);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::UnanchoredHealthy);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_unanchored_ancestor_returns_verified() {
        let repo = create_test_repo();
        // Unanchored resolution at an ancestor commit — the anchoring distinction
        // is irrelevant since HEAD has moved on; should be treated as historical.
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Active, false);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_b), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Verified);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_unanchored_ancestor_drifted_returns_outdated() {
        let repo = create_test_repo();
        let (resolution, bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Drifted, false);

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_b), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Outdated);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_non_current_active_returns_verified() {
        let repo = create_test_repo();
        let (resolution, mut bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Active, true);
        // Make this resolution non-current by pointing to a different ID
        bookmark.current_resolution_id = Some("other-resolution-id".to_string());

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Verified);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_non_current_drifted_returns_outdated() {
        let repo = create_test_repo();
        let (resolution, mut bookmark) =
            create_test_resolution(Some(repo.commit_a.clone()), BookmarkHealth::Drifted, true);
        bookmark.current_resolution_id = Some("other-resolution-id".to_string());

        let result =
            project_resolution_status(&resolution, &bookmark, Some(&repo.commit_a), &repo.path)
                .unwrap();
        assert_eq!(result, UIStatus::Outdated);

        fs::remove_dir_all(&repo.path).unwrap();
    }

    #[test]
    fn test_no_git_context_returns_broken() {
        let tmp = std::env::temp_dir().join(format!("not-a-git-repo-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();

        let (resolution, bookmark) =
            create_test_resolution(Some("abc".to_string()), BookmarkHealth::Active, true);
        let result = project_resolution_status(&resolution, &bookmark, None, &tmp).unwrap();
        assert_eq!(result, UIStatus::Broken);

        fs::remove_dir_all(&tmp).unwrap();
    }
}
