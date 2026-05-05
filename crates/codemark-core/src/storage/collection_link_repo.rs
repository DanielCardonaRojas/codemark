//! Collection link database operations.

use crate::engine::bookmark::CollectionLink;
use crate::error::Result;
use crate::storage::db::Database;
use std::str::FromStr;

impl Database {
    /// Insert a link for a collection.
    pub fn insert_collection_link(&self, link: &CollectionLink) -> Result<()> {
        self.conn().execute(
            "INSERT INTO collection_links (id, collection_id, kind, label, url, sort_order, added_at, added_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                link.id,
                link.collection_id,
                link.kind.to_string(),
                link.label,
                link.url,
                link.sort_order,
                link.added_at,
                link.added_by,
            ],
        )?;
        Ok(())
    }

    /// List all links for a specific collection.
    pub fn list_links_for_collection(&self, collection_id: &str) -> Result<Vec<CollectionLink>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, collection_id, kind, label, url, sort_order, added_at, added_by 
             FROM collection_links 
             WHERE collection_id = ?1 
             ORDER BY sort_order ASC, added_at ASC",
        )?;
        let rows = stmt.query_map([collection_id], |row| {
            let kind_str: String = row.get(2)?;
            Ok(CollectionLink {
                id: row.get(0)?,
                collection_id: row.get(1)?,
                kind: crate::engine::bookmark::CollectionLinkKind::from_str(&kind_str)
                    .unwrap_or(crate::engine::bookmark::CollectionLinkKind::Other),
                label: row.get(3)?,
                url: row.get(4)?,
                sort_order: row.get(5)?,
                added_at: row.get(6)?,
                added_by: row.get(7)?,
            })
        })?;
        let results: Vec<CollectionLink> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// Delete a link from a collection.
    pub fn delete_collection_link(&self, id: &str, collection_id: &str) -> Result<bool> {
        let count = self.conn().execute(
            "DELETE FROM collection_links WHERE id = ?1 AND collection_id = ?2",
            [id, collection_id],
        )?;
        Ok(count > 0)
    }

    /// Reorder links in a collection.
    pub fn reorder_collection_links(
        &self,
        collection_id: &str,
        ordered_ids: &[String],
    ) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        for (i, id) in ordered_ids.iter().enumerate() {
            tx.execute(
                "UPDATE collection_links SET sort_order = ?1 WHERE id = ?2 AND collection_id = ?3",
                rusqlite::params![i as i32, id, collection_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
