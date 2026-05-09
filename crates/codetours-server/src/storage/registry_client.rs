//! Registry client for scanning the global registry and discovering repository databases.

use anyhow::{Context, Result};
use codemark_core::storage::registry::open_registry;
use deadpool_sqlite::{Config as PoolConfig, Pool, Runtime};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Client for interacting with the global registry database.
#[derive(Clone)]
pub struct RegistryClient {
    /// Path to the registry database
    registry_path: PathBuf,
    /// Cached list of known repositories
    known_repos: Arc<RwLock<Vec<KnownRepoEntry>>>,
}

/// Entry for a known repository with its database path.
#[derive(Debug, Clone)]
pub struct KnownRepoEntry {
    pub repo_root: PathBuf,
    pub db_path: PathBuf,
    pub repo_owner: String,
    pub repo_name: String,
    pub origin_url: Option<String>,
}

impl RegistryClient {
    /// Create a new registry client.
    ///
    /// If `registry_path` is `None`, uses the default global registry path.
    pub fn new(registry_path: Option<PathBuf>) -> Result<Self> {
        let registry_path = if let Some(path) = registry_path {
            path
        } else {
            codemark_core::storage::registry::registry_path()
                .context("Failed to determine registry path")?
        };

        Ok(Self {
            registry_path,
            known_repos: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Open the registry database connection.
    pub fn open_registry(&self) -> Result<Connection> {
        if !self.registry_path.exists() {
            return Err(anyhow::anyhow!("Registry database not found at {:?}", self.registry_path));
        }
        open_registry().context("Failed to open registry database")
    }

    /// Scan the registry and cache the list of known repositories.
    pub async fn refresh(&self) -> Result<()> {
        let conn = self.open_registry()?;
        let repos = codemark_core::storage::registry::list_repos(&conn)?;

        let entries: Vec<KnownRepoEntry> = repos
            .into_iter()
            .filter_map(|repo| {
                // Codetours (collections) are stored in the codemark database
                let db_path = repo.repo_root.join(".codemark/codemark.db");
                if db_path.exists() {
                    Some(KnownRepoEntry {
                        repo_root: repo.repo_root.clone(),
                        db_path,
                        repo_owner: repo.repo_owner,
                        repo_name: repo.repo_name,
                        origin_url: repo.origin_url,
                    })
                } else {
                    tracing::debug!("No codemark database found at {:?}", db_path);
                    None
                }
            })
            .collect();

        *self.known_repos.write().await = entries;
        tracing::info!("Refreshed registry with {} repositories", self.known_repos.read().await.len());

        Ok(())
    }

    /// Get the cached list of known repositories.
    pub async fn known_repos(&self) -> Vec<KnownRepoEntry> {
        self.known_repos.read().await.clone()
    }

    /// Get the registry path.
    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }
}

/// Create a connection pool for a repository database.
pub fn create_repo_pool(db_path: PathBuf) -> Result<Pool> {
    if !db_path.exists() {
        return Err(anyhow::anyhow!("Database not found at {:?}", db_path));
    }

    let pool_config = PoolConfig::new(db_path.clone());
    let pool = pool_config
        .builder(Runtime::Tokio1)?
        .max_size(2) // Each repo gets a small pool
        .post_create(deadpool_sqlite::Hook::AsyncFn(Box::new(|conn, _| {
            Box::pin(async move {
                conn.interact(|conn| {
                    conn.execute_batch(
                        "PRAGMA journal_mode=WAL;
                         PRAGMA synchronous=NORMAL;
                         PRAGMA foreign_keys=ON;",
                    )
                })
                .await
                .map_err(|e| deadpool_sqlite::HookError::message(e.to_string()))?
                .map_err(|e| deadpool_sqlite::HookError::message(e.to_string()))?;
                Ok(())
            })
        })))
        .build()
        .context("Failed to build connection pool")?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_registry_client_creation() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path().join("registry.db");

        let client = RegistryClient::new(Some(registry_path)).unwrap();
        assert_eq!(client.registry_path(), temp_dir.path().join("registry.db"));
    }

    #[tokio::test]
    async fn test_known_repos_empty() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path().join("registry.db");

        let client = RegistryClient::new(Some(registry_path)).unwrap();
        let repos = client.known_repos().await;
        assert!(repos.is_empty());
    }
}
