//! Local embedding model support using Candle.

use super::{
    config::EmbeddingModel,
    provider::{EmbeddingError, EmbeddingProvider, EmbeddingResult},
};
use async_trait::async_trait;
use candle_core::{DType, Device, Result as CandleResult, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::{Repo, RepoType, api::sync::Api};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Mean pooling layer for sentence embeddings.
struct MeanPooling;

impl MeanPooling {
    fn apply(&self, hidden_states: &Tensor, attention_mask: &Tensor) -> CandleResult<Tensor> {
        // hidden_states: [batch, seq_len, hidden_dim]
        // attention_mask: [batch, seq_len] (f32)

        let (batch_size, seq_len, hidden_dim) = hidden_states.dims3()?;

        // Expand attention mask to [batch, seq_len, 1] for broadcasting
        let mask_3d = attention_mask.unsqueeze(2)?;

        // Broadcast mask to match hidden_states shape for multiplication
        let mask_broadcasted = mask_3d.broadcast_as((batch_size, seq_len, hidden_dim))?;

        // Multiply hidden states by mask (zeros out padding tokens)
        let masked = hidden_states.mul(&mask_broadcasted)?;

        // Sum over sequence length: [batch, hidden_dim]
        let sum = masked.sum(1)?;

        // Sum mask over sequence to get valid token counts: [batch, 1]
        let mask_sum = mask_3d.sum(1)?;

        // Add epsilon to avoid division by zero
        let epsilon = 1e-9_f32;
        let epsilon_tensor = Tensor::new(&[epsilon], sum.device())?.reshape((1, 1))?;
        let mask_sum_safe = mask_sum.add(&epsilon_tensor)?;

        // Create normalized sum by dividing each row by its count
        // Since Candle's broadcasting is tricky, we'll do it manually
        let mut result = Vec::with_capacity(batch_size * hidden_dim);
        let sum_vals = sum.to_vec2::<f32>()?;
        let count_vals = mask_sum_safe.to_vec2::<f32>()?;

        for b in 0..batch_size {
            let count = count_vals[b][0];
            result.extend(sum_vals[b].iter().take(hidden_dim).map(|&v| v / count));
        }

        Tensor::from_vec(result.clone(), (batch_size, hidden_dim), sum.device())
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
                std::slice::from_ref(&weights_path),
                DType::F32,
                device,
            )?
        };

        let model = BertModel::load(vb, &config)?;

        Ok(BertSentenceEmbedder { model, device: device.clone(), pooling: MeanPooling })
    }

    /// Encode texts to embeddings.
    fn encode(
        &self,
        texts: &[&str],
        tokenizer: &tokenizers::Tokenizer,
    ) -> CandleResult<Vec<Vec<f32>>> {
        let mut embeddings = Vec::new();

        for &text in texts {
            let tokens =
                tokenizer.encode(text, true).map_err(|e| candle_core::Error::Msg(e.to_string()))?;

            let input_ids: Vec<u32> = tokens.get_ids().to_vec();
            let attention_mask: Vec<u32> = tokens.get_attention_mask().to_vec();
            let token_type_ids: Vec<u32> = tokens.get_type_ids().to_vec();

            let seq_len = input_ids.len();
            let input_ids_tensor =
                Tensor::new(input_ids.as_slice(), &self.device)?.reshape((1, seq_len))?;
            let attention_mask_tensor =
                Tensor::new(attention_mask.as_slice(), &self.device)?.reshape((1, seq_len))?;
            let token_type_ids_tensor =
                Tensor::new(token_type_ids.as_slice(), &self.device)?.reshape((1, seq_len))?;

            // Convert attention mask to f32 for pooling operations
            let attention_mask_f32 = attention_mask_tensor.to_dtype(DType::F32)?;

            // Forward through BERT
            let hidden_states = self.model.forward(
                &input_ids_tensor,
                &token_type_ids_tensor,
                Some(&attention_mask_tensor),
            )?;

            // Apply mean pooling with f32 mask
            let pooled = self.pooling.apply(&hidden_states, &attention_mask_f32)?;

            // Normalize the embedding (L2 normalization)
            let pooled_squared = pooled.sqr()?;
            let pooled_sum = pooled_squared.sum_keepdim(1)?;
            let norm = (pooled_sum + 1e-9)?.sqrt()?;

            // Manual division to avoid broadcasting issues
            let pooled_vals = pooled.to_vec2::<f32>()?;
            let norm_vals = norm.to_vec2::<f32>()?;
            let mut normalized_vals = Vec::with_capacity(pooled_vals.len());

            for b in 0..pooled_vals.len() {
                let n = norm_vals[b][0];
                for &val in &pooled_vals[b] {
                    normalized_vals.push(val / n);
                }
            }

            let normalized =
                Tensor::from_vec(normalized_vals.clone(), pooled.shape(), pooled.device())?
                    .to_dtype(DType::F32)?;

            // Squeeze or flatten to get 1D vector
            let embedding = if normalized.dims().len() == 2 {
                let (b, h) = normalized.dims2()?;
                if b == 1 {
                    normalized.reshape((h,))?.to_vec1()?
                } else {
                    return Err(candle_core::Error::Msg("Unexpected batch size".to_string()));
                }
            } else {
                normalized.to_vec1()?
            };
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
        let mut model_guard = self
            .model
            .lock()
            .map_err(|_| candle_core::Error::Msg("Poison error getting model lock".to_string()))?;

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
            EmbeddingModel::AllMiniLmL6V2 => Repo::with_revision(
                "sentence-transformers/all-MiniLM-L6-v2".to_string(),
                RepoType::Model,
                "refs/pr/21".to_string(),
            ),
            EmbeddingModel::BgeSmallEnV1_5 => {
                Repo::new("BAAI/bge-small-en-v1.5".to_string(), RepoType::Model)
            }
        };

        let api_repo = api.repo(repo);

        let config = api_repo
            .get("config.json")
            .map_err(|e| candle_core::Error::Msg(format!("Failed to download config: {}", e)))?;
        let tokenizer = api_repo
            .get("tokenizer.json")
            .map_err(|e| candle_core::Error::Msg(format!("Failed to download tokenizer: {}", e)))?;
        let weights = api_repo
            .get("model.safetensors")
            .map_err(|e| candle_core::Error::Msg(format!("Failed to download weights: {}", e)))?;

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
        self.ensure_loaded().map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;

        let model_guard =
            self.model.lock().map_err(|_| EmbeddingError::ModelLoad("Poison error".to_string()))?;
        let tokenizer_guard = self
            .tokenizer
            .lock()
            .map_err(|_| EmbeddingError::ModelLoad("Poison error".to_string()))?;

        let model = model_guard.as_ref().expect("Model not loaded");
        let tokenizer = tokenizer_guard.as_ref().expect("Tokenizer not loaded");

        let results = model
            .encode(&[text], tokenizer)
            .map_err(|e| EmbeddingError::Generation(e.to_string()))?;

        Ok(results.into_iter().next().unwrap_or_default())
    }

    async fn embed_batch(&self, texts: &[String]) -> EmbeddingResult<Vec<Vec<f32>>> {
        self.ensure_loaded().map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;

        let model_guard =
            self.model.lock().map_err(|_| EmbeddingError::ModelLoad("Poison error".to_string()))?;
        let tokenizer_guard = self
            .tokenizer
            .lock()
            .map_err(|_| EmbeddingError::ModelLoad("Poison error".to_string()))?;

        let model = model_guard.as_ref().expect("Model not loaded");
        let tokenizer = tokenizer_guard.as_ref().expect("Tokenizer not loaded");

        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        model.encode(&text_refs, tokenizer).map_err(|e| EmbeddingError::Generation(e.to_string()))
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
