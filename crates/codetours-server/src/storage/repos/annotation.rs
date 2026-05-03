use anyhow::{Context, Result};
use codemark_core::engine::bookmark::Annotation;
use deadpool_sqlite::Pool;

pub struct AnnotationRepo {
    pool: Pool,
}

impl AnnotationRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn list_by_bookmark(&self, bookmark_id: String) -> Result<Vec<Annotation>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, bookmark_id, added_at, added_by, notes, context, source
                 FROM bookmark_annotations 
                 WHERE bookmark_id = ?1
                 ORDER BY added_at ASC",
            )?;
            let rows = stmt.query_map([bookmark_id], |row| {
                Ok(Annotation {
                    id: row.get(0)?,
                    bookmark_id: row.get(1)?,
                    added_at: row.get(2)?,
                    added_by: row.get(3)?,
                    notes: row.get(4)?,
                    context: row.get(5)?,
                    source: row.get(6)?,
                })
            })?;
            let results: Vec<Annotation> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn delete_by_bookmark(&self, bookmark_id: String) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute("DELETE FROM bookmark_annotations WHERE bookmark_id = ?1", [bookmark_id])?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}
