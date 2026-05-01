use std::path::Path;
use async_trait::async_trait;
use crate::error::Result;

#[async_trait]
pub trait FileProvider: Send + Sync {
    /// Fetches the content of a file at a specific point in time (commit SHA).
    /// If commit is None, it should fetch from the current workspace/HEAD.
    async fn read_file(&self, path: &Path, commit: Option<&str>) -> Result<String>;
    
    /// Checks if a file exists at a specific point in time.
    async fn exists(&self, path: &Path, commit: Option<&str>) -> bool;
}

pub struct LocalFileProvider;

#[async_trait]
impl FileProvider for LocalFileProvider {
    async fn read_file(&self, path: &Path, _commit: Option<&str>) -> Result<String> {
        // For LocalFileProvider, we ignore the commit for now as it's intended to work on the local disk.
        // Later we could use git2 to read from a specific commit if needed.
        std::fs::read_to_string(path)
            .map_err(|e| crate::error::Error::Input(format!("cannot read {}: {e}", path.display())))
    }

    async fn exists(&self, path: &Path, _commit: Option<&str>) -> bool {
        path.exists()
    }
}
