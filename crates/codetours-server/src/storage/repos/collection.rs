use anyhow::{Context, Result};
use codemark_core::engine::bookmark::Collection;
use deadpool_sqlite::Pool;

pub struct CollectionRepo {
    pool: Pool,
}

impl CollectionRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: String) -> Result<Option<Collection>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, visibility, created_at, created_by, created_branch, 
                        published_at, published_commit_sha, repo_url, status, updated_at, imported_from_url
                 FROM collections WHERE id = ?1",
            )?;
            let rows = stmt.query_map([id], row_to_collection)?;
            let results: Vec<Collection> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            match results.into_iter().next() {
                Some(collection) => Ok::<_, rusqlite::Error>(Some(collection)),
                None => Ok::<_, rusqlite::Error>(None),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn get_by_name(&self, name: String) -> Result<Vec<Collection>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, visibility, created_at, created_by, created_branch, 
                        published_at, published_commit_sha, repo_url, status, updated_at, imported_from_url
                 FROM collections WHERE name = ?1",
            )?;
            let rows = stmt.query_map([name], row_to_collection)?;
            let results: Vec<Collection> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn list_published(&self) -> Result<Vec<Collection>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, visibility, created_at, created_by, created_branch, 
                        published_at, published_commit_sha, repo_url, status, updated_at, imported_from_url
                 FROM collections 
                 WHERE visibility IS NOT NULL AND visibility != 'private'
                   AND published_at IS NOT NULL
                   AND status = 'ready'
                 ORDER BY published_at DESC",
            )?;
            let rows = stmt.query_map([], row_to_collection)?;
            let results: Vec<Collection> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn delete_by_id(&self, id: String) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute("DELETE FROM collections WHERE id = ?1", [id])?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn update_status(&self, id: String, status: String) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute(
                "UPDATE collections SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                [status, id],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}

fn row_to_collection(row: &rusqlite::Row) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        visibility: row.get::<_, String>(3)?.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: row.get(4)?,
        created_by: row.get(5)?,
        created_branch: row.get(6)?,
        published_at: row.get(7)?,
        published_commit_sha: row.get(8)?,
        repo_url: row.get(9)?,
        status: row.get(10)?,
        updated_at: row.get(11)?,
        imported_from_url: row.get(12)?,
    })
}
