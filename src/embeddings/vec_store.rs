//! Vector storage for embeddings.
//!
//! This module provides storage for vector embeddings. The full implementation
//! will use sqlite-vec for efficient similarity search. For now, we provide
//! the abstraction layer.

use rusqlite::{Connection, Result as SqliteResult};

/// Entry in the vector store.
#[derive(Debug, Clone)]
pub struct VecStoreEntry {
    pub bookmark_id: String,
    pub embedding: Vec<f32>,
}

/// Vector store for embeddings.
///
/// TODO: Integrate sqlite-vec extension for efficient similarity search.
/// Currently provides a placeholder implementation.
pub struct VecStore {
    /// Whether the vec0 extension is loaded.
    extension_loaded: bool,
}

impl VecStore {
    /// Create a new VecStore.
    pub fn new() -> Self {
        VecStore {
            extension_loaded: false,
        }
    }

    /// Load the sqlite-vec extension.
    ///
    /// Returns true if successful, false otherwise.
    pub fn load_extension(&mut self, _conn: &mut Connection) -> SqliteResult<bool> {
        // TODO: Implement sqlite-vec loading
        // For now, return false to indicate extension not available
        Ok(false)
    }

    /// Create the bookmark_embeddings table.
    pub fn create_table(&self, conn: &mut Connection, dimensions: usize) -> SqliteResult<()> {
        // Fallback: create a regular table without vec0
        // This will be replaced with vec0 virtual table once extension is loaded
        conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS bookmark_embeddings (
                    bookmark_id TEXT PRIMARY KEY,
                    embedding BLOB NOT NULL,
                    dimensions INTEGER NOT NULL DEFAULT {dimensions}
                )"
            ),
            [],
        )?;
        Ok(())
    }

    /// Insert an embedding for a bookmark.
    pub fn insert(&self, conn: &mut Connection, entry: &VecStoreEntry) -> SqliteResult<()> {
        // Convert f32 vector to bytes
        let bytes: Vec<u8> = entry
            .embedding
            .iter()
            .flat_map(|&v| v.to_le_bytes())
            .collect();

        conn.execute(
            "INSERT OR REPLACE INTO bookmark_embeddings (bookmark_id, embedding, dimensions)
             VALUES (?1, ?2, ?3)",
            (
                &entry.bookmark_id,
                &bytes,
                entry.embedding.len() as i32,
            ),
        )?;
        Ok(())
    }

    /// Insert embeddings in batch.
    pub fn insert_batch(&self, conn: &mut Connection, entries: &[VecStoreEntry]) -> SqliteResult<()> {
        let mut tx = conn.unchecked_transaction()?;
        for entry in entries {
            self.insert(&mut tx, entry)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Get an embedding for a bookmark.
    pub fn get(&self, conn: &Connection, bookmark_id: &str) -> SqliteResult<Option<Vec<f32>>> {
        let mut stmt = conn.prepare(
            "SELECT embedding FROM bookmark_embeddings WHERE bookmark_id = ?1",
        )?;

        let result = stmt.query_row([bookmark_id], |row| {
            let blob: Vec<u8> = row.get(0)?;

            // Parse as f32 array
            let mut embedding = Vec::with_capacity(blob.len() / 4);
            for chunk in blob.chunks_exact(4) {
                let bytes: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
                embedding.push(f32::from_le_bytes(bytes));
            }
            Ok(embedding)
        });

        match result {
            Ok(e) => Ok(Some(e)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete an embedding.
    pub fn delete(&self, conn: &mut Connection, bookmark_id: &str) -> SqliteResult<()> {
        conn.execute(
            "DELETE FROM bookmark_embeddings WHERE bookmark_id = ?1",
            [bookmark_id],
        )?;
        Ok(())
    }

    /// Find similar bookmarks by vector similarity.
    ///
    /// TODO: Implement proper similarity search with sqlite-vec.
    /// Currently returns all bookmarks without ranking.
    pub fn search(
        &self,
        conn: &Connection,
        _query_embedding: &[f32],
        limit: usize,
    ) -> SqliteResult<Vec<String>> {
        let mut stmt = conn.prepare(
            &format!(
                "SELECT bookmark_id FROM bookmark_embeddings LIMIT {limit}"
            ),
        )?;

        let bookmark_ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(bookmark_ids)
    }

    /// Count embeddings in the store.
    pub fn count(&self, conn: &Connection) -> SqliteResult<usize> {
        conn.query_row(
            "SELECT COUNT(*) FROM bookmark_embeddings",
            [],
            |row| row.get(0),
        )
    }

    /// Find bookmarks without embeddings.
    pub fn find_without_embeddings(&self, conn: &Connection) -> SqliteResult<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT id FROM bookmarks
             WHERE id NOT IN (SELECT bookmark_id FROM bookmark_embeddings)",
        )?;

        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ids)
    }

    /// Check if the extension is loaded.
    pub fn is_extension_loaded(&self) -> bool {
        self.extension_loaded
    }
}

impl Default for VecStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec_store_creation() {
        let store = VecStore::new();
        assert!(!store.is_extension_loaded());
    }
}
