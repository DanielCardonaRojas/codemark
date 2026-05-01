//! Configuration for embedding models.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Distance metric for vector similarity search.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DistanceMetric {
    /// L2 (Euclidean) distance.
    /// Range: [0, ∞), where 0 = identical, higher = more different.
    /// Typical threshold: 0.3-0.5.
    #[default]
    L2,
    /// Cosine distance (1 - cosine similarity).
    /// Range: [0, 2], where 0 = identical, 1 = orthogonal, 2 = opposite.
    /// Typical threshold: 0.3-0.5.
    Cosine,
    /// Inner product (dot product).
    /// Range: (-∞, ∞), where higher = more similar for normalized vectors.
    /// Typical threshold: 0.7-0.9 (similarity score).
    InnerProduct,
}

impl DistanceMetric {
    /// Returns true if lower distance values indicate better similarity.
    pub fn is_lower_better(&self) -> bool {
        !matches!(self, DistanceMetric::InnerProduct)
    }

    /// Returns the sqlite-vec metric name for use in queries.
    pub fn as_vec_name(&self) -> &'static str {
        match self {
            DistanceMetric::L2 => "l2",
            DistanceMetric::Cosine => "cosine",
            DistanceMetric::InnerProduct => "ip",
        }
    }
}

impl FromStr for DistanceMetric {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "l2" | "euclidean" => Ok(DistanceMetric::L2),
            "cosine" => Ok(DistanceMetric::Cosine),
            "ip" | "inner" | "innerproduct" | "dot" => Ok(DistanceMetric::InnerProduct),
            _ => Err(format!("Unknown distance metric: {}. Valid options: l2, cosine, ip", s)),
        }
    }
}

/// Available embedding models.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EmbeddingModel {
    /// all-MiniLM-L6-v2: 384 dimensions, ~80MB model
    #[serde(alias = "all-MiniLM-L6-v2")]
    #[default]
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

    /// Distance metric for similarity search.
    #[serde(default)]
    pub distance_metric: DistanceMetric,

    /// Maximum distance for a match (None = no threshold).
    /// Interpretation depends on distance_metric:
    /// - L2: values <= threshold are matches (typical: 0.3-0.5)
    /// - Cosine: values <= threshold are matches (typical: 0.3-0.5)
    /// - InnerProduct: values >= threshold are matches (typical: 0.7-0.9)
    pub threshold: Option<f32>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            enabled: true,
            model: EmbeddingModel::default(),
            batch_size: 32,
            distance_metric: DistanceMetric::default(),
            threshold: None,
        }
    }
}
