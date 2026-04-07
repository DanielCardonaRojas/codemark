//! Local embedding model support using Candle.

use super::{config::EmbeddingModel, provider::{EmbeddingError, EmbeddingProvider, EmbeddingResult}};
use async_trait::async_trait;
use candle_core::{Device, Result as CandleResult, Tensor, DType};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::{api::sync::Api, Repo, RepoType};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Mean pooling layer for sentence embeddings.
struct MeanPooling;

impl MeanPooling {
    fn apply(&self, hidden_states: &Tensor, attention_mask: &Tensor) -> CandleResult<Tensor> {
        // hidden_states: [batch, seq_len, hidden_dim]
        // attention_mask: [batch, seq_len]

        let (_batch_size, _seq_len, _hidden_dim) = hidden_states.dims3()?;

        // Expand attention mask to [batch, seq_len, 1] for broadcasting
        let mask_3d = attention_mask.unsqueeze(2)?;

        // Multiply hidden states by mask (zeros out padding tokens)
        let masked = hidden_states.mul(&mask_3d)?;

        // Sum over sequence length: [batch, hidden_dim]
        let sum = masked.sum(1)?;

        // Count non-padding tokens: [batch, 1]
        let mask_sum = mask_3d.sum(1)?;

        // Add epsilon to avoid division by zero, then reshape for broadcasting
        let epsilon = 1e-9_f32;
        let mask_sum_safe = mask_sum.add(&Tensor::new(&[epsilon], mask_sum.device())?)?;

        // Reshape mask_sum_safe to [batch, 1] for proper broadcasting
        let batch_size = mask_sum.dims()[0];
        let mask_sum_2d = mask_sum_safe.reshape((batch_size, 1))?;

        // Mean pooling: divide sum by count
        let pooled = sum.div(&mask_sum_2d.broadcast_as(sum.shape())?)?;
        Ok(pooled)
    }
}

/// BERT-based sentence embedding model.
struct BertSentenceEmbedder {
    model: BertModel,
    device: Device,
    pooling: MeanPooling,
}

impl BertSentenceEmbedder {
    /// Load model from path.
    fn load(model_path: &Path, device: &Device) -> CandleResult<Self> {
        let config_path = model_path.join("config.json");
        let weights_path = model_path.join("model.safetensors");

        let config_content = std::fs::read_to_string(config_path)?;
        let config: BertConfig = serde_json::from_str(&config_content)
            .map_err(|e| candle_core::Error::Msg(format!("Failed to parse config: {}", e)))?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                &[weights_path.clone()],
                DType::F32,
                device,
            )?
        };

        let model = BertModel::load(vb, &config)?;

        Ok(BertSentenceEmbedder {
            model,
            device: device.clone(),
            pooling: MeanPooling,
        })
    }

    /// Encode texts to embeddings.
    fn encode(
        &self,
        texts: &[&str],
        tokenizer: &tokenizers::Tokenizer,
    ) -> CandleResult<Vec<Vec<f32>>> {
        let mut embeddings = Vec::new();

        for &text in texts {
            let tokens = tokenizer
                .encode(text, true)
                .map_err(|e| candle_core::Error::Msg(e.to_string()))?;

            let input_ids: Vec<u32> = tokens.get_ids().iter().map(|&id| id as u32).collect();
            let attention_mask: Vec<u32> = tokens.get_attention_mask().iter().map(|&id| id as u32).collect();
            let token_type_ids: Vec<u32> = tokens.get_type_ids().iter().map(|&id| id as u32).collect();

            let seq_len = input_ids.len();
            let input_ids_tensor = Tensor::new(input_ids.as_slice(), &self.device)?.reshape((1, seq_len))?;
            let attention_mask_tensor = Tensor::new(attention_mask.as_slice(), &self.device)?.reshape((1, seq_len))?;
            let token_type_ids_tensor = Tensor::new(token_type_ids.as_slice(), &self.device)?.reshape((1, seq_len))?;

            // Forward through BERT
            let hidden_states = self.model.forward(
                &input_ids_tensor,
                &token_type_ids_tensor,
                Some(&attention_mask_tensor),
            )?;

            // Apply mean pooling
            let pooled = self.pooling.apply(&hidden_states, &attention_mask_tensor)?;

            // Normalize the embedding (L2 normalization)
            let norm = (pooled.sqr()?.sum_keepdim(1)? + 1e-9)?.sqrt()?;
            let normalized = (pooled / norm)?.to_dtype(DType::F32)?;

            let embedding = normalized.to_vec1()?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }
}

/// Local embedding provider using Candle.
pub struct LocalEmbeddingProvider {
    model: Mutex<Option<BertSentenceEmbedder>>,
    tokenizer: Mutex<Option<tokenizers::Tokenizer>>,
    device: Device,
    model_name: EmbeddingModel,
    model_path: PathBuf,
}

impl LocalEmbeddingProvider {
    /// Create a new local embedding provider.
    pub fn new(model: EmbeddingModel, cache_dir: Option<PathBuf>) -> EmbeddingResult<Self> {
        let device = Device::Cpu;

        let model_path = if let Some(cache) = cache_dir {
            std::fs::create_dir_all(&cache)
                .map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;
            cache
        } else {
            PathBuf::from(".")
        };

        Ok(LocalEmbeddingProvider {
            model: Mutex::new(None),
            tokenizer: Mutex::new(None),
            device,
            model_name: model,
            model_path,
        })
    }

    /// Ensure model and tokenizer are loaded.
    fn ensure_loaded(&self) -> CandleResult<()> {
        let mut model_guard = self.model.lock().map_err(|_| {
            candle_core::Error::Msg("Poison error getting model lock".to_string())
        })?;

        if model_guard.is_none() {
            let (model_path, tokenizer_path) = self.download_if_needed()?;

            let model = BertSentenceEmbedder::load(&model_path, &self.device)?;
            let tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path)
                .map_err(|e: tokenizers::Error| candle_core::Error::Msg(e.to_string()))?;

            *model_guard = Some(model);

            let mut tokenizer_guard = self.tokenizer.lock().map_err(|_| {
                candle_core::Error::Msg("Poison error getting tokenizer lock".to_string())
            })?;
            *tokenizer_guard = Some(tokenizer);
        }

        Ok(())
    }

    /// Download model files if not present.
    fn download_if_needed(&self) -> CandleResult<(PathBuf, PathBuf)> {
        let config_path = self.model_path.join("config.json");
        let weights_path = self.model_path.join("model.safetensors");
        let tokenizer_path = self.model_path.join("tokenizer.json");

        if config_path.exists() && weights_path.exists() && tokenizer_path.exists() {
            return Ok((self.model_path.clone(), tokenizer_path));
        }

        // Download from HF Hub
        let api = Api::new().map_err(|e| candle_core::Error::Msg(e.to_string()))?;

        let repo = match self.model_name {
            EmbeddingModel::AllMiniLmL6V2 => {
                Repo::with_revision(
                    "sentence-transformers/all-MiniLM-L6-v2".to_string(),
                    RepoType::Model,
                    "refs/pr/21".to_string()
                )
            }
            EmbeddingModel::BgeSmallEnV1_5 => {
                Repo::new("BAAI/bge-small-en-v1.5".to_string(), RepoType::Model)
            }
        };

        let api_repo = api.repo(repo);

        let config = api_repo.get("config.json").map_err(|e| {
            candle_core::Error::Msg(format!("Failed to download config: {}", e))
        })?;
        let tokenizer = api_repo.get("tokenizer.json").map_err(|e| {
            candle_core::Error::Msg(format!("Failed to download tokenizer: {}", e))
        })?;
        let weights = api_repo.get("model.safetensors").map_err(|e| {
            candle_core::Error::Msg(format!("Failed to download weights: {}", e))
        })?;

        // Copy to cache location
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&config, &config_path)?;
        std::fs::copy(&tokenizer, &tokenizer_path)?;
        std::fs::copy(&weights, &weights_path)?;

        Ok((self.model_path.clone(), tokenizer_path))
    }
}

#[async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    async fn embed(&self, text: &str) -> EmbeddingResult<Vec<f32>> {
        self.ensure_loaded()
            .map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;

        let model_guard = self.model.lock().map_err(|_| {
            EmbeddingError::ModelLoad("Poison error".to_string())
        })?;
        let tokenizer_guard = self.tokenizer.lock().map_err(|_| {
            EmbeddingError::ModelLoad("Poison error".to_string())
        })?;

        let model = model_guard.as_ref().expect("Model not loaded");
        let tokenizer = tokenizer_guard.as_ref().expect("Tokenizer not loaded");

        let results = model.encode(&[text], tokenizer)
            .map_err(|e| EmbeddingError::Generation(e.to_string()))?;

        Ok(results.into_iter().next().unwrap_or_default())
    }

    async fn embed_batch(&self, texts: &[String]) -> EmbeddingResult<Vec<Vec<f32>>> {
        self.ensure_loaded()
            .map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;

        let model_guard = self.model.lock().map_err(|_| {
            EmbeddingError::ModelLoad("Poison error".to_string())
        })?;
        let tokenizer_guard = self.tokenizer.lock().map_err(|_| {
            EmbeddingError::ModelLoad("Poison error".to_string())
        })?;

        let model = model_guard.as_ref().expect("Model not loaded");
        let tokenizer = tokenizer_guard.as_ref().expect("Tokenizer not loaded");

        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        model.encode(&text_refs, tokenizer)
            .map_err(|e| EmbeddingError::Generation(e.to_string()))
    }

    fn dimensions(&self) -> usize {
        self.model_name.dimensions()
    }

    fn name(&self) -> &str {
        self.model_name.model_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn test_local_provider_creation() {
        let provider = LocalEmbeddingProvider::new(EmbeddingModel::AllMiniLmL6V2, None);
        assert!(provider.is_ok());
    }
}
