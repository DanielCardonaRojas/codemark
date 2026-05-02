//! Embedding provider trait and common types.

#![allow(dead_code)]

use async_trait::async_trait;

/// Result type for embedding operations.
pub type EmbeddingResult<T> = Result<T, EmbeddingError>;

/// Errors that can occur during embedding generation.
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Failed to load embedding model: {0}")]
    ModelLoad(String),

    #[error("Failed to generate embedding: {0}")]
    Generation(String),

    #[error("Model not initialized")]
    NotInitialized,

    #[error("Invalid input text: {0}")]
    InvalidInput(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HF Hub API error: {0}")]
    HfHub(String),
}

/// Trait for embedding providers (local models, APIs, etc.).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding vector for the given text.
    async fn embed(&self, text: &str) -> EmbeddingResult<Vec<f32>>;

    /// Generate embeddings for multiple texts in batch.
    async fn embed_batch(&self, texts: &[String]) -> EmbeddingResult<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Returns the dimensionality of the embedding vectors produced by this provider.
    fn dimensions(&self) -> usize;

    /// Returns a human-readable name for this provider.
    fn name(&self) -> &str;
}

/// Simple text preparation for embedding.
pub fn prepare_embedding_text(
    tags: &[String],
    notes: Option<&str>,
    context: Option<&str>,
) -> String {
    let mut parts = Vec::new();

    if !tags.is_empty() {
        parts.push(format!("Tags: {}", tags.join(", ")));
    }

    if let Some(notes) = notes {
        parts.push(format!("Note: {notes}"));
    }

    if let Some(context) = context {
        parts.push(format!("Context: {context}"));
    }

    parts.join("\n")
}
