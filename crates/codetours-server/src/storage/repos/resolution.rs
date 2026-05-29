use anyhow::{Context, Result};
use codemark_core::engine::bookmark::{BookmarkHealth, Resolution};
use deadpool_sqlite::Pool;

pub struct ResolutionRepo {
    pool: Pool,
}

impl ResolutionRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn latest_by_bookmark(&self, bookmark_id: String) -> Result<Option<Resolution>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, bookmark_id, resolved_at, commit_hash, method,
                        match_count, file_path, byte_range, line_range, content_hash,
                        headline, snapshot, breadcrumbs, health, is_dirty
                 FROM resolutions 
                 WHERE bookmark_id = ?1
                 ORDER BY resolved_at DESC
                 LIMIT 1",
            )?;
            let rows = stmt.query_map([bookmark_id], row_to_resolution)?;
            let results: Vec<Resolution> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            match results.into_iter().next() {
                Some(res) => Ok::<_, rusqlite::Error>(Some(res)),
                None => Ok::<_, rusqlite::Error>(None),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn list_by_bookmark(&self, bookmark_id: String) -> Result<Vec<Resolution>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, bookmark_id, resolved_at, commit_hash, method,
                        match_count, file_path, byte_range, line_range, content_hash,
                        headline, snapshot, breadcrumbs, health, is_dirty
                 FROM resolutions 
                 WHERE bookmark_id = ?1
                 ORDER BY resolved_at DESC",
            )?;
            let rows = stmt.query_map([bookmark_id], row_to_resolution)?;
            let results: Vec<Resolution> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}

fn row_to_resolution(row: &rusqlite::Row) -> rusqlite::Result<Resolution> {
    let method_str: String = row.get(4)?;
    let method = method_str.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let health_str: String = row.get(13)?;
    let health: BookmarkHealth = health_str.parse().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(13, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Resolution {
        is_dirty: row.get(14)?,
        id: row.get(0)?,
        bookmark_id: row.get(1)?,
        resolved_at: row.get(2)?,
        health,
        commit_hash: row.get(3)?,
        method,
        match_count: row.get(5)?,
        file_path: row.get(6)?,
        byte_range: row.get(7)?,
        line_range: row.get(8)?,
        content_hash: row.get(9)?,
        headline: row.get(10)?,
        snapshot: row.get(11)?,
        breadcrumbs: row.get(12)?,
    })
}
