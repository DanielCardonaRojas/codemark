//! Vector storage for embeddings using sqlite-vec.
//!
//! This module provides storage for vector embeddings with efficient
//! similarity search using the sqlite-vec extension.

#![allow(dead_code)]

use crate::embeddings::config::DistanceMetric;
use rusqlite::{Connection, Result as SqliteResult};
use zerocopy::IntoBytes;

/// Which entity a vector store operates over.
///
/// Bookmarks and collections each get their own vec0 table so semantic search
/// can rank either kind independently. The variant drives the table name, the
/// id column, and the source table used to find rows lacking embeddings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingTarget {
    /// Bookmark embeddings (`bookmark_embeddings`).
    #[default]
    Bookmark,
    /// Collection embeddings (`collection_embeddings`).
    Collection,
}

impl EmbeddingTarget {
    /// Name of the vec0 virtual table holding the embeddings.
    pub fn table(&self) -> &'static str {
        match self {
            EmbeddingTarget::Bookmark => "bookmark_embeddings",
            EmbeddingTarget::Collection => "collection_embeddings",
        }
    }

    /// Primary-key column of the embeddings table.
    pub fn id_column(&self) -> &'static str {
        match self {
            EmbeddingTarget::Bookmark => "bookmark_id",
            EmbeddingTarget::Collection => "collection_id",
        }
    }

    /// Source table used to detect entities that don't have an embedding yet.
    pub fn source_table(&self) -> &'static str {
        match self {
            EmbeddingTarget::Bookmark => "bookmarks",
            EmbeddingTarget::Collection => "collections",
        }
    }
}

/// Entry in the vector store.
///
/// `id` holds the bookmark or collection id depending on the store's
/// [`EmbeddingTarget`].
#[derive(Debug, Clone)]
pub struct VecStoreEntry {
    pub id: String,
    pub embedding: Vec<f32>,
}

/// Result from a similarity search.
///
/// `id` holds the bookmark or collection id depending on the store's
/// [`EmbeddingTarget`].
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub distance: f64,
}

/// Vector store for embeddings using sqlite-vec.
///
/// Uses the sqlite-vec extension for efficient vector similarity search.
pub struct VecStore {
    /// Dimensions of the embedding vectors.
    dimensions: usize,
    /// Distance metric to use for similarity search.
    distance_metric: DistanceMetric,
    /// Which entity (bookmarks or collections) this store operates over.
    target: EmbeddingTarget,
}

impl VecStore {
    /// Initialize the sqlite-vec extension.
    ///
    /// This must be called once before creating any connections that will
    /// use vec0 virtual tables. Uses sqlite3_auto_extension to automatically
    /// load the extension for all future connections.
    pub fn load_extension() -> SqliteResult<()> {
        use rusqlite::ffi::sqlite3_auto_extension;
        use sqlite_vec::sqlite3_vec_init;
        use std::sync::Once;

        static INIT: Once = Once::new();
        INIT.call_once(|| unsafe {
            #[allow(clippy::missing_transmute_annotations)]
            sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
        });
        Ok(())
    }

    /// Ensure the extension is loaded.
    pub fn ensure_extension_loaded() {
        let _ = Self::load_extension();
    }

    /// Create a new VecStore over bookmarks with the default distance metric (L2).
    ///
    /// Panics if init_extension() was not called first.
    pub fn new(dimensions: usize) -> Self {
        VecStore {
            dimensions,
            distance_metric: DistanceMetric::default(),
            target: EmbeddingTarget::Bookmark,
        }
    }

    /// Create a new VecStore over bookmarks with a specific distance metric.
    ///
    /// Panics if init_extension() was not called first.
    pub fn with_metric(dimensions: usize, distance_metric: DistanceMetric) -> Self {
        VecStore { dimensions, distance_metric, target: EmbeddingTarget::Bookmark }
    }

    /// Create a new VecStore over a specific [`EmbeddingTarget`] (bookmarks or
    /// collections) with a specific distance metric.
    ///
    /// Panics if init_extension() was not called first.
    pub fn for_target(
        dimensions: usize,
        distance_metric: DistanceMetric,
        target: EmbeddingTarget,
    ) -> Self {
        VecStore { dimensions, distance_metric, target }
    }

    /// Create the embeddings table for this store's target as a vec0 virtual table.
    pub fn create_table(&self, conn: &mut Connection) -> SqliteResult<()> {
        let dim = self.dimensions;
        let table = self.target.table();
        let id_column = self.target.id_column();
        conn.execute(
            &format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING vec0(
                    {id_column} TEXT PRIMARY KEY,
                    embedding float[{dim}]
                )"
            ),
            [],
        )?;
        Ok(())
    }

    /// Insert an embedding for an entity.
    pub fn insert(&self, conn: &Connection, entry: &VecStoreEntry) -> SqliteResult<()> {
        let expected_dim = self.dimensions;
        let actual_dim = entry.embedding.len();

        if actual_dim != expected_dim {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Embedding dimension mismatch: expected {expected_dim}, got {actual_dim}"
            )));
        }

        let (table, id_column) = (self.target.table(), self.target.id_column());
        conn.execute(
            &format!(
                "INSERT OR IGNORE INTO {table} ({id_column}, embedding)
                 VALUES (?1, ?2)"
            ),
            (&entry.id, entry.embedding.as_bytes()),
        )?;
        Ok(())
    }

    /// Insert embeddings in batch.
    pub fn insert_batch(
        &self,
        conn: &mut Connection,
        entries: &[VecStoreEntry],
    ) -> SqliteResult<()> {
        let (table, id_column) = (self.target.table(), self.target.id_column());
        let tx = conn.unchecked_transaction()?;
        for entry in entries {
            // Delete existing embedding if any
            tx.execute(&format!("DELETE FROM {table} WHERE {id_column} = ?1"), [&entry.id])?;

            // Inline insert logic for transaction
            let expected_dim = self.dimensions;
            let actual_dim = entry.embedding.len();

            if actual_dim != expected_dim {
                return Err(rusqlite::Error::InvalidParameterName(format!(
                    "Embedding dimension mismatch: expected {expected_dim}, got {actual_dim}"
                )));
            }

            tx.execute(
                &format!(
                    "INSERT INTO {table} ({id_column}, embedding)
                      VALUES (?1, ?2)"
                ),
                (&entry.id, entry.embedding.as_bytes()),
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Get an embedding for an entity.
    ///
    /// Note: This retrieves the raw vector. For similarity search, use search().
    pub fn get(&self, conn: &Connection, id: &str) -> SqliteResult<Option<Vec<f32>>> {
        let (table, id_column) = (self.target.table(), self.target.id_column());
        let mut stmt =
            conn.prepare(&format!("SELECT embedding FROM {table} WHERE {id_column} = ?1"))?;

        // Get the blob and convert to f32
        let result = stmt.query_row([id], |row| {
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
    pub fn delete(&self, conn: &mut Connection, id: &str) -> SqliteResult<()> {
        let (table, id_column) = (self.target.table(), self.target.id_column());
        conn.execute(&format!("DELETE FROM {table} WHERE {id_column} = ?1"), [id])?;
        Ok(())
    }

    /// Find similar bookmarks by vector similarity.
    ///
    /// Returns bookmark IDs ordered by distance (closest first).
    /// Uses KNN search with the vec0 MATCH operator.
    ///
    /// Optionally filters results by threshold:
    /// - For L2/Cosine: only returns results with distance <= threshold
    /// - For InnerProduct: only returns results with distance >= threshold
    pub fn search(
        &self,
        conn: &Connection,
        query_embedding: &[f32],
        limit: usize,
    ) -> SqliteResult<Vec<SearchResult>> {
        self.search_with_threshold(conn, query_embedding, limit, None)
    }

    /// Find similar bookmarks with optional threshold filtering.
    ///
    /// The threshold behavior depends on the distance metric:
    /// - L2/Cosine: returns results where distance <= threshold
    /// - InnerProduct: returns results where distance >= threshold
    pub fn search_with_threshold(
        &self,
        conn: &Connection,
        query_embedding: &[f32],
        limit: usize,
        threshold: Option<f32>,
    ) -> SqliteResult<Vec<SearchResult>> {
        if query_embedding.len() != self.dimensions {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "Query embedding dimension mismatch: expected {}, got {}",
                self.dimensions,
                query_embedding.len()
            )));
        }

        // Fetch more results than needed when threshold is active,
        // since we'll filter some out
        let fetch_limit = if threshold.is_some() { limit * 10 } else { limit };

        let (table, id_column) = (self.target.table(), self.target.id_column());
        let sql = format!(
            "SELECT {id_column}, distance
             FROM {table}
             WHERE embedding MATCH ?1
             ORDER BY distance
             LIMIT {fetch_limit}"
        );

        let mut stmt = conn.prepare(&sql)?;

        let mut results: Vec<SearchResult> = stmt
            .query_map([query_embedding.as_bytes()], |row| {
                Ok(SearchResult { id: row.get(0)?, distance: row.get(1)? })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // sqlite-vec always returns Euclidean (L2) distance regardless of the
        // configured metric. Convert each distance so reported values and
        // threshold comparisons match `distance_metric`. The conversion is
        // monotonic in L2, so the ORDER BY ranking above is preserved.
        for r in &mut results {
            r.distance = self.distance_metric.from_l2_distance(r.distance);
        }

        // Apply threshold filter if specified
        if let Some(threshold) = threshold {
            if self.distance_metric.is_lower_better() {
                // L2 and Cosine: lower is better
                results.retain(|r| r.distance <= threshold as f64);
            } else {
                // InnerProduct: higher is better
                results.retain(|r| r.distance >= threshold as f64);
            }
        }

        // Truncate to requested limit
        results.truncate(limit);
        Ok(results)
    }

    /// Count embeddings in the store.
    pub fn count(&self, conn: &Connection) -> SqliteResult<usize> {
        let table = self.target.table();
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
    }

    /// Find entities without embeddings.
    pub fn find_without_embeddings(&self, conn: &Connection) -> SqliteResult<Vec<String>> {
        let (table, id_column, source_table) =
            (self.target.table(), self.target.id_column(), self.target.source_table());
        let mut stmt = conn.prepare(&format!(
            "SELECT id FROM {source_table}
             WHERE id NOT IN (SELECT {id_column} FROM {table})"
        ))?;

        let ids =
            stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;

        Ok(ids)
    }

    /// Returns the dimensionality of embeddings in this store.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Returns the distance metric being used.
    pub fn distance_metric(&self) -> DistanceMetric {
        self.distance_metric
    }

    /// Returns the entity target (bookmarks or collections) of this store.
    pub fn target(&self) -> EmbeddingTarget {
        self.target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec_store_creation() {
        let store = VecStore::new(384);
        assert_eq!(store.dimensions(), 384);
        assert_eq!(store.target(), EmbeddingTarget::Bookmark);
    }

    #[test]
    fn test_embedding_target_maps_tables_and_columns() {
        assert_eq!(EmbeddingTarget::Bookmark.table(), "bookmark_embeddings");
        assert_eq!(EmbeddingTarget::Bookmark.id_column(), "bookmark_id");
        assert_eq!(EmbeddingTarget::Bookmark.source_table(), "bookmarks");
        assert_eq!(EmbeddingTarget::Collection.table(), "collection_embeddings");
        assert_eq!(EmbeddingTarget::Collection.id_column(), "collection_id");
        assert_eq!(EmbeddingTarget::Collection.source_table(), "collections");
    }

    #[test]
    fn test_collection_store_uses_separate_table() {
        VecStore::load_extension().unwrap();

        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let bookmarks = VecStore::new(4);
        let collections =
            VecStore::for_target(4, DistanceMetric::default(), EmbeddingTarget::Collection);

        bookmarks.create_table(&mut conn).unwrap();
        collections.create_table(&mut conn).unwrap();

        collections
            .insert(&conn, &VecStoreEntry { id: "col-1".to_string(), embedding: vec![0.1; 4] })
            .unwrap();

        // The collection embedding lands only in the collection store.
        assert_eq!(collections.count(&conn).unwrap(), 1);
        assert_eq!(bookmarks.count(&conn).unwrap(), 0);

        let query = vec![0.1_f32; 4];
        let hits = collections.search(&conn, &query, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "col-1");
    }

    mod integration_tests {
        use super::super::*;
        use rusqlite::Connection;

        #[test]
        fn test_vec_store_integration() {
            // Initialize the extension once
            VecStore::load_extension().unwrap();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(4);

            // Create table
            store.create_table(&mut conn).unwrap();

            // Insert embeddings
            let entries = vec![
                VecStoreEntry { id: "bookmark1".to_string(), embedding: vec![0.1, 0.1, 0.1, 0.1] },
                VecStoreEntry { id: "bookmark2".to_string(), embedding: vec![0.2, 0.2, 0.2, 0.2] },
                VecStoreEntry { id: "bookmark3".to_string(), embedding: vec![0.9, 0.9, 0.9, 0.9] },
            ];

            store.insert_batch(&mut conn, &entries).unwrap();

            // Test count
            assert_eq!(store.count(&conn).unwrap(), 3);

            // Test search - query close to bookmark2
            let query = vec![0.3, 0.3, 0.3, 0.3];
            let results = store.search(&conn, &query, 3).unwrap();

            assert_eq!(results.len(), 3);
            // bookmark2 should be closest (smallest distance)
            assert_eq!(results[0].id, "bookmark2");
            // bookmark3 should be farthest
            assert_eq!(results[2].id, "bookmark3");

            // Verify distances are in ascending order
            assert!(results[0].distance < results[1].distance);
            assert!(results[1].distance < results[2].distance);
        }

        #[test]
        fn test_vec_store_get() {
            VecStore::load_extension().unwrap();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(3);

            store.create_table(&mut conn).unwrap();

            let entry = VecStoreEntry { id: "test".to_string(), embedding: vec![1.0, 2.0, 3.0] };

            store.insert(&conn, &entry).unwrap();

            let retrieved = store.get(&conn, "test").unwrap().unwrap();
            assert_eq!(retrieved, vec![1.0, 2.0, 3.0]);
        }

        #[test]
        fn test_vec_store_delete() {
            VecStore::load_extension().unwrap();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(2);

            store.create_table(&mut conn).unwrap();

            let entry = VecStoreEntry { id: "to_delete".to_string(), embedding: vec![1.0, 2.0] };

            store.insert(&conn, &entry).unwrap();
            assert_eq!(store.count(&conn).unwrap(), 1);

            store.delete(&mut conn, "to_delete").unwrap();
            assert_eq!(store.count(&conn).unwrap(), 0);
        }

        #[test]
        fn test_search_with_threshold() {
            VecStore::load_extension().unwrap();

            let mut conn = Connection::open_in_memory().unwrap();
            let store = VecStore::new(3);

            store.create_table(&mut conn).unwrap();

            // Insert embeddings
            let entries = vec![
                VecStoreEntry { id: "close".to_string(), embedding: vec![0.1, 0.1, 0.1] },
                VecStoreEntry { id: "medium".to_string(), embedding: vec![0.5, 0.5, 0.5] },
                VecStoreEntry { id: "far".to_string(), embedding: vec![1.0, 1.0, 1.0] },
            ];

            store.insert_batch(&mut conn, &entries).unwrap();

            // Query near "close"
            let query = vec![0.15, 0.15, 0.15];

            // Without threshold - returns all
            let all_results = store.search(&conn, &query, 10).unwrap();
            assert_eq!(all_results.len(), 3);

            // With threshold - only returns close matches
            let thresholded = store.search_with_threshold(&conn, &query, 10, Some(0.3)).unwrap();
            assert!(thresholded.len() < 3);
            // "close" should be included (distance ~0.086)
            assert!(thresholded.iter().any(|r| r.id == "close"));
        }

        #[test]
        fn test_distance_metric_default() {
            let store = VecStore::new(128);
            assert_eq!(store.distance_metric(), DistanceMetric::L2);
        }

        #[test]
        fn test_distance_metric_custom() {
            let store = VecStore::with_metric(128, DistanceMetric::Cosine);
            assert_eq!(store.distance_metric(), DistanceMetric::Cosine);
        }
    }
}
