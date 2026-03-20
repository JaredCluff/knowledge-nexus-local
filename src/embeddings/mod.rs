//! ONNX-based text embeddings using MiniLM-L6-v2.
//!
//! Provides local, offline text embedding generation using ONNX Runtime.
//! The model is automatically downloaded on first use.

use anyhow::{Context, Result};
use ndarray::Array2;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use std::path::PathBuf;
use tokenizers::Tokenizer;
use tracing::info;

use crate::config;

/// Embedding dimension for MiniLM-L6-v2
pub const EMBEDDING_DIM: usize = 384;

/// Maximum sequence length for the model
const MAX_SEQ_LENGTH: usize = 256;

/// Embedding model using ONNX Runtime
pub struct EmbeddingModel {
    session: Session,
    tokenizer: Tokenizer,
}

impl EmbeddingModel {
    /// Create a new embedding model, downloading if necessary
    pub fn new() -> Result<Self> {
        info!("Initializing embedding model...");

        // Get model directory
        let model_dir = config::data_dir().join("models");
        std::fs::create_dir_all(&model_dir)?;

        // Download model if not present
        let model_path = model_dir.join("minilm-l6-v2.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");

        if !model_path.exists() {
            info!("Downloading MiniLM-L6-v2 model...");
            download_model(&model_path)?;
        }

        if !tokenizer_path.exists() {
            info!("Downloading tokenizer...");
            download_tokenizer(&tokenizer_path)?;
        }

        // Load ONNX session
        let session = Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(4)?
            .commit_from_file(&model_path)
            .context("Failed to load ONNX model")?;

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

        info!("Embedding model ready");
        Ok(Self { session, tokenizer })
    }

    /// Generate embedding for a single text
    pub fn embed_text(&mut self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed_batch(&[text])?;
        embeddings
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No embeddings returned"))
    }

    /// Generate embeddings for multiple texts
    pub fn embed_batch(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let batch_size = texts.len();

        // Tokenize
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

        // Prepare inputs
        let max_len = encodings
            .iter()
            .map(|e| e.get_ids().len().min(MAX_SEQ_LENGTH))
            .max()
            .unwrap_or(0);

        let mut input_ids = vec![0i64; batch_size * max_len];
        let mut attention_mask = vec![0i64; batch_size * max_len];
        let token_type_ids = vec![0i64; batch_size * max_len];

        for (i, encoding) in encodings.iter().enumerate() {
            let ids = encoding.get_ids();
            let mask = encoding.get_attention_mask();

            let len = ids.len().min(max_len);
            for j in 0..len {
                input_ids[i * max_len + j] = ids[j] as i64;
                attention_mask[i * max_len + j] = mask[j] as i64;
            }
        }

        // Create tensors
        let input_ids_arr = Array2::from_shape_vec((batch_size, max_len), input_ids)?;
        let attention_mask_arr =
            Array2::from_shape_vec((batch_size, max_len), attention_mask.clone())?;
        let token_type_ids_arr = Array2::from_shape_vec((batch_size, max_len), token_type_ids)?;

        // Convert to ort Tensors
        let input_ids_tensor = Tensor::from_array(input_ids_arr)?;
        let attention_mask_tensor = Tensor::from_array(attention_mask_arr)?;
        let token_type_ids_tensor = Tensor::from_array(token_type_ids_arr)?;

        // Run inference
        let outputs = self.session.run(ort::inputs![
            "input_ids" => input_ids_tensor,
            "attention_mask" => attention_mask_tensor,
            "token_type_ids" => token_type_ids_tensor,
        ])?;

        // Get output tensor (last_hidden_state)
        let output = outputs["last_hidden_state"].try_extract_array::<f32>()?;

        // Mean pooling over sequence length
        let output_view = output.view();
        let shape = output_view.shape();
        let batch = shape[0];
        let seq_len = shape[1];
        let hidden_dim = shape[2];

        let mut embeddings = Vec::with_capacity(batch);
        for b in 0..batch {
            let mut pooled = vec![0f32; hidden_dim];
            let mut count = 0f32;

            for s in 0..seq_len {
                let mask_val = attention_mask[b * max_len + s] as f32;
                if mask_val > 0.0 {
                    for h in 0..hidden_dim {
                        pooled[h] += output_view[[b, s, h]] * mask_val;
                    }
                    count += mask_val;
                }
            }

            // Normalize
            if count > 0.0 {
                for val in pooled.iter_mut() {
                    *val /= count;
                }
            }

            // L2 normalize
            let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in pooled.iter_mut() {
                    *val /= norm;
                }
            }

            embeddings.push(pooled);
        }

        Ok(embeddings)
    }
}

/// Download the ONNX model
fn download_model(path: &PathBuf) -> Result<()> {
    // MiniLM-L6-v2 ONNX model from Hugging Face
    const MODEL_URL: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx";

    download_file(MODEL_URL, path)
}

/// Download the tokenizer
fn download_tokenizer(path: &PathBuf) -> Result<()> {
    const TOKENIZER_URL: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json";

    download_file(TOKENIZER_URL, path)
}

/// Download a file from URL
fn download_file(url: &str, path: &PathBuf) -> Result<()> {
    info!("Downloading: {}", url);

    // Build client with timeout to prevent hanging forever
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout for large models
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client.get(url).send().context("Failed to download file")?;

    if !response.status().is_success() {
        anyhow::bail!("Download failed with status: {}", response.status());
    }

    const MAX_DOWNLOAD_BYTES: u64 = 500 * 1024 * 1024; // 500 MB
    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_BYTES {
            anyhow::bail!(
                "Model file too large to download: {} bytes (limit {} bytes)",
                len,
                MAX_DOWNLOAD_BYTES
            );
        }
    }

    let bytes = response.bytes()?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        anyhow::bail!("Downloaded model exceeds size limit: {} bytes", bytes.len());
    }

    // Write to temp file first, then rename (atomic on Unix; on Windows
    // we must remove the target first since rename fails if it exists)
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, &bytes)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(&temp_path, path)?;

    info!("Downloaded to: {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

/// Cosine similarity between two embeddings
#[allow(dead_code)]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore] // Requires model download
    fn test_embedding_generation() {
        let mut model = EmbeddingModel::new().unwrap();

        let embedding = model.embed_text("Hello, world!").unwrap();
        assert_eq!(embedding.len(), EMBEDDING_DIM);

        // Embedding should be normalized
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    #[ignore] // Requires model download
    fn test_semantic_similarity() {
        let mut model = EmbeddingModel::new().unwrap();

        let emb1 = model.embed_text("The cat sat on the mat").unwrap();
        let emb2 = model.embed_text("A kitten is resting on a rug").unwrap();
        let emb3 = model
            .embed_text("Python is a programming language")
            .unwrap();

        let sim_related = cosine_similarity(&emb1, &emb2);
        let sim_unrelated = cosine_similarity(&emb1, &emb3);

        // Related sentences should have higher similarity
        assert!(sim_related > sim_unrelated);
    }
}
