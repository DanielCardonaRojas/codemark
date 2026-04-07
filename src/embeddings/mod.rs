//! Semantic search via vector embeddings.
//!
//! This module provides embedding generation and vector similarity search
//! for finding bookmarks by meaning rather than exact keywords.

pub mod config;
pub mod local;
pub mod provider;
pub mod vec_store;

pub use config::EmbeddingConfig;
pub use local::LocalEmbeddingProvider;
pub use provider::{EmbeddingProvider, EmbeddingResult};
pub use vec_store::{VecStore, VecStoreEntry};
