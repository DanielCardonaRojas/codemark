use deadpool_sqlite::Pool;
use codemark_core::engine::bookmark::BookmarkComment;
use anyhow::{Context, Result};

pub struct CommentRepo {
    pool: Pool,
}

impl CommentRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn list_by_bookmark(&self, bookmark_id: String) -> Result<Vec<BookmarkComment>> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, bookmark_id, author, body, created_at, parent_id
                 FROM bookmark_comments 
                 WHERE bookmark_id = ?1
                 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map([bookmark_id], |row| {
                Ok(BookmarkComment {
                    id: row.get(0)?,
                    bookmark_id: row.get(1)?,
                    author: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
                    parent_id: row.get(5)?,
                })
            })?;
            let results: Vec<BookmarkComment> = rows.filter_map(|r| r.ok()).collect();
            Ok::<_, rusqlite::Error>(results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn insert(&self, comment: BookmarkComment) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute(
                "INSERT INTO bookmark_comments (id, bookmark_id, author, body, created_at, parent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    comment.id,
                    comment.bookmark_id,
                    comment.author,
                    comment.body,
                    comment.created_at,
                    comment.parent_id,
                ],
            )?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }

    pub async fn delete_by_bookmark(&self, bookmark_id: String) -> Result<()> {
        let conn = self.pool.get().await?;
        conn.interact(move |conn| {
            conn.execute("DELETE FROM bookmark_comments WHERE bookmark_id = ?1", [bookmark_id])?;
            Ok::<_, rusqlite::Error>(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("Interaction error: {}", e))?
        .context("Database error")
    }
}
