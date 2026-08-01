//! Configuration for embedding models.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Distance metric for vector similarity search.
///
/// sqlite-vec always stores and computes Euclidean (L2) distance. Codemark's
/// embeddings are L2-normalized before storage, so the three metrics are
/// deterministic, monotonic transforms of one another; the configured metric is
/// applied by converting the raw L2 distance in Rust (see
/// [`DistanceMetric::from_l2_distance`]) rather than changing the vec0 table,
/// which would require migrating existing databases.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DistanceMetric {
    /// L2 (Euclidean) distance. 0 = identical; for these normalized embeddings,
    /// unrelated text clusters around √2 ≈ 1.41 (orthogonal vectors).
    /// Default threshold: 1.3.
    #[default]
    L2,
    /// Cosine distance (1 − cosine similarity). 0 = identical, 1 = orthogonal,
    /// 2 = opposite. Default threshold: 0.85.
    Cosine,
    /// Inner product (dot product). For normalized embeddings this equals
    /// cosine similarity: higher = more similar, 0 = orthogonal.
    /// Default threshold: 0.15 (higher is better).
    InnerProduct,
}

impl DistanceMetric {
    /// Returns true if lower distance values indicate better similarity.
    pub fn is_lower_better(&self) -> bool {
        !matches!(self, DistanceMetric::InnerProduct)
    }

    /// Convert a raw sqlite-vec L2 distance into this metric's scale.
    ///
    /// vec0 always returns Euclidean distance; since codemark's embeddings are
    /// L2-normalized (unit vectors) the conversions are exact:
    /// - **L2**: unchanged
    /// - **Cosine distance** = L2² / 2  (= 1 − cosine_similarity for unit vectors)
    /// - **Inner product**    = 1 − L2² / 2  (= cosine_similarity for unit vectors)
    ///
    /// All three are monotonic in L2, so search ordering is preserved.
    pub fn from_l2_distance(&self, l2: f64) -> f64 {
        match self {
            DistanceMetric::L2 => l2,
            DistanceMetric::Cosine => (l2 * l2) / 2.0,
            DistanceMetric::InnerProduct => 1.0 - (l2 * l2) / 2.0,
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

    /// Maximum distance for a match (None = metric default; see SemanticConfig).
    /// Interpretation depends on distance_metric:
    /// - L2 / Cosine: values <= threshold are matches (defaults 1.3 / 0.85)
    /// - InnerProduct: values >= threshold are matches (default 0.15)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_distance_is_identity() {
        assert_eq!(DistanceMetric::L2.from_l2_distance(0.0), 0.0);
        assert_eq!(DistanceMetric::L2.from_l2_distance(1.3), 1.3);
    }

    #[test]
    fn cosine_distance_is_half_l2_squared() {
        // identical (L2 0) -> cosine distance 0; orthogonal (L2 sqrt(2)) -> 1.0
        assert!((DistanceMetric::Cosine.from_l2_distance(0.0)).abs() < 1e-6);
        assert!((DistanceMetric::Cosine.from_l2_distance(2.0_f64.sqrt()) - 1.0).abs() < 1e-6);
        // a measured point: L2 1.3 -> 1.69 / 2 = 0.845
        assert!((DistanceMetric::Cosine.from_l2_distance(1.3) - 0.845).abs() < 1e-6);
    }

    #[test]
    fn inner_product_is_cosine_similarity() {
        // identical -> dot product 1.0; orthogonal -> 0.0
        assert!((DistanceMetric::InnerProduct.from_l2_distance(0.0) - 1.0).abs() < 1e-6);
        assert!((DistanceMetric::InnerProduct.from_l2_distance(2.0_f64.sqrt())).abs() < 1e-6);
        // L2 1.3 -> 1 - 0.845 = 0.155
        assert!((DistanceMetric::InnerProduct.from_l2_distance(1.3) - 0.155).abs() < 1e-6);
    }

    #[test]
    fn conversions_preserve_l2_ordering() {
        // Each metric is monotonic in L2, so search ranking is unaffected by
        // the configured metric. Lower-better metrics ascend with L2; inner
        // product (higher-better) descends.
        let l2s = [0.5, 0.9, 1.1, 1.3, 1.41];
        for metric in [DistanceMetric::L2, DistanceMetric::Cosine, DistanceMetric::InnerProduct] {
            let vals: Vec<f64> = l2s.iter().map(|&d| metric.from_l2_distance(d)).collect();
            if metric.is_lower_better() {
                assert!(vals.windows(2).all(|w| w[0] < w[1]), "{metric:?} not ascending");
            } else {
                assert!(vals.windows(2).all(|w| w[0] > w[1]), "{metric:?} not descending");
            }
        }
    }
}
