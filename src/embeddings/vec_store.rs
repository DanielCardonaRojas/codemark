//! Vector storage for embeddings using sqlite-vec.
//!
//! This module provides storage for vector embeddings with efficient
//! similarity search using the sqlite-vec extension.

use rusqlite::{Connection, Result as SqliteResult};
use zerocopy::IntoBytes;

/// Entry in the vector store.
#[derive(Debug, Clone)]
pub struct VecStoreEntry {
    pub bookmark_id: String,
    pub embedding: Vec<f32>,
}

/// Result from a similarity search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub bookmark_id: String,
    pub distance: f64,
}

/// Vector store for embeddings using sqlite-vec.
///
/// Uses the sqlite-vec extension for efficient vector similarity search.
pub struct VecStore {
    /// Dimensions of the embedding vectors.
    dimensions: usize,
}

impl VecStore {
    /// Initialize the sqlite-vec extension.
    ///
    /// This must be called once before creating any connections that will
    /// use vec0 virtual tables. Uses sqlite3_auto_extension to automatically
    /// load the extension for all future connections.
    ///
    /// This is safe to call multiple times - subsequent calls will be no-ops.
    pub fn init_extension() {
        use std::sync::Once;
        use rusqlite::ffi::sqlite3_auto_extension;
        use sqlite_vec::sqlite3_vec_init;

        static INIT: Once = Once::new();
        INIT.call_once(|| unsafe {
            sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite3_vec_init as *const (),
            )));
        });
    }

    /// Ensure the extension is loaded. Call this before any vec0 operations.
    pub fn ensure_extension_loaded() {
        Self::init_extension();
    }

    /// Create a new VecStore.
    ///
    /// Panics if init_extension() was not called first.
    pub fn new(dimensions: usize) -> Self {
        VecStore { dimensions }
    }

    /// Create the bookmark_embeddings table as a vec0 virtual table.
    pub fn create_table(&self, conn: &mut Connection) -> SqliteResult<()> {
        let dim = self.dimensions;
        conn.execute(
            &format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS bookmark_embeddings USING vec0(
                    bookmark_id TEXT PRIMARY KEY,
                    embedding float[{dim}]
                )"
            ),
            [],
        )?;
        Ok(())
    }

    /// Insert an embedding for a bookmark.
    pub fn insert(&self, conn: &Connection, entry: &VecStoreEntry) -> SqliteResult<()> {
        let expected_dim = self.dimensions;
        let actual_dim = entry.embedding.len();

        if actual_dim != expected_dim {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Embedding dimension mismatch: expected {expected_dim}, got {actual_dim}"
            )));
        }

        conn.execute(
            "INSERT OR REPLACE INTO bookmark_embeddings (bookmark_id, embedding)
             VALUES (?1, ?2)",
            (&entry.bookmark_id, entry.embedding.as_bytes()),
        )?;
        Ok(())
    }

    /// Insert embeddings in batch.
    pub fn insert_batch(&self, conn: &mut Connection, entries: &[VecStoreEntry]) -> SqliteResult<()> {
        let tx = conn.unchecked_transaction()?;
        for entry in entries {
            // Inline insert logic for transaction
            let expected_dim = self.dimensions;
            let actual_dim = entry.embedding.len();

            if actual_dim != expected_dim {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "Embedding dimension mismatch: expected {expected_dim}, got {actual_dim}"
                )));
            }

            tx.execute(
                "INSERT OR REPLACE INTO bookmark_embeddings (bookmark_id, embedding)
                 VALUES (?1, ?2)",
                (&entry.bookmark_id, entry.embedding.as_bytes()),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Get an embedding for a bookmark.
    ///
    /// Note: This retrieves the raw vector. For similarity search, use search().
    pub fn get(&self, conn: &Connection, bookmark_id: &str) -> SqliteResult<Option<Vec<f32>>> {
        let mut stmt = conn.prepare(
            "SELECT embedding FROM bookmark_embeddings WHERE bookmark_id = ?1",
        )?;

        // Get the blob and convert to f32
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
    /// Returns bookmark IDs ordered by distance (closest first).
    /// Uses KNN search with the vec0 MATCH operator.
    pub fn search(
        &self,
        conn: &Connection,
        query_embedding: &[f32],
        limit: usize,
    ) -> SqliteResult<Vec<SearchResult>> {
        if query_embedding.len() != self.dimensions {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Query embedding dimension mismatch: expected {}, got {}",
                self.dimensions,
                query_embedding.len()
            )));
        }

        let sql = format!(
            "SELECT bookmark_id, distance
             FROM bookmark_embeddings
             WHERE embedding MATCH ?1
             ORDER BY distance
             LIMIT {limit}"
        );

        let mut stmt = conn.prepare(&sql)?;

        let results = stmt
            .query_map([query_embedding.as_bytes()], |row| {
                Ok(SearchResult {
                    bookmark_id: row.get(0)?,
                    distance: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(results)
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

    /// Returns the dimensionality of embeddings in this store.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec_store_creation() {
        let store = VecStore::new(384);
        assert_eq!(store.dimensions(), 384);
    }

    mod integration_tests {
        use super::super::*;
        use rusqlite::Connection;

        #[test]
        fn test_vec_store_integration() {
            // Initialize the extension once
            VecStore::init_extension();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(4);

            // Create table
            store.create_table(&mut conn).unwrap();

            // Insert embeddings
            let entries = vec![
                VecStoreEntry {
                    bookmark_id: "bookmark1".to_string(),
                    embedding: vec![0.1, 0.1, 0.1, 0.1],
                },
                VecStoreEntry {
                    bookmark_id: "bookmark2".to_string(),
                    embedding: vec![0.2, 0.2, 0.2, 0.2],
                },
                VecStoreEntry {
                    bookmark_id: "bookmark3".to_string(),
                    embedding: vec![0.9, 0.9, 0.9, 0.9],
                },
            ];

            store.insert_batch(&mut conn, &entries).unwrap();

            // Test count
            assert_eq!(store.count(&conn).unwrap(), 3);

            // Test search - query close to bookmark2
            let query = vec![0.3, 0.3, 0.3, 0.3];
            let results = store.search(&conn, &query, 3).unwrap();

            assert_eq!(results.len(), 3);
            // bookmark2 should be closest (smallest distance)
            assert_eq!(results[0].bookmark_id, "bookmark2");
            // bookmark3 should be farthest
            assert_eq!(results[2].bookmark_id, "bookmark3");

            // Verify distances are in ascending order
            assert!(results[0].distance < results[1].distance);
            assert!(results[1].distance < results[2].distance);
        }

        #[test]
        fn test_vec_store_get() {
            VecStore::init_extension();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(3);

            store.create_table(&mut conn).unwrap();

            let entry = VecStoreEntry {
                bookmark_id: "test".to_string(),
                embedding: vec![1.0, 2.0, 3.0],
            };

            store.insert(&conn, &entry).unwrap();

            let retrieved = store.get(&conn, "test").unwrap().unwrap();
            assert_eq!(retrieved, vec![1.0, 2.0, 3.0]);
        }

        #[test]
        fn test_vec_store_delete() {
            VecStore::init_extension();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(2);

            store.create_table(&mut conn).unwrap();

            let entry = VecStoreEntry {
                bookmark_id: "to_delete".to_string(),
                embedding: vec![1.0, 2.0],
            };

            store.insert(&conn, &entry).unwrap();
            assert_eq!(store.count(&conn).unwrap(), 1);

            store.delete(&mut conn, "to_delete").unwrap();
            assert_eq!(store.count(&conn).unwrap(), 0);
        }
    }
}
