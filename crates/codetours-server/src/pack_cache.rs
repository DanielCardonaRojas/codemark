use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::fs;

/// PackCache manages the storage and retrieval of SQLite pack files on disk.
pub struct PackCache {
    base_dir: PathBuf,
}

impl PackCache {
    /// Creates a new PackCache rooted at the specified data directory.
    pub fn new(data_dir: PathBuf) -> Self {
        let base_dir = data_dir.join("pack-cache").join("tours");
        Self { base_dir }
    }

    /// Ensures that all required directories exist on disk.
    pub async fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.base_dir)
            .await
            .context("Failed to create pack cache directories")?;
        Ok(())
    }

    /// Returns the expected path on disk for a given tour's pack file.
    pub fn get_pack_path(&self, tour_id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.sqlite", tour_id))
    }

    /// Moves a pack file from a temporary location into the cache.
    /// Handles cross-filesystem renames by falling back to copy-and-remove.
    pub async fn save_pack(&self, tour_id: &str, temp_path: &Path) -> Result<()> {
        self.ensure_dirs().await?;
        let dest_path = self.get_pack_path(tour_id);

        // Attempt to rename (fast if on same filesystem)
        if let Err(e) = fs::rename(temp_path, &dest_path).await {
            // Fallback for cross-filesystem move (e.g. /tmp to /data)
            tracing::debug!("Rename failed ({}), falling back to copy and remove", e);
            fs::copy(temp_path, &dest_path).await.context("Failed to copy pack to cache")?;
            fs::remove_file(temp_path)
                .await
                .context("Failed to remove temporary pack file after copy")?;
        }
        Ok(())
    }

    /// Removes a pack file from the cache.
    pub async fn delete_pack(&self, tour_id: &str) -> Result<()> {
        let path = self.get_pack_path(tour_id);
        if fs::try_exists(&path).await.unwrap_or(false) {
            fs::remove_file(path).await.context("Failed to delete pack from cache")?;
        }
        Ok(())
    }

    /// Returns true if a pack file for the given tour ID exists in the cache.
    pub async fn exists(&self, tour_id: &str) -> bool {
        fs::try_exists(self.get_pack_path(tour_id)).await.unwrap_or(false)
    }
}
