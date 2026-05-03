use std::path::{Path, PathBuf};
use anyhow::Result;

pub struct PackCache {
    base_dir: PathBuf,
}

impl PackCache {
    pub fn new(data_dir: PathBuf) -> Self {
        let base_dir = data_dir.join("pack-cache").join("tours");
        Self { base_dir }
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(())
    }

    pub fn get_pack_path(&self, tour_id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.sqlite", tour_id))
    }

    pub async fn save_pack(&self, tour_id: &str, temp_path: &Path) -> Result<()> {
        self.ensure_dirs()?;
        let dest_path = self.get_pack_path(tour_id);
        std::fs::rename(temp_path, dest_path)?;
        Ok(())
    }

    pub async fn delete_pack(&self, tour_id: &str) -> Result<()> {
        let path = self.get_pack_path(tour_id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn exists(&self, tour_id: &str) -> bool {
        self.get_pack_path(tour_id).exists()
    }
}
