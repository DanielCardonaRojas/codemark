//! Collection tag database operations.

use crate::engine::bookmark::CollectionTag;
use crate::error::Result;
use crate::storage::db::Database;

impl Database {
    /// Insert a tag for a collection.
    pub fn insert_collection_tag(&self, tag: &CollectionTag) -> Result<()> {
        self.conn().execute(
            "INSERT OR IGNORE INTO collection_tags (collection_id, tag, added_at, added_by)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![tag.collection_id, tag.tag, tag.added_at, tag.added_by],
        )?;
        Ok(())
    }

    /// Insert multiple tags for a collection.
    pub fn insert_collection_tags(&self, tags: &[CollectionTag]) -> Result<()> {
        let tx = self.conn().unchecked_transaction()?;
        for tag in tags {
            tx.execute(
                "INSERT OR IGNORE INTO collection_tags (collection_id, tag, added_at, added_by)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![tag.collection_id, tag.tag, tag.added_at, tag.added_by],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// List all tags for a specific collection.
    pub fn list_tags_for_collection(&self, collection_id: &str) -> Result<Vec<CollectionTag>> {
        let mut stmt = self.conn().prepare(
            "SELECT collection_id, tag, added_at, added_by 
             FROM collection_tags 
             WHERE collection_id = ?1 
             ORDER BY added_at ASC",
        )?;
        let rows = stmt.query_map([collection_id], |row| {
            Ok(CollectionTag {
                collection_id: row.get(0)?,
                tag: row.get(1)?,
                added_at: row.get(2)?,
                added_by: row.get(3)?,
            })
        })?;
        let results: Vec<CollectionTag> = rows.filter_map(|r| r.ok()).collect();
        Ok(results)
    }

    /// Delete a tag from a collection.
    pub fn delete_collection_tag(&self, collection_id: &str, tag: &str) -> Result<bool> {
        let count = self.conn().execute(
            "DELETE FROM collection_tags WHERE collection_id = ?1 AND tag = ?2",
            [collection_id, tag],
        )?;
        Ok(count > 0)
    }

    /// Delete all tags for a collection.
    pub fn delete_all_tags_for_collection(&self, collection_id: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM collection_tags WHERE collection_id = ?1", [collection_id])?;
        Ok(())
    }

    /// List all unique tags used in collections.
    pub fn list_collection_tags(&self) -> Result<Vec<String>> {
        let mut stmt =
            self.conn().prepare("SELECT DISTINCT tag FROM collection_tags ORDER BY tag")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tags = Vec::new();
        for tag in rows {
            tags.push(tag?);
        }
        Ok(tags)
    }

    /// List all unique tags across bookmarks and collections.
    pub fn list_all_tags(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT tag FROM bookmark_tags
             UNION
             SELECT DISTINCT tag FROM collection_tags
             ORDER BY tag",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut tags = Vec::new();
        for tag in rows {
            tags.push(tag?);
        }
        Ok(tags)
    }
}
