//! Lightweight code embeddings using character n-gram hashing.
//!
//! Instead of a neural network, we use a technique inspired by FastText:
//! hash character n-grams of each token into a fixed-size vector,
//! then average across all tokens. This captures subword similarity
//! ("payment" and "pay" share n-grams) without any model file.
//!
//! Zero binary size increase. No ONNX. No API keys.
//! Accuracy is lower than neural embeddings but combined with
//! BM25 and exact search via RRF, it's competitive.

use std::collections::HashMap;

const EMBEDDING_DIM: usize = 128;
const NGRAM_MIN: usize = 3;
const NGRAM_MAX: usize = 6;

/// A fixed-size embedding vector.
pub type Embedding = [f32; EMBEDDING_DIM];

/// Embed a piece of text into a fixed-size vector using character n-gram hashing.
pub fn embed_text(text: &str) -> Embedding {
    let mut vec = [0.0f32; EMBEDDING_DIM];
    let mut count = 0u32;

    // Tokenize: split on non-alphanumeric, also split camelCase
    let tokens = tokenize(text);

    for token in &tokens {
        // Generate character n-grams for each token
        let padded = format!("<{}>", token.to_lowercase());
        let chars: Vec<char> = padded.chars().collect();

        for n in NGRAM_MIN..=NGRAM_MAX.min(chars.len()) {
            for window in chars.windows(n) {
                let ngram: String = window.iter().collect();
                // Hash the n-gram to a position in the vector
                let hash = fnv_hash(&ngram);
                let idx = (hash as usize) % EMBEDDING_DIM;
                // Use the sign bit to determine +1 or -1 (random projection)
                let sign = if (hash >> 31) & 1 == 0 { 1.0 } else { -1.0 };
                vec[idx] += sign;
                count += 1;
            }
        }
    }

    // Normalize
    if count > 0 {
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in vec.iter_mut() {
                *v /= norm;
            }
        }
    }

    vec
}

/// Cosine similarity between two embeddings.
pub fn cosine_similarity(a: &Embedding, b: &Embedding) -> f32 {
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..EMBEDDING_DIM {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom > 0.0 { dot / denom } else { 0.0 }
}

/// Reciprocal Rank Fusion: merge multiple ranked lists into one.
/// Each input is a list of (item_id, rank_position).
/// Returns items sorted by fused score (highest first).
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

/// Tokenize text: split on non-alphanumeric, split camelCase, lowercase.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = vec![];
    let mut current = String::new();

    for ch in text.chars() {
        if ch == '_' || ch == '-' || ch == '.' || ch == '/' || ch == ' '
            || ch == '(' || ch == ')' || ch == '{' || ch == '}' || ch == ';'
            || ch == ':' || ch == ',' || ch == '\n' || ch == '\t' {
            if !current.is_empty() {
                tokens.extend(split_camel(&current));
                current.clear();
            }
        } else if ch.is_uppercase() && !current.is_empty()
            && current.chars().last().map(|c| c.is_lowercase()).unwrap_or(false) {
            tokens.extend(split_camel(&current));
            current.clear();
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        tokens.extend(split_camel(&current));
    }

    // Filter noise
    tokens.retain(|t| t.len() >= 2);
    tokens.iter().map(|t| t.to_lowercase()).collect()
}

fn split_camel(s: &str) -> Vec<String> {
    let mut parts = vec![];
    let mut current = String::new();
    for ch in s.chars() {
        if ch.is_uppercase() && !current.is_empty() {
            parts.push(current.to_lowercase());
            current.clear();
        }
        current.push(ch);
    }
    if !current.is_empty() {
        parts.push(current.to_lowercase());
    }
    parts
}

/// FNV-1a hash for strings.
fn fnv_hash(s: &str) -> u32 {
    let mut hash: u32 = 2166136261;
    for byte in s.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_similar_concepts_have_higher_similarity() {
        let payment_handler = embed_text("handlePaymentTransaction retry backoff stripe charge");
        let payment_query = embed_text("payment retry logic");
        let unrelated = embed_text("render dashboard component user interface React");

        let sim_related = cosine_similarity(&payment_handler, &payment_query);
        let sim_unrelated = cosine_similarity(&payment_handler, &unrelated);

        assert!(sim_related > sim_unrelated,
            "Related concepts should have higher similarity: {} vs {}",
            sim_related, sim_unrelated);
    }

    #[test]
    fn test_rrf_merges_ranked_lists() {
        let list1 = vec![("a".to_string(), 0), ("b".to_string(), 1), ("c".to_string(), 2)];
        let list2 = vec![("b".to_string(), 0), ("c".to_string(), 1), ("a".to_string(), 2)];

        let fused = reciprocal_rank_fusion(&[list1, list2], 60.0);
        // "b" appears at rank 0+1 in list1 and rank 0 in list2, should rank highest
        assert_eq!(fused[0].0, "b");
    }
}
