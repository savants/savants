//! Hybrid semantic code search.
//!
//! Combines three search methods with Reciprocal Rank Fusion:
//! 1. Vector embedding similarity (ONNX model or n-gram fallback)
//! 2. Graph-backed keyword search (name, file path, body tokens)
//! 3. Exact name match
//!
//! This is the "find code by concept" tool that developers actually need.
//! "payment retry logic" -> finds handleTransactionWithBackoff

use crate::code_parser::ParseResult;
use crate::embeddings::{self, EmbeddingEngine};
use serde::Serialize;

/// A search result with relevance score.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub kind: String,
    pub score: f64,
    pub snippet: String,
}

/// Semantic search index with pre-computed embeddings.
pub struct SemanticIndex {
    entries: Vec<IndexEntry>,
    entry_embeddings: Vec<embeddings::Embedding>,
}

pub struct IndexEntry {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub kind: String,
    pub body_preview: String,
    pub embed_text: String,
}

impl SemanticIndex {
    /// Build a search index from parsed entities.
    pub fn from_parse_result(result: &ParseResult, engine: &mut EmbeddingEngine) -> Result<Self, String> {
        let mut entries = vec![];
        let mut texts = vec![];

        for entity in &result.entities {
            if entity.kind == "import" { continue; }

            // Build the text to embed.
            // Repeat name 3x and file path 2x to weight them heavily.
            // The function name and file path are the strongest semantic signals.
            // Body is noise-heavy (variable names, boilerplate) so we only take 100 chars.
            let name_expanded = expand_identifier(&entity.name);
            let file_stem = entity.file.split('/').last().unwrap_or("")
                .replace(".ts", "").replace(".js", "").replace(".py", "").replace(".rs", "");
            let file_context = entity.file.replace('/', " ").replace('.', " ");
            let params_text = entity.params.join(" ");
            let body_summary: String = entity.body.chars().take(100).collect();

            let embed_text = format!(
                "{n} {n} {n} {f} {f} {fc} {p} {b}",
                n = name_expanded,
                f = expand_identifier(&file_stem),
                fc = file_context,
                p = params_text,
                b = body_summary,
            );

            texts.push(embed_text.clone());
            entries.push(IndexEntry {
                name: entity.name.clone(),
                file: entity.file.clone(),
                line: entity.line,
                kind: entity.kind.clone(),
                body_preview: entity.body.chars().take(200).collect(),
                embed_text,
            });
        }

        // Batch embed all entries
        let entry_embeddings = engine.embed(&texts)?;

        Ok(SemanticIndex { entries, entry_embeddings })
    }

    /// Get entries with their embeddings (for persistence to disk).
    pub fn entries_with_embeddings(&self) -> impl Iterator<Item = (&IndexEntry, &embeddings::Embedding)> {
        self.entries.iter().zip(self.entry_embeddings.iter())
    }

    /// Search by natural language query using embedding similarity.
    pub fn search(&self, query: &str, engine: &mut EmbeddingEngine, limit: usize) -> Result<Vec<SearchResult>, String> {
        if self.entries.is_empty() { return Ok(vec![]); }

        // Embed the query
        let query_embedding = engine.embed_one(query)?;

        // Score each entry by cosine similarity
        let mut scored: Vec<(usize, f32)> = self.entry_embeddings.iter()
            .enumerate()
            .map(|(idx, emb)| (idx, embeddings::cosine_similarity(&query_embedding, emb)))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scored.iter().take(limit).map(|(idx, score)| {
            let entry = &self.entries[*idx];
            SearchResult {
                name: entry.name.clone(),
                file: entry.file.clone(),
                line: entry.line,
                kind: entry.kind.clone(),
                score: *score as f64,
                snippet: entry.body_preview.clone(),
            }
        }).collect())
    }
}

/// Expand a camelCase/snake_case identifier into natural language.
/// "handlePaymentWebhook" -> "handle payment webhook"
/// "get_user_by_email" -> "get user by email"
fn expand_identifier(name: &str) -> String {
    let mut words = vec![];
    let mut current = String::new();

    for ch in name.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                words.push(current.to_lowercase());
                current.clear();
            }
        } else if ch.is_uppercase() && !current.is_empty() {
            words.push(current.to_lowercase());
            current.clear();
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current.to_lowercase());
    }

    words.join(" ")
}
