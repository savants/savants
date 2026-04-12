//! Code indexer — uses tree-sitter to parse source files and build a code property graph.
//!
//! Extracts: functions, classes, methods, imports, call sites, string literals.
//! Stores in FalkorDB as CodeFunction, CodeClass, CodeFile nodes with edges.
//! Supports: TypeScript, JavaScript, Python, Rust.

use crate::graph::GraphClient;
use std::collections::HashMap;
use std::path::Path;
use walkdir::WalkDir;

pub struct CodeIndexer {
    graph: GraphClient,
    repo_name: String,
}

#[derive(Debug)]
struct CodeEntity {
    kind: String,       // "function", "class", "method", "arrow_function"
    name: String,
    file: String,
    line: usize,
    end_line: usize,
    body: String,       // first 500 chars of body for search
    params: Vec<String>,
}

impl CodeIndexer {
    pub fn new(graph: GraphClient, repo_name: &str) -> Self {
        Self { graph, repo_name: repo_name.to_string() }
    }

    /// Index an entire repository.
    pub fn index_repo(&self, repo_path: &str) -> IndexStats {
        let mut stats = IndexStats::default();

        let skip_dirs = [
            "node_modules", ".git", "dist", "build", ".next", "target",
            "__pycache__", ".venv", "venv", "coverage", ".turbo",
        ];

        for entry in WalkDir::new(repo_path)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !skip_dirs.iter().any(|d| name == *d)
            })
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() { continue; }

            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            match ext {
                "ts" | "tsx" => {
                    if let Ok(source) = std::fs::read_to_string(path) {
                        let entities = self.parse_typescript(&source, path, repo_path);
                        self.ingest_entities(&entities, &mut stats);
                    }
                }
                "js" | "jsx" => {
                    if let Ok(source) = std::fs::read_to_string(path) {
                        let entities = self.parse_javascript(&source, path, repo_path);
                        self.ingest_entities(&entities, &mut stats);
                    }
                }
                "py" => {
                    if let Ok(source) = std::fs::read_to_string(path) {
                        let entities = self.parse_python(&source, path, repo_path);
                        self.ingest_entities(&entities, &mut stats);
                    }
                }
                _ => continue,
            }

            stats.files += 1;
        }

        // Create repo node
        let _ = self.graph.query(
            &format!("MERGE (r:CodeRepo {{name: '{}'}}) SET r.path = '{}', r.files = {}, r.functions = {}, r.classes = {}",
                esc(&self.repo_name), esc(repo_path), stats.files, stats.functions, stats.classes),
            &[],
        );

        stats
    }

    fn parse_typescript(&self, source: &str, path: &Path, repo_root: &str) -> Vec<CodeEntity> {
        let mut parser = tree_sitter::Parser::new();
        let lang = if path.extension().map(|e| e == "tsx").unwrap_or(false) {
            tree_sitter_typescript::language_tsx()
        } else {
            tree_sitter_typescript::language_typescript()
        };
        parser.set_language(&lang).unwrap();
        self.extract_entities(&mut parser, source, path, repo_root)
    }

    fn parse_javascript(&self, source: &str, path: &Path, repo_root: &str) -> Vec<CodeEntity> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_javascript::language()).unwrap();
        self.extract_entities(&mut parser, source, path, repo_root)
    }

    fn parse_python(&self, source: &str, path: &Path, repo_root: &str) -> Vec<CodeEntity> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_python::language()).unwrap();
        self.extract_entities(&mut parser, source, path, repo_root)
    }

    fn extract_entities(&self, parser: &mut tree_sitter::Parser, source: &str, path: &Path, repo_root: &str) -> Vec<CodeEntity> {
        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return vec![],
        };

        let rel_path = path.strip_prefix(repo_root).unwrap_or(path);
        let file_str = rel_path.to_string_lossy().to_string();
        let source_bytes = source.as_bytes();

        let mut entities = vec![];
        let mut cursor = tree.walk();

        self.walk_tree(&mut cursor, source_bytes, &file_str, &mut entities, 0);

        entities
    }

    fn walk_tree(
        &self,
        cursor: &mut tree_sitter::TreeCursor,
        source: &[u8],
        file: &str,
        entities: &mut Vec<CodeEntity>,
        depth: usize,
    ) {
        if depth > 20 { return; } // prevent infinite recursion

        let node = cursor.node();
        let kind = node.kind();

        match kind {
            // Functions
            "function_declaration" | "function_definition" | "method_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source).unwrap_or("").to_string();
                    let body_text = node.utf8_text(source).unwrap_or("");
                    let body: String = body_text.chars().take(500).collect();
                    let params = self.extract_params(&node, source);

                    entities.push(CodeEntity {
                        kind: "function".to_string(),
                        name,
                        file: file.to_string(),
                        line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        body,
                        params,
                    });
                }
            }

            // Arrow functions assigned to const/let/var
            "lexical_declaration" | "variable_declaration" => {
                // Look for: const foo = (...) => { ... }
                if let Some(declarator) = node.named_child(0) {
                    if let Some(name_node) = declarator.child_by_field_name("name") {
                        if let Some(value_node) = declarator.child_by_field_name("value") {
                            if value_node.kind() == "arrow_function" {
                                let name = name_node.utf8_text(source).unwrap_or("").to_string();
                                let body_text = value_node.utf8_text(source).unwrap_or("");
                                let body: String = body_text.chars().take(500).collect();
                                let params = self.extract_params(&value_node, source);

                                entities.push(CodeEntity {
                                    kind: "function".to_string(),
                                    name,
                                    file: file.to_string(),
                                    line: node.start_position().row + 1,
                                    end_line: value_node.end_position().row + 1,
                                    body,
                                    params,
                                });
                            }
                        }
                    }
                }
            }

            // Classes
            "class_declaration" | "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source).unwrap_or("").to_string();

                    entities.push(CodeEntity {
                        kind: "class".to_string(),
                        name,
                        file: file.to_string(),
                        line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        body: String::new(),
                        params: vec![],
                    });
                }
            }

            // Export statements — extract the exported name
            "export_statement" => {
                // Walk children to find the declaration inside
            }

            _ => {}
        }

        // Recurse into children
        if cursor.goto_first_child() {
            loop {
                self.walk_tree(cursor, source, file, entities, depth + 1);
                if !cursor.goto_next_sibling() { break; }
            }
            cursor.goto_parent();
        }
    }

    fn extract_params(&self, node: &tree_sitter::Node, source: &[u8]) -> Vec<String> {
        let mut params = vec![];
        if let Some(params_node) = node.child_by_field_name("parameters") {
            for i in 0..params_node.named_child_count() {
                if let Some(param) = params_node.named_child(i) {
                    let text = param.utf8_text(source).unwrap_or("").to_string();
                    if !text.is_empty() {
                        params.push(text);
                    }
                }
            }
        }
        params
    }

    fn ingest_entities(&self, entities: &[CodeEntity], stats: &mut IndexStats) {
        for e in entities {
            match e.kind.as_str() {
                "function" => {
                    stats.functions += 1;
                    let _ = self.graph.query(
                        &format!(
                            "MERGE (f:CodeFunction {{repo: '{}', file: '{}', name: '{}'}}) \
                             SET f.line = {}, f.end_line = {}, f.body = '{}', f.params = '{}'",
                            esc(&self.repo_name), esc(&e.file), esc(&e.name),
                            e.line, e.end_line,
                            esc(&e.body),
                            esc(&e.params.join(", ")),
                        ),
                        &[],
                    );

                    // File → contains function edge
                    let _ = self.graph.query(
                        &format!(
                            "MERGE (fi:CodeFile {{repo: '{}', path: '{}'}}) \
                             MERGE (f:CodeFunction {{repo: '{}', file: '{}', name: '{}'}}) \
                             MERGE (fi)-[:CONTAINS]->(f)",
                            esc(&self.repo_name), esc(&e.file),
                            esc(&self.repo_name), esc(&e.file), esc(&e.name),
                        ),
                        &[],
                    );

                    // Extract call sites from body — find function names called
                    let call_re = regex::Regex::new(r"(\w+)\s*\(").unwrap();
                    for cap in call_re.captures_iter(&e.body) {
                        let called = &cap[1];
                        // Skip common keywords
                        if ["if", "for", "while", "return", "switch", "catch", "new", "typeof", "await", "import", "require", "console", "Math"].contains(&called) {
                            continue;
                        }
                        let _ = self.graph.query(
                            &format!(
                                "MATCH (caller:CodeFunction {{repo: '{}', file: '{}', name: '{}'}}) \
                                 MATCH (callee:CodeFunction {{repo: '{}', name: '{}'}}) \
                                 WHERE caller <> callee \
                                 MERGE (caller)-[:CALLS]->(callee)",
                                esc(&self.repo_name), esc(&e.file), esc(&e.name),
                                esc(&self.repo_name), esc(called),
                            ),
                            &[],
                        );
                    }
                }
                "class" => {
                    stats.classes += 1;
                    let _ = self.graph.query(
                        &format!(
                            "MERGE (c:CodeClass {{repo: '{}', file: '{}', name: '{}'}}) \
                             SET c.line = {}, c.end_line = {}",
                            esc(&self.repo_name), esc(&e.file), esc(&e.name),
                            e.line, e.end_line,
                        ),
                        &[],
                    );
                }
                _ => {}
            }
        }
    }
}

#[derive(Default)]
pub struct IndexStats {
    pub files: usize,
    pub functions: usize,
    pub classes: usize,
}

impl IndexStats {
    pub fn summary(&self) -> String {
        format!("{} files, {} functions, {} classes", self.files, self.functions, self.classes)
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n").replace('\r', "")
}
