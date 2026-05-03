use deadpool_sqlite::Pool;
use codemark_core::engine::bookmark::Tag;
use anyhow::{Context, Result};

pub struct TagRepo {
    pool: Pool,
}

impl TagRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn list_by_bookmark(&self, bookmark_id: String) -> Result<Vec<Tag>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT bookmark_id, tag, added_at, added_by
                 FROM bookmark_tags 
                 WHERE bookmark_id = ?1
                 ORDER BY tag ASC",
            )?;
            let rows = stmt.query_map([bookmark_id], |row| {
                Ok(Tag {
                    bookmark_id: row.get(0)?,
                    tag: row.get(1)?,
                    added_at: row.get(2)?,
                    added_by: row.get(3)?,
                })
            })?;
            let results: Vec<Tag> = rows.filter_map(|r| r.ok()).collect();
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn delete_by_bookmark(&self, bookmark_id: String) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute("DELETE FROM bookmark_tags WHERE bookmark_id = ?1", [bookmark_id])?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}
