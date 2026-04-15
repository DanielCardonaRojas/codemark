//! Semantic search operations using vector embeddings.

#![allow(dead_code)]

use std::path::PathBuf;

use rusqlite::Connection;

use crate::embeddings::config::{DistanceMetric, EmbeddingModel};
use crate::embeddings::{
    EmbeddingProvider, LocalEmbeddingProvider, SearchResult, VecStore, VecStoreEntry,
};
use crate::engine::bookmark::Bookmark;
use crate::error::Result;

/// Semantic search repository.
pub struct SemanticRepo {
    /// Path to the model cache directory.
    cache_dir: Option<PathBuf>,
    /// The embedding model to use.
    model: EmbeddingModel,
    /// Distance metric for similarity search.
    distance_metric: DistanceMetric,
    /// Optional threshold for filtering results.
    threshold: Option<f32>,
}

impl SemanticRepo {
    /// Create a new semantic search repository.
    pub fn new(cache_dir: Option<PathBuf>, model: EmbeddingModel) -> Self {
        Self { cache_dir, model, distance_metric: DistanceMetric::default(), threshold: None }
    }

    /// Create a new semantic search repository with custom distance metric and threshold.
    pub fn with_config(
        cache_dir: Option<PathBuf>,
        model: EmbeddingModel,
        distance_metric: DistanceMetric,
        threshold: Option<f32>,
    ) -> Self {
        Self { cache_dir, model, distance_metric, threshold }
    }

    /// Get or create the embedding provider.
    fn provider(&self) -> Result<LocalEmbeddingProvider> {
        LocalEmbeddingProvider::new(self.model.clone(), self.cache_dir.clone()).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to create embedding provider: {}", e))
        })
    }

    /// Generate an embedding for a bookmark's searchable text.
    pub fn embed_bookmark(&self, bookmark: &Bookmark) -> Result<Vec<f32>> {
        let text = self.prepare_bookmark_text(bookmark);
        let provider = self.provider()?;

        // Use tokio runtime for async embedding
        let rt = tokio::runtime::Runtime::new().map_err(|e| {
            crate::error::Error::Operation(format!("Failed to create runtime: {}", e))
        })?;

        let embedding = rt.block_on(provider.embed(&text)).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate embedding: {}", e))
        })?;

        Ok(embedding)
    }

    /// Prepare searchable text from a bookmark.
    fn prepare_bookmark_text(&self, bookmark: &Bookmark) -> String {
        use crate::embeddings::provider::prepare_embedding_text;

        // Get the first annotation's notes and context
        let notes = bookmark.annotations.first().and_then(|a| a.notes.as_deref());
        let context = bookmark.annotations.first().and_then(|a| a.context.as_deref());

        prepare_embedding_text(&bookmark.tags, notes, context)
    }

    /// Store embeddings for multiple bookmarks.
    pub fn store_embeddings(&self, conn: &mut Connection, bookmarks: &[Bookmark]) -> Result<usize> {
        if bookmarks.is_empty() {
            return Ok(0);
        }

        // Ensure sqlite-vec extension is loaded
        crate::embeddings::VecStore::ensure_extension_loaded();

        let provider = self.provider()?;
        let rt = tokio::runtime::Runtime::new().map_err(|e| {
            crate::error::Error::Operation(format!("Failed to create runtime: {}", e))
        })?;

        // Prepare texts for batch embedding
        let texts: Vec<String> = bookmarks.iter().map(|b| self.prepare_bookmark_text(b)).collect();

        // Generate embeddings in batch
        let embeddings = rt.block_on(provider.embed_batch(&texts)).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate embeddings: {}", e))
        })?;

        // Create vec_store entries and insert
        let store = VecStore::with_metric(provider.dimensions(), self.distance_metric);
        let entries: Vec<VecStoreEntry> = bookmarks
            .iter()
            .zip(embeddings)
            .map(|(bookmark, embedding)| VecStoreEntry {
                bookmark_id: bookmark.id.clone(),
                embedding,
            })
            .collect();

        store.insert_batch(conn, &entries).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to store embeddings: {}", e))
        })?;

        Ok(entries.len())
    }

    /// Search for similar bookmarks by semantic similarity.
    pub fn search(
        &self,
        conn: &Connection,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        self.search_with_threshold(conn, query, limit, self.threshold)
    }

    /// Search for similar bookmarks with optional threshold override.
    pub fn search_with_threshold(
        &self,
        conn: &Connection,
        query: &str,
        limit: usize,
        threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        // Ensure sqlite-vec extension is loaded
        crate::embeddings::VecStore::ensure_extension_loaded();

        let provider = self.provider()?;
        let rt = tokio::runtime::Runtime::new().map_err(|e| {
            crate::error::Error::Operation(format!("Failed to create runtime: {}", e))
        })?;

        let query_embedding = rt.block_on(provider.embed(query)).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate query embedding: {}", e))
        })?;

        let store = VecStore::with_metric(provider.dimensions(), self.distance_metric);
        let results =
            store.search_with_threshold(conn, &query_embedding, limit, threshold).map_err(|e| {
                crate::error::Error::Operation(format!("Semantic search failed: {}", e))
            })?;

        Ok(results)
    }

    /// Find bookmarks that don't have embeddings yet.
    pub fn find_without_embeddings(&self, conn: &Connection) -> Result<Vec<String>> {
        let store = VecStore::with_metric(self.model.dimensions(), self.distance_metric);
        store.find_without_embeddings(conn).map_err(|e| {
            crate::error::Error::Operation(format!(
                "Failed to find bookmarks without embeddings: {}",
                e
            ))
        })
    }

    /// Delete an embedding for a bookmark.
    pub fn delete_embedding(&self, conn: &mut Connection, bookmark_id: &str) -> Result<()> {
        let store = VecStore::with_metric(self.model.dimensions(), self.distance_metric);
        store.delete(conn, bookmark_id).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to delete embedding: {}", e))
        })
    }

    /// Count total embeddings in the store.
    pub fn count_embeddings(&self, conn: &Connection) -> Result<usize> {
        let store = VecStore::with_metric(self.model.dimensions(), self.distance_metric);
        store.count(conn).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to count embeddings: {}", e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prepare_bookmark_text() {
        let repo = SemanticRepo::new(None, EmbeddingModel::AllMiniLmL6V2);

        let bookmark = Bookmark {
            id: "test".to_string(),
            query: "function test() {}".to_string(),
            language: "rust".to_string(),
            file_path: "/test.rs".to_string(),
            content_hash: Some("hash".to_string()),
            commit_hash: None,
            status: crate::engine::bookmark::BookmarkStatus::Active,
            resolution_method: Some(crate::engine::bookmark::ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: chrono::Utc::now().to_string(),
            created_by: Some("user".to_string()),
            tags: vec!["tag1".to_string(), "tag2".to_string()],
            annotations: vec![crate::engine::bookmark::Annotation {
                id: "ann-1".to_string(),
                bookmark_id: "test-bm".to_string(),
                added_at: chrono::Utc::now().to_string(),
                added_by: None,
                notes: Some("A test function".to_string()),
                context: Some("Testing utilities".to_string()),
                source: None,
            }],
        };

        let text = repo.prepare_bookmark_text(&bookmark);
        assert!(text.contains("Tags: tag1, tag2"));
        assert!(text.contains("Note: A test function"));
        assert!(text.contains("Context: Testing utilities"));
    }
}
