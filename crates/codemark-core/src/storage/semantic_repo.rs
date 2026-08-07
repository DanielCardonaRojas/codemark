//! Semantic search operations using vector embeddings.

#![allow(dead_code)]

use std::path::PathBuf;

use rusqlite::Connection;

use crate::embeddings::config::{DistanceMetric, EmbeddingModel};
use crate::embeddings::vec_store::EmbeddingTarget;
use crate::embeddings::{
    EmbeddingProvider, LocalEmbeddingProvider, SearchResult, VecStore, VecStoreEntry,
};
use crate::engine::bookmark::{Bookmark, Collection};
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
    pub async fn embed_bookmark(&self, bookmark: &Bookmark) -> Result<Vec<f32>> {
        let text = self.prepare_bookmark_text(bookmark);
        let provider = self.provider()?;

        let embedding = provider.embed(&text).await.map_err(|e| {
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

        let mut text = prepare_embedding_text(&bookmark.tags, notes, context);

        // Enrich with the node type and target derived from the tree-sitter query so
        // semantic search can match on structural intent (e.g. "function", "AuthService").
        if let Some(summary) = self.summarize_bookmark_query(bookmark) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("Node Type: {}", summary.label));
            if let Some(identifier) = &summary.identifier {
                text.push_str(&format!("\nNode Target: {identifier}"));
            }
        }

        text
    }

    /// Summarize a bookmark's tree-sitter query, mapping its language string to the
    /// `Language` enum. Returns `None` if the language is unknown or the query can't
    /// be summarized.
    fn summarize_bookmark_query(
        &self,
        bookmark: &Bookmark,
    ) -> Option<crate::query::summarizer::QuerySummary> {
        let language = bookmark.language.parse::<crate::parser::languages::Language>().ok()?;
        crate::query::summarizer::summarize_query(&bookmark.query, Some(language)).ok()
    }

    /// Store embeddings for multiple bookmarks.
    pub async fn store_embeddings(
        &self,
        conn: &mut Connection,
        bookmarks: &[Bookmark],
    ) -> Result<usize> {
        if bookmarks.is_empty() {
            return Ok(0);
        }

        // Ensure sqlite-vec extension is loaded
        crate::embeddings::VecStore::ensure_extension_loaded();

        let provider = self.provider()?;

        // Prepare texts for batch embedding
        let texts: Vec<String> = bookmarks.iter().map(|b| self.prepare_bookmark_text(b)).collect();

        // Generate embeddings in batch
        let embeddings = provider.embed_batch(&texts).await.map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate embeddings: {}", e))
        })?;

        // Create vec_store entries and insert
        let store = VecStore::with_metric(provider.dimensions(), self.distance_metric);
        let entries: Vec<VecStoreEntry> = bookmarks
            .iter()
            .zip(embeddings)
            .map(|(bookmark, embedding)| VecStoreEntry { id: bookmark.id.clone(), embedding })
            .collect();

        store.insert_batch(conn, &entries).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to store embeddings: {}", e))
        })?;

        Ok(entries.len())
    }

    /// Embed a query string once (loads the model). Reuse the result across DBs.
    ///
    /// This is the "embed-once" half of the split: callers that want to search
    /// many databases with the same query can call this once and then feed the
    /// resulting embedding into [`search_prepared`](Self::search_prepared) /
    /// [`search_collections_prepared`](Self::search_collections_prepared)
    /// per-connection without reloading the embedding model each time.
    pub async fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        // Ensure sqlite-vec extension is loaded
        crate::embeddings::VecStore::ensure_extension_loaded();

        let provider = self.provider()?;

        provider.embed(query).await.map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate query embedding: {}", e))
        })
    }

    /// Search a single connection with a pre-computed query embedding (no model load).
    ///
    /// The store is constructed from `query_embedding.len()`, which equals the
    /// provider's dimensions since the embedding was produced by that provider,
    /// so this is behaviorally identical to embedding the query inline.
    pub fn search_prepared(
        &self,
        conn: &Connection,
        query_embedding: &[f32],
        limit: usize,
        threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        // Ensure sqlite-vec extension is loaded
        crate::embeddings::VecStore::ensure_extension_loaded();

        let store = VecStore::with_metric(query_embedding.len(), self.distance_metric);
        store
            .search_with_threshold(conn, query_embedding, limit, threshold)
            .map_err(|e| crate::error::Error::Operation(format!("Semantic search failed: {}", e)))
    }

    /// Collection-target variant of [`search_prepared`](Self::search_prepared).
    pub fn search_collections_prepared(
        &self,
        conn: &Connection,
        query_embedding: &[f32],
        limit: usize,
        threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        // Ensure sqlite-vec extension is loaded
        crate::embeddings::VecStore::ensure_extension_loaded();

        let store = self.collection_store(query_embedding.len());
        store
            .search_with_threshold(conn, query_embedding, limit, threshold)
            .map_err(|e| crate::error::Error::Operation(format!("Semantic search failed: {}", e)))
    }

    /// Search for similar bookmarks by semantic similarity.
    pub async fn search(
        &self,
        conn: &Connection,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        self.search_with_threshold(conn, query, limit, self.threshold).await
    }

    /// Search for similar bookmarks with optional threshold override.
    pub async fn search_with_threshold(
        &self,
        conn: &Connection,
        query: &str,
        limit: usize,
        threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        let query_embedding = self.embed_query(query).await?;
        self.search_prepared(conn, &query_embedding, limit, threshold)
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

    // ── Collections ──────────────────────────────────────────────────────

    /// Build a [`VecStore`] targeting the collection embeddings table.
    fn collection_store(&self, dimensions: usize) -> VecStore {
        VecStore::for_target(dimensions, self.distance_metric, EmbeddingTarget::Collection)
    }

    /// Prepare searchable text from a collection and its tags.
    ///
    /// Combines the human-facing fields (name, description) with tags so
    /// semantic search can match on either the collection's purpose or its
    /// labels.
    fn prepare_collection_text(&self, collection: &Collection, tags: &[String]) -> String {
        let mut parts = Vec::new();

        parts.push(format!("Name: {}", collection.name));

        if let Some(description) = &collection.description
            && !description.is_empty()
        {
            parts.push(format!("Description: {description}"));
        }

        if !tags.is_empty() {
            parts.push(format!("Tags: {}", tags.join(", ")));
        }

        parts.join("\n")
    }

    /// Load a collection's tags from the database.
    fn collection_tags(conn: &Connection, collection_id: &str) -> Result<Vec<String>> {
        let mut stmt = conn
            .prepare("SELECT tag FROM collection_tags WHERE collection_id = ?1 ORDER BY tag ASC")?;
        let tags = stmt
            .query_map([collection_id], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(tags)
    }

    /// Generate an embedding for a collection's searchable text.
    pub async fn embed_collection(
        &self,
        conn: &Connection,
        collection: &Collection,
    ) -> Result<Vec<f32>> {
        let tags = Self::collection_tags(conn, &collection.id)?;
        let text = self.prepare_collection_text(collection, &tags);
        let provider = self.provider()?;

        provider.embed(&text).await.map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate embedding: {}", e))
        })
    }

    /// Store embeddings for multiple collections.
    ///
    /// Tags are loaded from the database for each collection so the embedded
    /// text matches [`prepare_collection_text`].
    pub async fn store_collection_embeddings(
        &self,
        conn: &mut Connection,
        collections: &[Collection],
    ) -> Result<usize> {
        if collections.is_empty() {
            return Ok(0);
        }

        crate::embeddings::VecStore::ensure_extension_loaded();

        let provider = self.provider()?;

        // Prepare texts for batch embedding, enriched with each collection's tags.
        let mut texts = Vec::with_capacity(collections.len());
        for collection in collections {
            let tags = Self::collection_tags(conn, &collection.id)?;
            texts.push(self.prepare_collection_text(collection, &tags));
        }

        let embeddings = provider.embed_batch(&texts).await.map_err(|e| {
            crate::error::Error::Operation(format!("Failed to generate embeddings: {}", e))
        })?;

        let store = self.collection_store(provider.dimensions());
        let entries: Vec<VecStoreEntry> = collections
            .iter()
            .zip(embeddings)
            .map(|(collection, embedding)| VecStoreEntry { id: collection.id.clone(), embedding })
            .collect();

        store.insert_batch(conn, &entries).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to store embeddings: {}", e))
        })?;

        Ok(entries.len())
    }

    /// Search for similar collections by semantic similarity.
    pub async fn search_collections(
        &self,
        conn: &Connection,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        self.search_collections_with_threshold(conn, query, limit, self.threshold).await
    }

    /// Search for similar collections with an optional threshold override.
    pub async fn search_collections_with_threshold(
        &self,
        conn: &Connection,
        query: &str,
        limit: usize,
        threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        let query_embedding = self.embed_query(query).await?;
        self.search_collections_prepared(conn, &query_embedding, limit, threshold)
    }

    /// Find collections that don't have embeddings yet.
    pub fn find_collections_without_embeddings(&self, conn: &Connection) -> Result<Vec<String>> {
        let store = self.collection_store(self.model.dimensions());
        store.find_without_embeddings(conn).map_err(|e| {
            crate::error::Error::Operation(format!(
                "Failed to find collections without embeddings: {}",
                e
            ))
        })
    }

    /// Delete an embedding for a collection.
    pub fn delete_collection_embedding(
        &self,
        conn: &mut Connection,
        collection_id: &str,
    ) -> Result<()> {
        let store = self.collection_store(self.model.dimensions());
        store.delete(conn, collection_id).map_err(|e| {
            crate::error::Error::Operation(format!("Failed to delete embedding: {}", e))
        })
    }

    /// Count total collection embeddings in the store.
    pub fn count_collection_embeddings(&self, conn: &Connection) -> Result<usize> {
        let store = self.collection_store(self.model.dimensions());
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
            query: r#"(function_item name: (identifier) @fn_name (#eq? @fn_name "cycle_mode")) @target"#.to_string(),
            language: "rust".to_string(),
            file_path: "/test.rs".to_string(),
            content_hash: Some("hash".to_string()),
            commit_hash: None,
            health: crate::engine::bookmark::BookmarkHealth::Active,
            resolution_method: Some(crate::engine::bookmark::ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: chrono::Utc::now().to_string(),
            created_by: Some("user".to_string()),
            current_resolution_id: None,
            repo_id: None,
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
            comments: vec![],
        };

        let text = repo.prepare_bookmark_text(&bookmark);
        assert!(text.contains("Tags: tag1, tag2"));
        assert!(text.contains("Note: A test function"));
        assert!(text.contains("Context: Testing utilities"));
        assert!(text.contains("Node Type: function"));
        assert!(text.contains("Node Target: cycle_mode"));
    }

    /// Build a minimal bookmark with the given id, tags, and notes for embedding.
    fn sample_bookmark(id: &str, tags: &[&str], notes: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: r#"(function_item name: (identifier) @fn_name) @target"#.to_string(),
            language: "rust".to_string(),
            file_path: "/test.rs".to_string(),
            content_hash: Some("hash".to_string()),
            commit_hash: None,
            health: crate::engine::bookmark::BookmarkHealth::Active,
            resolution_method: Some(crate::engine::bookmark::ResolutionMethod::Exact),
            last_resolved_at: None,
            stale_since: None,
            created_at: chrono::Utc::now().to_string(),
            created_by: Some("user".to_string()),
            current_resolution_id: None,
            repo_id: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            annotations: vec![crate::engine::bookmark::Annotation {
                id: format!("{id}-ann"),
                bookmark_id: id.to_string(),
                added_at: chrono::Utc::now().to_string(),
                added_by: None,
                notes: Some(notes.to_string()),
                context: None,
                source: None,
            }],
            comments: vec![],
        }
    }

    /// Verifies the embed-once + search_prepared split is behaviorally identical
    /// to the all-in-one `search`: embedding a query once and searching a
    /// connection with the prepared embedding must return the same ids in the
    /// same order as `search`, which embeds inline.
    ///
    /// Requires the AllMiniLmL6V2 model to be cached locally (hf-hub cache).
    /// Ignored by default so CI without model access still passes; run with
    /// `cargo test -p codemark-core embed_query_then_prepared -- --ignored`.
    #[tokio::test]
    #[ignore = "needs the AllMiniLmL6V2 embedding model cached locally"]
    async fn embed_query_then_prepared_matches_search() {
        let repo = SemanticRepo::new(None, EmbeddingModel::AllMiniLmL6V2);

        // In-memory store with the bookmark_embeddings table created for the model.
        crate::embeddings::VecStore::ensure_extension_loaded();
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        VecStore::with_metric(
            EmbeddingModel::AllMiniLmL6V2.dimensions(),
            DistanceMetric::default(),
        )
        .create_table(&mut conn)
        .unwrap();

        let bookmarks = vec![
            sample_bookmark("auth", &["auth", "login"], "Handles user authentication and login"),
            sample_bookmark("parse", &["parser"], "Parses configuration files from disk"),
            sample_bookmark("net", &["network"], "Opens a TCP socket and sends bytes"),
        ];
        let stored = repo.store_embeddings(&mut conn, &bookmarks).await.unwrap();
        assert_eq!(stored, 3);

        let query = "how does user sign in work";

        // All-in-one path.
        let inline = repo.search(&conn, query, 5).await.unwrap();

        // Embed-once + prepared path.
        let emb = repo.embed_query(query).await.unwrap();
        assert_eq!(emb.len(), EmbeddingModel::AllMiniLmL6V2.dimensions());
        let prepared = repo.search_prepared(&conn, &emb, 5, None).unwrap();

        let inline_ids: Vec<&str> = inline.iter().map(|r| r.id.as_str()).collect();
        let prepared_ids: Vec<&str> = prepared.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(inline_ids, prepared_ids, "prepared search must match inline search ordering");

        // Distances must also match since the embedding and store are identical.
        for (a, b) in inline.iter().zip(prepared.iter()) {
            assert_eq!(a.distance, b.distance);
        }
    }
}
