//! Local embedding persistence.
//!
//! Stores pre-computed embeddings in a binary sidecar file at
//! ~/.savants/embeddings/{repo}.bin
//!
//! Format: simple binary - header + entries
//! Header: magic(4) + version(4) + count(4) + dim(4)
//! Entry: name_len(2) + name(utf8) + file_len(2) + file(utf8) + line(4) + kind(1) + embedding(dim*4)
//!
//! No graph schema, no Cypher, no IP. Just vectors from a public model.

use std::io::{Read, Write, Cursor};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"SVEC";
const VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct StoredEntry {
    pub name: String,
    pub file: String,
    pub line: u32,
    pub kind: u8, // 0=function, 1=class, 2=interface
    pub embedding: Vec<f32>,
}

pub struct EmbeddingStore {
    pub entries: Vec<StoredEntry>,
    pub dim: u32,
}

impl EmbeddingStore {
    pub fn new(dim: u32) -> Self {
        Self { entries: vec![], dim }
    }

    pub fn add(&mut self, name: &str, file: &str, line: u32, kind: u8, embedding: Vec<f32>) {
        self.entries.push(StoredEntry {
            name: name.to_string(),
            file: file.to_string(),
            line,
            kind,
            embedding,
        });
    }

    /// Save to disk at ~/.savants/embeddings/{repo}.bin
    pub fn save(&self, repo: &str) -> Result<(), String> {
        let path = store_path(repo);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        }

        let mut buf: Vec<u8> = Vec::new();

        // Header
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        buf.extend_from_slice(&self.dim.to_le_bytes());

        // Entries
        for entry in &self.entries {
            let name_bytes = entry.name.as_bytes();
            buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(name_bytes);

            let file_bytes = entry.file.as_bytes();
            buf.extend_from_slice(&(file_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(file_bytes);

            buf.extend_from_slice(&entry.line.to_le_bytes());
            buf.push(entry.kind);

            for &val in &entry.embedding {
                buf.extend_from_slice(&val.to_le_bytes());
            }
        }

        std::fs::write(&path, &buf).map_err(|e| format!("write: {}", e))?;
        Ok(())
    }

    /// Load from disk.
    pub fn load(repo: &str) -> Result<Self, String> {
        let path = store_path(repo);
        let data = std::fs::read(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
        let mut cursor = Cursor::new(&data);

        // Header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic).map_err(|_| "bad header")?;
        if &magic != MAGIC { return Err("not a savants embedding file".to_string()); }

        let version = read_u32(&mut cursor)?;
        if version != VERSION { return Err(format!("unsupported version: {}", version)); }

        let count = read_u32(&mut cursor)? as usize;
        let dim = read_u32(&mut cursor)?;

        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let name_len = read_u16(&mut cursor)? as usize;
            let name = read_string(&mut cursor, name_len)?;

            let file_len = read_u16(&mut cursor)? as usize;
            let file = read_string(&mut cursor, file_len)?;

            let line = read_u32(&mut cursor)?;
            let mut kind_byte = [0u8; 1];
            cursor.read_exact(&mut kind_byte).map_err(|_| "bad kind")?;

            let mut embedding = vec![0.0f32; dim as usize];
            for val in embedding.iter_mut() {
                let mut bytes = [0u8; 4];
                cursor.read_exact(&mut bytes).map_err(|_| "bad embedding")?;
                *val = f32::from_le_bytes(bytes);
            }

            entries.push(StoredEntry {
                name, file, line, kind: kind_byte[0], embedding,
            });
        }

        Ok(Self { entries, dim })
    }

    /// Check if embeddings exist for a repo.
    pub fn exists(repo: &str) -> bool {
        store_path(repo).exists()
    }

    /// Search by cosine similarity against a query embedding.
    pub fn search(&self, query: &[f32], limit: usize) -> Vec<(usize, f32)> {
        let mut scores: Vec<(usize, f32)> = self.entries.iter()
            .enumerate()
            .map(|(idx, entry)| (idx, cosine_sim(query, &entry.embedding)))
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(limit);
        scores
    }
}

fn store_path(repo: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".savants")
        .join("embeddings")
        .join(format!("{}.bin", repo))
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let d = na.sqrt() * nb.sqrt();
    if d > 0.0 { dot / d } else { 0.0 }
}

fn read_u32(cursor: &mut Cursor<&Vec<u8>>) -> Result<u32, String> {
    let mut buf = [0u8; 4];
    cursor.read_exact(&mut buf).map_err(|_| "read u32")?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u16(cursor: &mut Cursor<&Vec<u8>>) -> Result<u16, String> {
    let mut buf = [0u8; 2];
    cursor.read_exact(&mut buf).map_err(|_| "read u16")?;
    Ok(u16::from_le_bytes(buf))
}

fn read_string(cursor: &mut Cursor<&Vec<u8>>, len: usize) -> Result<String, String> {
    let mut buf = vec![0u8; len];
    cursor.read_exact(&mut buf).map_err(|_| "read string")?;
    String::from_utf8(buf).map_err(|_| "bad utf8".to_string())
}
