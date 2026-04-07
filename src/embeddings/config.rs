//! Configuration for embedding models.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Available embedding models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EmbeddingModel {
    /// all-MiniLM-L6-v2: 384 dimensions, ~80MB model
    #[serde(alias = "all-MiniLM-L6-v2")]
    AllMiniLmL6V2,
    /// bge-small-en-v1.5: 384 dimensions, ~130MB model
    #[serde(alias = "bge-small-en-v1.5")]
    BgeSmallEnV1_5,
}

impl EmbeddingModel {
    /// Returns the embedding dimension for this model.
    pub fn dimensions(&self) -> usize {
        match self {
            EmbeddingModel::AllMiniLmL6V2 => 384,
            EmbeddingModel::BgeSmallEnV1_5 => 384,
        }
    }

    /// Returns the HuggingFace model ID.
    pub fn model_id(&self) -> &'static str {
        match self {
            EmbeddingModel::AllMiniLmL6V2 => "sentence-transformers/all-MiniLM-L6-v2",
            EmbeddingModel::BgeSmallEnV1_5 => "BAAI/bge-small-en-v1.5",
        }
    }
}

impl Default for EmbeddingModel {
    fn default() -> Self {
        EmbeddingModel::AllMiniLmL6V2
    }
}

impl FromStr for EmbeddingModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "all-minilm-l6-v2" | "all_minilm_l6_v2" => Ok(EmbeddingModel::AllMiniLmL6V2),
            "bge-small-en-v1.5" | "bge_small_en_v1_5" => Ok(EmbeddingModel::BgeSmallEnV1_5),
            _ => Err(format!("Unknown embedding model: {}", s)),
        }
    }
}

/// Semantic search configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Whether semantic search is enabled.
    pub enabled: bool,

    /// The embedding model to use.
    pub model: EmbeddingModel,

    /// Batch size for embedding generation.
    pub batch_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            enabled: true,
            model: EmbeddingModel::default(),
            batch_size: 32,
        }
    }
}
