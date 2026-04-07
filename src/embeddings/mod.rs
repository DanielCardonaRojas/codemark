//! Semantic search via vector embeddings.
//!
//! This module provides embedding generation and vector similarity search
//! for finding bookmarks by meaning rather than exact keywords.

pub mod config;
pub mod provider;

pub use config::EmbeddingConfig;
pub use provider::{EmbeddingProvider, EmbeddingResult};
