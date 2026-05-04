use crate::config::StorageConfig;
use anyhow::{Context, Result};
use codemark_core::storage::db::Database;
use deadpool_sqlite::{Config as PoolConfig, Pool, Runtime};
use rusqlite::Connection;
use std::path::PathBuf;

pub struct StorageManager {
    pool: Pool,
}

impl StorageManager {
    pub fn new(data_dir: PathBuf, config: StorageConfig) -> Result<Self> {
        let tenant_dir = data_dir.join("tenants").join("default");
        std::fs::create_dir_all(&tenant_dir)
            .with_context(|| format!("Failed to create tenant directory: {:?}", tenant_dir))?;

        let db_path = tenant_dir.join("codetours.sqlite");

        // 1. Run migrations eagerly on construct
        codemark_core::embeddings::VecStore::load_extension()
            .with_context(|| "Failed to load sqlite-vec extension")?;

        let mut conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open database at {:?}", db_path))?;

        Database::run_migrations_on(&mut conn)
            .with_context(|| "Failed to run migrations using shared core logic")?;

        // 2. Build connection pool
        let pool_config = PoolConfig::new(db_path.clone());
        let pool = pool_config
            .builder(Runtime::Tokio1)?
            .max_size(config.pool_size as usize)
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

        Ok(Self { pool })
    }

    pub async fn get_conn(&self) -> Result<deadpool_sqlite::Object> {
        self.pool.get().await.context("Failed to get connection from pool")
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_storage_manager_init() -> Result<()> {
        let dir = tempdir()?;
        let config = StorageConfig { pool_size: 2 };

        let manager = StorageManager::new(dir.path().to_path_buf(), config)?;

        // Verify file exists
        let db_path = dir.path().join("tenants/default/codetours.sqlite");
        assert!(db_path.exists());

        // Verify schema version
        let conn = manager.get_conn().await?;
        let version: i64 = conn
            .interact(|conn| {
                conn.query_row(
                    "SELECT value FROM schema_meta WHERE key = 'schema_version'",
                    [],
                    |row| {
                        let v: String = row.get(0)?;
                        Ok(v.parse::<i64>().unwrap())
                    },
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("Interact error: {}", e))??;

        assert_eq!(version, 14);
        Ok(())
    }
}
