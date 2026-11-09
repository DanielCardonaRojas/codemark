//! Virtual File System (VFS) abstraction for file access.
//!
//! Provides a unified interface for reading files from local disk or git commits.

use crate::error::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Trait for file operations, supporting both local and versioned file access.
#[async_trait]
pub trait FileProvider: Send + Sync {
    /// Fetches the content of a file at a specific point in time (commit SHA).
    /// If commit is None, it should fetch from the current workspace/HEAD.
    async fn read_file(&self, path: &Path, commit: Option<&str>) -> Result<String>;

    /// Checks if a file exists at a specific point in time.
    async fn exists(&self, path: &Path, commit: Option<&str>) -> bool;

    /// Canonicalizes a path if possible.
    async fn canonicalize(&self, path: &Path) -> PathBuf;

    /// Returns the file's last-modified time, used by [`ParseCache`] to decide
    /// whether a cached parse tree is still valid.
    ///
    /// Defaults to `None`, meaning "treat as immutable" — appropriate for
    /// commit-pinned reads where the content never changes for a given path.
    /// Local working-tree providers override this so edits invalidate the cache.
    ///
    /// [`ParseCache`]: crate::parser::languages::ParseCache
    async fn mtime(&self, _path: &Path) -> Option<std::time::SystemTime> {
        None
    }
}

/// A file provider that reads from the local filesystem.
pub struct LocalFileProvider;

#[async_trait]
impl FileProvider for LocalFileProvider {
    async fn read_file(&self, path: &Path, _commit: Option<&str>) -> Result<String> {
        // For LocalFileProvider, we ignore the commit for now as it's intended to work on the local disk.
        // Later we could use git2 to read from a specific commit if needed.
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| crate::error::Error::Input(format!("cannot read {}: {e}", path.display())))
    }

    async fn exists(&self, path: &Path, _commit: Option<&str>) -> bool {
        tokio::fs::try_exists(path).await.unwrap_or(false)
    }

    async fn canonicalize(&self, path: &Path) -> PathBuf {
        tokio::fs::canonicalize(path).await.unwrap_or_else(|_| path.to_path_buf())
    }

    async fn mtime(&self, path: &Path) -> Option<std::time::SystemTime> {
        tokio::fs::metadata(path).await.ok().and_then(|m| m.modified().ok())
    }
}

/// A file provider that reads files as committed at a fixed git revision,
/// straight from the object store, with no checkout.
///
/// Every worktree shares one object store, so this works from any worktree, and
/// it is how a collection transfer resolves its steps on a branch that isn't
/// checked out. Paths handed to it are absolute paths under `repo_root` (what
/// [`resolve_bookmark_file_path`](crate::git::context::resolve_bookmark_file_path)
/// produces); they are mapped back to repo-relative paths for the tree lookup.
pub struct GitRevFileProvider {
    repo_root: PathBuf,
    /// Every spelling of the repo root that incoming paths may start with
    /// (as given, canonical, and any aliases such as a symlinked `--db` path).
    roots: Vec<PathBuf>,
    rev: String,
}

impl GitRevFileProvider {
    pub fn new(repo_root: &Path, rev: impl Into<String>) -> Self {
        Self {
            repo_root: repo_root.to_path_buf(),
            roots: root_spellings(repo_root),
            rev: rev.into(),
        }
    }

    /// Also accept paths under `alias`, another spelling of the repo root.
    pub fn with_alias(mut self, alias: &Path) -> Self {
        self.roots.extend(root_spellings(alias));
        self
    }

    /// The revision this provider reads from.
    pub fn rev(&self) -> &str {
        &self.rev
    }

    /// Map an absolute path back to a `/`-separated repo-relative path. Accepts
    /// paths under either the given or the canonical root, because a file that
    /// doesn't exist in the working tree can't be canonicalized.
    fn relative(&self, path: &Path) -> Option<String> {
        let rel = if path.is_relative() {
            path
        } else {
            self.roots.iter().find_map(|root| path.strip_prefix(root).ok())?
        };
        Some(
            rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
        )
    }
}

#[async_trait]
impl FileProvider for GitRevFileProvider {
    async fn read_file(&self, path: &Path, commit: Option<&str>) -> Result<String> {
        let rev = commit.unwrap_or(&self.rev);
        let rel = self.relative(path).ok_or_else(|| {
            crate::error::Error::Input(format!(
                "{} is not under repo root {}",
                path.display(),
                self.repo_root.display()
            ))
        })?;
        tracing::debug!(target: "codemark::git", rev, file = %rel, "reading file at revision");
        crate::git::context::read_file_at_commit(&self.repo_root, rev, &rel)
    }

    async fn exists(&self, path: &Path, commit: Option<&str>) -> bool {
        self.read_file(path, commit).await.is_ok()
    }

    async fn canonicalize(&self, path: &Path) -> PathBuf {
        // Revision reads never touch the working tree, so there is nothing to
        // resolve; the path is only a cache key and a lookup into the tree.
        path.to_path_buf()
    }
}

/// Reads the local filesystem, but under a different worktree than the one
/// paths are expressed against.
///
/// Every worktree shares the main checkout's database, so bookmark paths are
/// resolved against the main checkout's root. A transfer run from a linked
/// worktree must read *that worktree's* files, so this re-roots
/// `<main_root>/x` to `<worktree_root>/x` before touching disk.
pub struct WorktreeFileProvider {
    /// Every spelling of the main checkout's root (see [`GitRevFileProvider`]).
    main_roots: Vec<PathBuf>,
    worktree_root: PathBuf,
}

impl WorktreeFileProvider {
    pub fn new(main_root: &Path, worktree_root: &Path) -> Self {
        Self { main_roots: root_spellings(main_root), worktree_root: worktree_root.to_path_buf() }
    }

    /// Also accept paths under `alias`, another spelling of the main root.
    pub fn with_alias(mut self, alias: &Path) -> Self {
        self.main_roots.extend(root_spellings(alias));
        self
    }

    fn reroot(&self, path: &Path) -> PathBuf {
        match self.main_roots.iter().find_map(|root| path.strip_prefix(root).ok()) {
            Some(rel) => self.worktree_root.join(rel),
            None => path.to_path_buf(),
        }
    }
}

#[async_trait]
impl FileProvider for WorktreeFileProvider {
    async fn read_file(&self, path: &Path, commit: Option<&str>) -> Result<String> {
        LocalFileProvider.read_file(&self.reroot(path), commit).await
    }

    async fn exists(&self, path: &Path, commit: Option<&str>) -> bool {
        LocalFileProvider.exists(&self.reroot(path), commit).await
    }

    async fn canonicalize(&self, path: &Path) -> PathBuf {
        LocalFileProvider.canonicalize(&self.reroot(path)).await
    }

    async fn mtime(&self, path: &Path) -> Option<std::time::SystemTime> {
        LocalFileProvider.mtime(&self.reroot(path)).await
    }
}

/// `root` as given plus its canonical form (they differ through symlinks, e.g.
/// macOS `/var` vs `/private/var`). A path that doesn't exist on disk can't be
/// canonicalized, so callers must match either spelling.
fn root_spellings(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    if let Ok(canonical) = std::fs::canonicalize(root)
        && canonical != root
    {
        roots.push(canonical);
    }
    roots
}
