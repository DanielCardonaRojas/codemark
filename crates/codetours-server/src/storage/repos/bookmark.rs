use anyhow::{Context, Result};
use codemark_core::engine::bookmark::Bookmark;
use deadpool_sqlite::Pool;

pub struct BookmarkRepo {
    pool: Pool,
}

impl BookmarkRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get_by_id(&self, id: String) -> Result<Option<Bookmark>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, query, language, file_path, content_hash, commit_hash,
                        health, resolution_method, last_resolved_at, stale_since,
                        created_at, created_by
                 FROM bookmarks WHERE id = ?1",
            )?;
            let mut rows = stmt.query_map([id], row_to_bookmark)?;
            match rows.next() {
                Some(row) => Ok::<_, rusqlite::Error>(Some(row?)),
                None => Ok::<_, rusqlite::Error>(None),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn list_by_collection(&self, collection_id: String) -> Result<Vec<Bookmark>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT b.id, b.query, b.language, b.file_path, b.content_hash, b.commit_hash,
                        b.health, b.resolution_method, b.last_resolved_at, b.stale_since,
                        b.created_at, b.created_by
                 FROM bookmarks b
                 JOIN collection_bookmarks cb ON b.id = cb.bookmark_id

                 WHERE cb.collection_id = ?1
                 ORDER BY cb.position ASC",
            )?;
            let rows = stmt.query_map([collection_id], row_to_bookmark)?;
            let results: Vec<Bookmark> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn delete_orphans(&self) -> Result<usize> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let n = conn.execute(
                "DELETE FROM bookmarks WHERE id NOT IN (SELECT bookmark_id FROM collection_bookmarks)",
                [],
            )?;
            Ok::<_, rusqlite::Error>(n)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}

fn row_to_bookmark(row: &rusqlite::Row) -> rusqlite::Result<Bookmark> {
    let health_str: String = row.get(6)?;
    let method_str: Option<String> = row.get(7)?;

    let health = health_str.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let resolution_method = match method_str {
        Some(s) => Some(s.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };

    Ok(Bookmark {
        id: row.get(0)?,
        query: row.get(1)?,
        language: row.get(2)?,
        file_path: row.get(3)?,
        content_hash: row.get(4)?,
        commit_hash: row.get(5)?,
        health,
        resolution_method,
        last_resolved_at: row.get(8)?,
        stale_since: row.get(9)?,
        created_at: row.get(10)?,
        created_by: row.get(11)?,
        tags: vec![],
        annotations: vec![],
        comments: vec![],
    })
}
