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
