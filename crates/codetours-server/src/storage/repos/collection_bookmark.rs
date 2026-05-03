use anyhow::{Context, Result};
use deadpool_sqlite::Pool;

#[derive(Debug, Clone)]
pub struct CollectionBookmark {
    pub collection_id: String,
    pub bookmark_id: String,
    pub added_at: String,
    pub position: i64,
}

pub struct CollectionBookmarkRepo {
    pool: Pool,
}

impl CollectionBookmarkRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn list_by_collection(
        &self,
        collection_id: String,
    ) -> Result<Vec<CollectionBookmark>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT collection_id, bookmark_id, added_at, position
                 FROM collection_bookmarks 
                 WHERE collection_id = ?1
                 ORDER BY position ASC",
            )?;
            let rows = stmt.query_map([collection_id], |row| {
                Ok(CollectionBookmark {
                    collection_id: row.get(0)?,
                    bookmark_id: row.get(1)?,
                    added_at: row.get(2)?,
                    position: row.get(3)?,
                })
            })?;
            let results: Vec<CollectionBookmark> = rows.filter_map(|r| r.ok()).collect();
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn delete_by_collection(&self, collection_id: String) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute(
                "DELETE FROM collection_bookmarks WHERE collection_id = ?1",
                [collection_id],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}
