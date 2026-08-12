//! Semantic search via vector embeddings.
//!
//! This module provides embedding generation and vector similarity search
//! for finding bookmarks by meaning rather than exact keywords.

#![allow(dead_code)]
#![allow(unused_imports)]

pub mod config;
#[cfg(feature = "semantic")]
pub mod local;
pub mod provider;
pub mod vec_store;

pub use config::{DistanceMetric, EmbeddingConfig, EmbeddingModel};
#[cfg(feature = "semantic")]
pub use local::{LocalEmbeddingProvider, shared_local_provider};
pub use provider::EmbeddingProvider;
pub use vec_store::{SearchResult, VecStore, VecStoreEntry};
