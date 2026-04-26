//! Semantic code search using BM25 ranking.
//!
//! Searches function bodies, names, and file paths using BM25 (Best Matching 25).
//! No external model, no API keys, no binary size increase.
//! Works entirely offline from parsed code entities.

use crate::code_parser::{ParsedEntity, ParseResult};
use crate::embeddings;
use serde::Serialize;
use std::collections::HashMap;

/// A search result with BM25 relevance score.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub kind: String,
    pub score: f64,
    pub snippet: String,
}

/// Hybrid search index: BM25 + character n-gram embeddings + exact name match.
/// Results are merged using Reciprocal Rank Fusion (RRF).
pub struct SemanticIndex {
    documents: Vec<Document>,
    /// Term -> list of (doc_index, term_frequency)
    inverted_index: HashMap<String, Vec<(usize, f64)>>,
    /// Pre-computed embeddings for each document
    embeddings: Vec<embeddings::Embedding>,
    avg_doc_len: f64,
    doc_count: usize,
}

struct Document {
    name: String,
    file: String,
    line: usize,
    kind: String,
    body: String,
    tokens: Vec<String>,
    token_count: usize,
}

impl SemanticIndex {
    /// Build a search index from parsed entities.
    pub fn from_parse_result(result: &ParseResult) -> Self {
        let mut documents = vec![];
        let mut inverted_index: HashMap<String, Vec<(usize, f64)>> = HashMap::new();
        let mut total_tokens = 0usize;

        for entity in &result.entities {
            if entity.kind == "import" { continue; }

            // Tokenize: split name (camelCase/snake_case) + body into searchable tokens
            let mut tokens = vec![];

            // Split function name into parts
            tokens.extend(split_identifier(&entity.name));
            tokens.push(entity.name.to_lowercase());

            // Add file path parts
            for part in entity.file.split('/') {
                let stem = part.strip_suffix(".ts").or(part.strip_suffix(".js"))
                    .or(part.strip_suffix(".py")).or(part.strip_suffix(".rs"))
                    .unwrap_or(part);
                tokens.extend(split_identifier(stem));
            }

            // Tokenize body (first 2000 chars)
            for word in entity.body.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if word.len() >= 3 {
                    tokens.push(word.to_lowercase());
                }
            }

            // Add parameter names
            for param in &entity.params {
                let clean = param.split(':').next().unwrap_or(param).trim();
                tokens.extend(split_identifier(clean));
            }

            // Build term frequency map for this document
            let mut tf_map: HashMap<String, usize> = HashMap::new();
            for token in &tokens {
                *tf_map.entry(token.clone()).or_default() += 1;
            }

            let doc_idx = documents.len();
            let token_count = tokens.len();

            // Add to inverted index
            for (term, count) in &tf_map {
                let tf = *count as f64 / token_count.max(1) as f64;
                inverted_index.entry(term.clone()).or_default().push((doc_idx, tf));
            }

            total_tokens += token_count;

            documents.push(Document {
                name: entity.name.clone(),
                file: entity.file.clone(),
                line: entity.line,
                kind: entity.kind.clone(),
                body: entity.body.chars().take(200).collect(),
                tokens,
                token_count,
            });
        }

        let doc_count = documents.len();
        let avg_doc_len = if doc_count > 0 { total_tokens as f64 / doc_count as f64 } else { 1.0 };

        // Compute embeddings for each document
        let doc_embeddings: Vec<embeddings::Embedding> = documents.iter().map(|doc| {
            // Embed: function name + file path + first 500 chars of body
            let text = format!("{} {} {}", doc.name, doc.file, &doc.body[..doc.body.len().min(500)]);
            embeddings::embed_text(&text)
        }).collect();

        SemanticIndex { documents, inverted_index, embeddings: doc_embeddings, avg_doc_len, doc_count }
    }

    /// Hybrid search: BM25 + embedding similarity + exact name match, merged with RRF.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        if self.documents.is_empty() { return vec![]; }

        let query_lower = query.to_lowercase();
        let query_tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| w.len() >= 2)
            .flat_map(|w| {
                let mut tokens = split_identifier(w);
                tokens.push(w.to_lowercase());
                tokens
            })
            .collect();

        // === Method 1: BM25 ===
        let k1: f64 = 1.2;
        let b: f64 = 0.75;
        let mut bm25_scores: Vec<(usize, f64)> = vec![];

        for (doc_idx, doc) in self.documents.iter().enumerate() {
            let mut score = 0.0;
            for query_term in &query_tokens {
                let postings = match self.inverted_index.get(query_term) {
                    Some(p) => p,
                    None => continue,
                };
                let df = postings.len() as f64;
                let idf = ((self.doc_count as f64 - df + 0.5) / (df + 0.5) + 1.0).ln();
                let tf = postings.iter()
                    .find(|(idx, _)| *idx == doc_idx)
                    .map(|(_, tf)| *tf)
                    .unwrap_or(0.0);
                if tf > 0.0 {
                    let doc_len_norm = doc.token_count as f64 / self.avg_doc_len;
                    let tf_component = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * doc_len_norm));
                    score += idf * tf_component;
                }
            }
            if score > 0.0 { bm25_scores.push((doc_idx, score)); }
        }
        bm25_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // === Method 2: Embedding similarity ===
        let query_embedding = embeddings::embed_text(query);
        let mut embed_scores: Vec<(usize, f32)> = self.embeddings.iter().enumerate()
            .map(|(idx, emb)| (idx, embeddings::cosine_similarity(&query_embedding, emb)))
            .collect();
        embed_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // === Method 3: Exact name match ===
        let mut name_scores: Vec<(usize, f64)> = vec![];
        for (idx, doc) in self.documents.iter().enumerate() {
            let name_lower = doc.name.to_lowercase();
            let name_parts = split_identifier(&doc.name);
            let mut score = 0.0;

            // Exact full name match
            if query_tokens.iter().any(|t| *t == name_lower) { score += 100.0; }
            // Partial name match (query words appear in function name parts)
            let mut name_hits = 0;
            for qt in &query_tokens {
                if name_parts.contains(qt) { score += 30.0; name_hits += 1; }
                // Substring match in function name
                else if name_lower.contains(qt.as_str()) { score += 15.0; name_hits += 1; }
            }
            // Bonus for multiple query terms matching the name
            if name_hits >= 2 { score += 50.0; }
            if name_hits >= 3 { score += 100.0; }

            // File path match (directory + filename are strong signals)
            let file_lower = doc.file.to_lowercase();
            let file_parts: Vec<&str> = file_lower.split('/').collect();
            for qt in &query_tokens {
                // Exact match on a path segment (e.g., "stripe" matches "services/stripe.ts")
                if file_parts.iter().any(|p| p.contains(qt.as_str())) { score += 25.0; }
            }

            // Body keyword match (weaker signal)
            let body_lower = doc.body.to_lowercase();
            for qt in &query_tokens {
                if body_lower.contains(qt.as_str()) { score += 3.0; }
            }

            if score > 0.0 { name_scores.push((idx, score)); }
        }
        name_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // === Reciprocal Rank Fusion ===
        let bm25_ranked: Vec<(String, usize)> = bm25_scores.iter().take(20)
            .enumerate().map(|(rank, (idx, _))| (idx.to_string(), rank)).collect();
        let embed_ranked: Vec<(String, usize)> = embed_scores.iter().take(20)
            .enumerate().map(|(rank, (idx, _))| (idx.to_string(), rank)).collect();
        let name_ranked: Vec<(String, usize)> = name_scores.iter().take(20)
            .enumerate().map(|(rank, (idx, _))| (idx.to_string(), rank)).collect();

        let fused = embeddings::reciprocal_rank_fusion(
            &[bm25_ranked, embed_ranked, name_ranked], 60.0
        );

        // Return top N
        fused.iter().take(limit).map(|(idx_str, score)| {
            let idx: usize = idx_str.parse().unwrap_or(0);
            let doc = &self.documents[idx];
            SearchResult {
                name: doc.name.clone(),
                file: doc.file.clone(),
                line: doc.line,
                kind: doc.kind.clone(),
                score: *score as f64,
                snippet: doc.body.chars().take(200).collect(),
            }
        }).collect()
    }
}

/// Split a camelCase or snake_case identifier into lowercase words.
/// "handlePaymentRetry" -> ["handle", "payment", "retry"]
/// "get_user_by_id" -> ["get", "user", "by", "id"]
fn split_identifier(name: &str) -> Vec<String> {
    let mut words = vec![];
    let mut current = String::new();

    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == '.' {
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

    // Filter out very short words and common noise
    words.retain(|w| w.len() >= 2 && !["is", "to", "in", "on", "by", "of", "or", "an", "as", "at"].contains(&w.as_str()));
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_identifier() {
        assert_eq!(split_identifier("handlePaymentRetry"), vec!["handle", "payment", "retry"]);
        assert_eq!(split_identifier("get_user_by_id"), vec!["get", "user", "id"]);
        assert_eq!(split_identifier("HTMLParser"), vec!["html", "parser"]);
    }

    #[test]
    fn test_search_finds_by_concept() {
        let result = ParseResult {
            repo: "test".to_string(),
            files: 1,
            entities: vec![
                ParsedEntity {
                    kind: "function".to_string(),
                    name: "handleTransactionWithBackoff".to_string(),
                    file: "server/services/payment.ts".to_string(),
                    line: 42, end_line: 80,
                    body: "async function handleTransactionWithBackoff(amount, retryCount) { try { await stripe.charges.create({amount}) } catch(e) { if (retryCount < 3) return handleTransactionWithBackoff(amount, retryCount + 1) } }".to_string(),
                    params: vec!["amount".to_string(), "retryCount".to_string()],
                    import_source: String::new(), import_names: vec![],
                },
                ParsedEntity {
                    kind: "function".to_string(),
                    name: "getUserProfile".to_string(),
                    file: "server/services/user.ts".to_string(),
                    line: 10, end_line: 20,
                    body: "async function getUserProfile(userId) { return db.users.findOne({id: userId}) }".to_string(),
                    params: vec!["userId".to_string()],
                    import_source: String::new(), import_names: vec![],
                },
            ],
            call_sites: vec![],
        };

        let index = SemanticIndex::from_parse_result(&result);

        // Search for "payment retry" should find handleTransactionWithBackoff
        let results = index.search("payment retry logic", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "handleTransactionWithBackoff");
    }
}
