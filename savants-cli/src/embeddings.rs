//! Code embeddings using fastembed (ONNX-based local embeddings).
//!
//! Embeds function bodies and names into dense vectors for semantic search.
//! The model runs entirely locally - no API keys, no data leaves the machine.
//! Uses all-MiniLM-L6-v2 (22MB ONNX model, 384 dimensions).
//!
//! When the `embeddings` feature is disabled, falls back to character n-gram
//! hashing (lower quality but zero model size).

use std::collections::HashMap;

#[cfg(feature = "embeddings")]
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};

pub type Embedding = Vec<f32>;

/// Embedding engine - wraps fastembed or fallback.
pub struct EmbeddingEngine {
    #[cfg(feature = "embeddings")]
    model: TextEmbedding,
}

impl EmbeddingEngine {
    /// Create a new embedding engine. Downloads model on first use (~22MB).
    pub fn new() -> Result<Self, String> {
        #[cfg(feature = "embeddings")]
        {
            let model = TextEmbedding::try_new(
                InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true)
            ).map_err(|e| format!("Failed to load embedding model: {}", e))?;
            Ok(Self { model })
        }

        #[cfg(not(feature = "embeddings"))]
        {
            Ok(Self {})
        }
    }

    /// Embed a batch of texts into vectors.
    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Embedding>, String> {
        #[cfg(feature = "embeddings")]
        {
            // fastembed's embed takes Vec<String> and batch size
            let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            self.model.embed(text_refs, None)
                .map_err(|e| format!("Embedding failed: {}", e))
        }

        #[cfg(not(feature = "embeddings"))]
        {
            // Fallback: character n-gram hashing (lower quality)
            Ok(texts.iter().map(|t| embed_ngram(t)).collect())
        }
    }

    /// Embed a single text.
    pub fn embed_one(&mut self, text: &str) -> Result<Embedding, String> {
        let results = self.embed(&[text.to_string()])?;
        results.into_iter().next().ok_or("No embedding result".to_string())
    }
}

/// Cosine similarity between two embeddings.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom > 0.0 { dot / denom } else { 0.0 }
}

/// Reciprocal Rank Fusion: merge multiple ranked lists into one.
pub fn reciprocal_rank_fusion(ranked_lists: &[Vec<(String, usize)>], k: f32) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();
    for list in ranked_lists {
        for (item, rank) in list {
            *scores.entry(item.clone()).or_default() += 1.0 / (k + *rank as f32);
        }
    }
    let mut results: Vec<(String, f32)> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    results
}

/// Fallback: character n-gram hashing (used when fastembed feature is disabled).
#[cfg(not(feature = "embeddings"))]
fn embed_ngram(text: &str) -> Vec<f32> {
    const DIM: usize = 128;
    let mut vec = vec![0.0f32; DIM];
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut count = 0u32;

    for n in 3..=6usize.min(chars.len()) {
        for window in chars.windows(n) {
            let ngram: String = window.iter().collect();
            let hash = fnv_hash(&ngram);
            let idx = (hash as usize) % DIM;
            let sign = if (hash >> 31) & 1 == 0 { 1.0 } else { -1.0 };
            vec[idx] += sign;
            count += 1;
        }
    }

    if count > 0 {
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 { for v in vec.iter_mut() { *v /= norm; } }
    }
    vec
}

#[cfg(not(feature = "embeddings"))]
fn fnv_hash(s: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for byte in s.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}
