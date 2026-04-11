use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use git2::{Repository, Signature};

/// Git repository context captured at a point in time.
pub struct GitContext {
    pub repo_root: PathBuf,
    pub head_commit: Option<String>,
}

/// Detect git repo root and HEAD commit. Returns None if not in a git repo.
///
/// Uses `git rev-parse --git-common-dir` to find the repo root, which correctly
/// handles git worktrees (all worktrees share a common .git directory).
pub fn detect_context(from_path: &Path) -> Option<GitContext> {
    // Use git rev-parse to get the common git dir
    let output = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--git-common-dir")
        .current_dir(from_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let git_common_dir = String::from_utf8(output.stdout).ok()?;
    let git_common_dir = git_common_dir.trim();
    if git_common_dir.is_empty() {
        return None;
    }

    // Resolve the git common dir relative to the working directory
    // --git-common-dir may return a relative path like ".git" or an absolute path
    let git_path = if PathBuf::from(git_common_dir).is_absolute() {
        PathBuf::from(git_common_dir)
    } else {
        // Resolve relative path from the current directory
        let cwd = std::env::current_dir().ok()?;
        cwd.join(git_common_dir)
    };

    // The parent of the git common dir is the repo root
    // For a normal repo: .git -> parent is the repo root
    // For a worktree, this points to the main repo's .git, whose parent is the main repo root
    let repo_root = git_path.parent()?.to_path_buf();

    // Get HEAD commit using git2 (still reliable for this purpose)
    let repo = git2::Repository::discover(from_path).ok()?;
    let head_commit =
        repo.head().ok().and_then(|r| r.peel_to_commit().ok()).map(|c| c.id().to_string());

    Some(GitContext { repo_root, head_commit })
}

/// Make a path relative to the repo root, normalized (no ./, forward slashes only).
pub fn relative_to_root(repo_root: &Path, file_path: &Path) -> Result<String> {
    let abs_file = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(file_path)
    };

    let abs_file = canonicalize_best_effort(&abs_file);
    let abs_root = canonicalize_best_effort(repo_root);

    let rel = abs_file.strip_prefix(&abs_root).map_err(|_| {
        Error::Input(format!(
            "file {} is not under repo root {}",
            abs_file.display(),
            abs_root.display()
        ))
    })?;

    let normalized =
        rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");

    Ok(normalized)
}

/// Get the list of files changed between a commit and HEAD.
/// Returns paths relative to the repo root.
pub fn changed_files_since(from_path: &Path, since_commit: &str) -> Result<Vec<String>> {
    let repo = git2::Repository::discover(from_path)
        .map_err(|e| Error::Git(format!("cannot open repo: {e}")))?;

    let since_obj = repo
        .revparse_single(since_commit)
        .map_err(|e| Error::Git(format!("cannot resolve '{since_commit}': {e}")))?;
    let since_commit = since_obj
        .peel_to_commit()
        .map_err(|e| Error::Git(format!("'{since_commit}' is not a commit: {e}")))?;
    let since_tree =
        since_commit.tree().map_err(|e| Error::Git(format!("cannot get tree: {e}")))?;

    let head = repo
        .head()
        .map_err(|e| Error::Git(format!("cannot get HEAD: {e}")))?
        .peel_to_commit()
        .map_err(|e| Error::Git(format!("HEAD is not a commit: {e}")))?;
    let head_tree = head.tree().map_err(|e| Error::Git(format!("cannot get HEAD tree: {e}")))?;

    let diff = repo
        .diff_tree_to_tree(Some(&since_tree), Some(&head_tree), None)
        .map_err(|e| Error::Git(format!("cannot diff: {e}")))?;

    let mut files = Vec::new();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path() {
            files.push(path.to_string_lossy().to_string());
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

/// Read the contents of a file at a specific commit.
/// `file_path` must be relative to the repo root.
pub fn read_file_at_commit(from_path: &Path, commit_sha: &str, file_path: &str) -> Result<String> {
    let repo = git2::Repository::discover(from_path)
        .map_err(|e| Error::Git(format!("cannot open repo: {e}")))?;

    let obj = repo
        .revparse_single(commit_sha)
        .map_err(|e| Error::Git(format!("cannot resolve '{commit_sha}': {e}")))?;
    let commit = obj
        .peel_to_commit()
        .map_err(|e| Error::Git(format!("'{commit_sha}' is not a commit: {e}")))?;
    let tree = commit.tree().map_err(|e| Error::Git(format!("cannot get tree: {e}")))?;

    let entry = tree
        .get_path(Path::new(file_path))
        .map_err(|_| Error::Git(format!("file '{file_path}' not found at commit {commit_sha}")))?;

    let blob =
        entry.to_object(&repo).map_err(|e| Error::Git(format!("cannot read object: {e}")))?;
    let blob = blob.as_blob().ok_or_else(|| Error::Git("object is not a blob".into()))?;

    String::from_utf8(blob.content().to_vec())
        .map_err(|_| Error::Git("file is not valid UTF-8".into()))
}

/// Check if `ancestor_commit` is an ancestor of `descendant_commit`.
///
/// Returns Ok(true) if `ancestor_commit` is an ancestor of `descendant_commit`.
/// Returns Ok(false) if not (including unrelated branches).
/// Returns Err if repo/commits can't be accessed (graceful degradation).
pub fn is_ancestor(
    repo_path: &Path,
    ancestor_commit: &str,
    descendant_commit: &str,
) -> Result<bool> {
    let repo = Repository::discover(repo_path)
        .map_err(|e| Error::Git(format!("cannot open repo: {e}")))?;
    let ancestor = repo
        .revparse_single(ancestor_commit)
        .and_then(|obj: git2::Object| obj.peel_to_commit())
        .map_err(|e| Error::Git(format!("cannot resolve '{ancestor_commit}': {e}")))?;
    let descendant = repo
        .revparse_single(descendant_commit)
        .and_then(|obj: git2::Object| obj.peel_to_commit())
        .map_err(|e| Error::Git(format!("cannot resolve '{descendant_commit}': {e}")))?;

    // graph_descendant_of returns true if descendant is a descendant of ancestor
    // (i.e., if ancestor is an ancestor of descendant)
    repo.graph_descendant_of(descendant.id(), ancestor.id())
        .map_err(|e| Error::Git(format!("graph check failed: {e}")))
}

/// Find the nearest ancestor commit from a list of candidate commit hashes.
///
/// Given the current HEAD and a list of candidate commits, returns the commit hash
/// from the list that is an ancestor of HEAD and is "nearest" to HEAD (i.e., has
/// the fewest commits between it and HEAD).
///
/// Returns Ok(Some(commit_hash)) if a valid ancestor is found.
/// Returns Ok(None) if no candidate is an ancestor (or list is empty).
/// Returns Err if repo/HEAD can't be accessed.
pub fn find_nearest_ancestor(
    repo_path: &Path,
    candidate_hashes: &[String],
) -> Result<Option<String>> {
    if candidate_hashes.is_empty() {
        return Ok(None);
    }

    let repo = Repository::discover(repo_path)
        .map_err(|e| Error::Git(format!("cannot open repo: {e}")))?;

    let head = repo.head().map_err(|e| Error::Git(format!("cannot get HEAD: {e}")))?;
    let head_commit =
        head.peel_to_commit().map_err(|e| Error::Git(format!("HEAD is not a commit: {e}")))?;
    let head_id = head_commit.id();

    // Filter to ancestors of HEAD, then find the one with the shortest merge base distance
    let mut nearest: Option<(git2::Oid, usize)> = None;

    for candidate_hash in candidate_hashes {
        let obj = match repo.revparse_single(candidate_hash) {
            Ok(o) => o,
            Err(_) => continue, // Skip commits that don't exist
        };

        let commit = match obj.peel_to_commit() {
            Ok(c) => c,
            Err(_) => continue, // Skip non-commits
        };

        let candidate_id = commit.id();

        // Special case: if candidate IS HEAD, it's always the nearest (distance 0)
        if candidate_id == head_id {
            return Ok(Some(candidate_hash.to_string()));
        }

        // Check if this candidate is an ancestor of HEAD
        if !repo.graph_descendant_of(head_id, candidate_id).unwrap_or(false) {
            continue;
        }
        let obj = match repo.revparse_single(candidate_hash) {
            Ok(o) => o,
            Err(_) => continue, // Skip commits that don't exist
        };

        let commit = match obj.peel_to_commit() {
            Ok(c) => c,
            Err(_) => continue, // Skip non-commits
        };

        let candidate_id = commit.id();

        // Check if this candidate is an ancestor of HEAD
        if !repo.graph_descendant_of(head_id, candidate_id).unwrap_or(false) {
            continue;
        }

        // Find merge base to determine distance
        let merge_base = repo.merge_base(head_id, candidate_id);
        let distance = match merge_base {
            Ok(base_id) if base_id == candidate_id => {
                // Direct ancestry - count commits from candidate to HEAD
                count_commits_between(&repo, candidate_id, head_id).unwrap_or(usize::MAX)
            }
            Ok(_) => {
                // Not a direct ancestor - use a large distance to deprioritize
                usize::MAX
            }
            Err(_) => continue,
        };

        // Update nearest if this is closer
        match nearest {
            None => nearest = Some((candidate_id, distance)),
            Some((_, current_distance)) if distance < current_distance => {
                nearest = Some((candidate_id, distance));
            }
            Some(_) => {}
        }
    }

    Ok(nearest.map(|(oid, _)| oid.to_string()))
}

/// Count the number of commits from `start` (exclusive) to `end` (inclusive).
/// Used to determine how "close" an ancestor commit is to HEAD.
fn count_commits_between(repo: &Repository, start: git2::Oid, end: git2::Oid) -> Result<usize> {
    // Special case: if start == end, there are 0 commits between
    if start == end {
        return Ok(0);
    }

    let mut revwalk = repo.revwalk().map_err(|e| Error::Git(format!("revwalk failed: {e}")))?;
    revwalk.push(end).map_err(|e| Error::Git(format!("push failed: {e}")))?;
    revwalk.hide(start).map_err(|e| Error::Git(format!("hide failed: {e}")))?;

    Ok(revwalk.count())
}

/// Try to canonicalize; fall back to the original path if it doesn't exist yet.
fn canonicalize_best_effort(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn relative_to_root_normalizes() {
        let root = PathBuf::from("/repo");
        let file = PathBuf::from("/repo/src/main.swift");
        // Since these paths don't exist, canonicalize falls back to original
        let rel = relative_to_root(&root, &file).unwrap();
        assert_eq!(rel, "src/main.swift");
        assert!(!rel.starts_with("./"));
        assert!(!rel.contains('\\'));
    }

    #[test]
    fn relative_to_root_rejects_outside_file() {
        let root = PathBuf::from("/repo");
        let file = PathBuf::from("/other/file.swift");
        assert!(relative_to_root(&root, &file).is_err());
    }

    #[test]
    fn detect_context_in_real_repo() {
        // This test runs from within the codemark repo itself
        let ctx = detect_context(Path::new("."));
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert!(ctx.repo_root.exists());
        // HEAD should exist since we have commits
        assert!(ctx.head_commit.is_some());
        let commit = ctx.head_commit.unwrap();
        assert_eq!(commit.len(), 40); // full SHA hex
    }

    #[test]
    fn detect_context_outside_repo() {
        let tmp = std::env::temp_dir().join("codemark_test_no_git");
        let _ = fs::create_dir_all(&tmp);
        let ctx = detect_context(&tmp);
        // temp dir might be inside a git repo on some systems, so just check it doesn't panic
        // The important thing is graceful behavior
        drop(ctx);
        let _ = fs::remove_dir(&tmp);
    }

    #[test]
    fn is_ancestor_true() {
        // HEAD~1 should be an ancestor of HEAD in a repo with commits
        let ctx = detect_context(Path::new(".")).unwrap();
        let head = ctx.head_commit.unwrap();

        // Create a temp repo with known ancestry
        let tmp = std::env::temp_dir().join("codemark_test_ancestor_true");
        let _ = fs::create_dir_all(&tmp);

        let repo = Repository::init(&tmp).unwrap();
        let mut index = repo.index().unwrap();

        // Create first commit
        let path = tmp.join("test.txt");
        fs::write(&path, "initial").unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();
        let parent_commit_oid =
            repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).unwrap();
        let parent_commit = repo.find_commit(parent_commit_oid).unwrap();

        // Create second commit
        fs::write(&path, "modified").unwrap();
        let mut index = repo.index().unwrap();
        index.update_all(vec![Path::new("test.txt")], None).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let child_commit_oid =
            repo.commit(Some("HEAD"), &sig, &sig, "modified", &tree, &[&parent_commit]).unwrap();

        let parent_oid = parent_commit_oid.to_string();
        let child_oid = child_commit_oid.to_string();

        // Test that parent is ancestor of child
        let result = is_ancestor(&tmp, &parent_oid, &child_oid);
        assert!(result.is_ok());
        assert!(result.unwrap(), "parent should be ancestor of child");

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_ancestor_false_unrelated() {
        // Create a temp repo with unrelated branches
        let tmp = std::env::temp_dir().join("codemark_test_ancestor_unrelated");
        let _ = fs::create_dir_all(&tmp);

        let repo = Repository::init(&tmp).unwrap();
        let mut index = repo.index().unwrap();

        // Create first commit on main
        let path = tmp.join("test.txt");
        fs::write(&path, "initial").unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();
        let commit1_oid = repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[]).unwrap();

        // Create an orphan branch
        let mut index = repo.index().unwrap();
        fs::write(&path, "orphan").unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();

        // Create a commit without parent (orphan branch)
        let orphan_oid =
            repo.commit(Some("refs/heads/orphan"), &sig, &sig, "orphan", &tree, &[]).unwrap();

        let commit1_oid_str = commit1_oid.to_string();
        let orphan_oid_str = orphan_oid.to_string();

        // Test that unrelated commits return false
        let result = is_ancestor(&tmp, &commit1_oid_str, &orphan_oid_str);
        assert!(result.is_ok());
        assert!(!result.unwrap(), "unrelated commits should return false");

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_ancestor_missing_commit_graceful() {
        // Missing commits should return Err, not panic
        let tmp = std::env::temp_dir().join("codemark_test_ancestor_missing");
        let _ = fs::create_dir_all(&tmp);

        let _repo = Repository::init(&tmp).unwrap();
        // Empty repo, no commits

        let result = is_ancestor(&tmp, "nonexistent", "alsofake");
        assert!(result.is_err(), "missing commits should return Err");

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_nearest_ancestor_selects_closest() {
        // Create a repo with a chain of commits: A -> B -> C (HEAD)
        let tmp = std::env::temp_dir().join("codemark_test_nearest_ancestor");
        let _ = fs::create_dir_all(&tmp);

        let repo = Repository::init(&tmp).unwrap();
        let mut index = repo.index().unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();

        // Commit A
        let path = tmp.join("test.txt");
        fs::write(&path, "A").unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let commit_a_oid = repo.commit(Some("HEAD"), &sig, &sig, "A", &tree, &[]).unwrap();

        // Commit B
        fs::write(&path, "B").unwrap();
        let mut index = repo.index().unwrap();
        index.update_all(vec![Path::new("test.txt")], None).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let commit_b_oid = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "B",
                &tree,
                &[&repo.find_commit(commit_a_oid).unwrap()],
            )
            .unwrap();

        // Commit C (HEAD)
        fs::write(&path, "C").unwrap();
        let mut index = repo.index().unwrap();
        index.update_all(vec![Path::new("test.txt")], None).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let _commit_c_oid = repo
            .commit(
                Some("HEAD"),
                &sig,
                &sig,
                "C",
                &tree,
                &[&repo.find_commit(commit_b_oid).unwrap()],
            )
            .unwrap();

        let commit_a = commit_a_oid.to_string();
        let commit_b = commit_b_oid.to_string();

        // Test: from B and A, should select B (closer to HEAD)
        let candidates = vec![commit_a.clone(), commit_b.clone()];
        let result = find_nearest_ancestor(&tmp, &candidates);
        assert!(result.is_ok());
        let found = result.unwrap();
        assert_eq!(found, Some(commit_b), "should select the closer ancestor (B)");

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_nearest_ancestor_empty_list() {
        let tmp = std::env::temp_dir().join("codemark_test_nearest_empty");
        let _ = fs::create_dir_all(&tmp);

        let result = find_nearest_ancestor(&tmp, &[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None, "empty list should return None");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_nearest_ancestor_filters_non_ancestors() {
        // Create a repo where only one candidate is an ancestor
        let tmp = std::env::temp_dir().join("codemark_test_nearest_filter");
        let _ = fs::create_dir_all(&tmp);

        let repo = Repository::init(&tmp).unwrap();
        let mut index = repo.index().unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();

        // Commit A (ancestor)
        let path = tmp.join("test.txt");
        fs::write(&path, "A").unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let commit_a_oid = repo.commit(Some("HEAD"), &sig, &sig, "A", &tree, &[]).unwrap();

        // Commit B (HEAD)
        fs::write(&path, "B").unwrap();
        let mut index = repo.index().unwrap();
        index.update_all(vec![Path::new("test.txt")], None).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "B",
            &tree,
            &[&repo.find_commit(commit_a_oid).unwrap()],
        )
        .unwrap();

        // Create an orphan branch (not an ancestor of HEAD)
        fs::write(&path, "orphan").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("test.txt")).unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let orphan_oid =
            repo.commit(Some("refs/heads/orphan"), &sig, &sig, "orphan", &tree, &[]).unwrap();

        let commit_a = commit_a_oid.to_string();
        let orphan = orphan_oid.to_string();

        // Only A should be found (orphan is not an ancestor)
        let candidates = vec![commit_a.clone(), orphan.clone()];
        let result = find_nearest_ancestor(&tmp, &candidates);
        assert!(result.is_ok());
        let found = result.unwrap();
        assert_eq!(found, Some(commit_a), "should filter out non-ancestors");

        let _ = fs::remove_dir_all(&tmp);
    }
}
