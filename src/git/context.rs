use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Git repository context captured at a point in time.
pub struct GitContext {
    pub repo_root: PathBuf,
    pub head_commit: Option<String>,
}

/// Detect git repo root and HEAD commit. Returns None if not in a git repo.
pub fn detect_context(from_path: &Path) -> Option<GitContext> {
    let repo = git2::Repository::discover(from_path).ok()?;
    let repo_root = repo.workdir()?.to_path_buf();
    let head_commit = repo
        .head()
        .ok()
        .and_then(|r| r.peel_to_commit().ok())
        .map(|c| c.id().to_string());

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

    let rel = abs_file
        .strip_prefix(&abs_root)
        .map_err(|_| Error::Input(format!("file {} is not under repo root {}", abs_file.display(), abs_root.display())))?;

    let normalized = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

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
    let since_tree = since_commit.tree()
        .map_err(|e| Error::Git(format!("cannot get tree: {e}")))?;

    let head = repo.head()
        .map_err(|e| Error::Git(format!("cannot get HEAD: {e}")))?
        .peel_to_commit()
        .map_err(|e| Error::Git(format!("HEAD is not a commit: {e}")))?;
    let head_tree = head.tree()
        .map_err(|e| Error::Git(format!("cannot get HEAD tree: {e}")))?;

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
    let tree = commit
        .tree()
        .map_err(|e| Error::Git(format!("cannot get tree: {e}")))?;

    let entry = tree
        .get_path(Path::new(file_path))
        .map_err(|_| Error::Git(format!("file '{file_path}' not found at commit {commit_sha}")))?;

    let blob = entry
        .to_object(&repo)
        .map_err(|e| Error::Git(format!("cannot read object: {e}")))?;
    let blob = blob
        .as_blob()
        .ok_or_else(|| Error::Git("object is not a blob".into()))?;

    String::from_utf8(blob.content().to_vec())
        .map_err(|_| Error::Git("file is not valid UTF-8".into()))
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
}
