//! Code parser - tree-sitter based source code parser.
//!
//! This module ONLY parses files and extracts structured metadata.
//! It does NOT touch any database, graph, or network resource.
//! The output is serializable JSON that can be sent to the cloud API
//! for graph construction.
//!
//! This is the module that ships in the binary. The graph construction
//! (Cypher queries, edge resolution, schema) stays server-side.

use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::Path;
use walkdir::WalkDir;

/// Parsed entity from source code - serializable, no graph references.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ParsedEntity {
    pub kind: String,       // "function", "class", "interface", "import"
    pub name: String,
    pub file: String,       // relative path
    pub line: usize,
    pub end_line: usize,
    pub body: String,       // first 2000 chars
    pub params: Vec<String>,
    pub import_source: String,
    pub import_names: Vec<String>,
}

/// Result of parsing a repository - serializable JSON.
#[derive(Debug, Serialize, Deserialize)]
pub struct ParseResult {
    pub repo: String,
    pub files: usize,
    pub entities: Vec<ParsedEntity>,
    pub call_sites: Vec<CallSite>,
}

/// A call site: function A calls function B.
#[derive(Debug, Serialize, Deserialize)]
pub struct CallSite {
    pub caller_file: String,
    pub caller_name: String,
    pub callee_name: String,
}

pub struct CodeParser {
    repo_name: String,
    workspace_map: HashMap<String, String>,
}

impl CodeParser {
    pub fn new(repo_name: &str) -> Self {
        Self { repo_name: repo_name.to_string(), workspace_map: HashMap::new() }
    }

    /// Parse a repository and return structured metadata as serializable JSON.
    /// No database, no graph, no network calls.
    pub fn parse_repo(&mut self, repo_path: &str) -> ParseResult {
        self.workspace_map = Self::build_workspace_map(repo_path);
        let mut entities = vec![];
        let mut call_sites = vec![];
        let mut file_count = 0;

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

            let parsed = match ext {
                "ts" | "tsx" => self.parse_file(path, repo_path, tree_sitter_typescript::language_tsx()),
                "js" | "jsx" | "mjs" | "cjs" => self.parse_file(path, repo_path, tree_sitter_javascript::language()),
                "py" | "pyi" => self.parse_file(path, repo_path, tree_sitter_python::language()),
                "rs" => self.parse_file(path, repo_path, tree_sitter_rust::language()),
                "go" => self.parse_file(path, repo_path, tree_sitter_go::language()),
                "java" => self.parse_file(path, repo_path, tree_sitter_java::language()),
                "c" | "h" => self.parse_file(path, repo_path, tree_sitter_c::language()),
                "cpp" | "cc" | "cxx" | "hpp" | "hxx" => self.parse_file(path, repo_path, tree_sitter_cpp::language()),
                "rb" => self.parse_file(path, repo_path, tree_sitter_ruby::language()),
                "cs" => self.parse_file(path, repo_path, tree_sitter_c_sharp::language()),
                _ => continue,
            };

            if let Some(file_entities) = parsed {
                // Extract call sites from function bodies using FULL source text.
                // We read the file again to get the full body (entities have truncated bodies).
                let full_source = std::fs::read_to_string(entry.path()).unwrap_or_default();
                let call_re = regex::Regex::new(r"(\w+)\s*\(").unwrap();
                let skip_keywords = ["if", "for", "while", "return", "switch", "catch", "new",
                    "typeof", "await", "import", "require", "console", "Math",
                    "let", "mut", "pub", "fn", "mod", "use", "impl", "struct",
                    "enum", "match", "Some", "None", "Ok", "Err", "println", "eprintln",
                    "format", "vec", "assert", "assert_eq", "panic", "todo", "unimplemented",
                    "include_str", "cfg", "derive", "test", "allow", "warn", "deny"];

                for e in &file_entities {
                    if e.kind == "function" {
                        // Find the full body from source using line numbers
                        let start_line = e.line.saturating_sub(1);
                        let end_line = e.end_line;
                        let full_body: String = full_source.lines()
                            .skip(start_line)
                            .take(end_line - start_line)
                            .collect::<Vec<&str>>()
                            .join("\n");

                        for cap in call_re.captures_iter(&full_body) {
                            let called = &cap[1];
                            if called.len() > 1
                                && !skip_keywords.contains(&called)
                                && called != e.name
                                && called.chars().next().map(|c| c.is_lowercase()).unwrap_or(false)
                            {
                                call_sites.push(CallSite {
                                    caller_file: e.file.clone(),
                                    caller_name: e.name.clone(),
                                    callee_name: called.to_string(),
                                });
                            }
                        }
                    }
                }

                entities.extend(file_entities);
                file_count += 1;
            }
        }

        ParseResult {
            repo: self.repo_name.clone(),
            files: file_count,
            entities,
            call_sites,
        }
    }

    fn parse_file(&self, path: &Path, repo_root: &str, language: tree_sitter::Language) -> Option<Vec<ParsedEntity>> {
        let source = std::fs::read_to_string(path).ok()?;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&language).ok()?;
        let tree = parser.parse(&source, None)?;

        let rel_path = path.strip_prefix(repo_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        let mut entities = vec![];
        let mut cursor = tree.root_node().walk();
        self.walk_tree(&mut cursor, source.as_bytes(), &rel_path, &mut entities, 0);
        Some(entities)
    }

    fn walk_tree(
        &self,
        cursor: &mut tree_sitter::TreeCursor,
        source: &[u8],
        file: &str,
        entities: &mut Vec<ParsedEntity>,
        depth: usize,
    ) {
        if depth > 20 { return; }

        let node = cursor.node();
        let kind = node.kind();

        match kind {
            // Functions across all languages
            "function_declaration" | "function_definition" | "method_definition" | "function_item"
            // Go
            | "func_literal"
            // Java/C#
            | "method_declaration" | "constructor_declaration"
            // Ruby
            | "method" | "singleton_method"
            // C/C++
            | "preproc_function_def" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source).unwrap_or("").to_string();
                    let body_text = node.utf8_text(source).unwrap_or("");
                    let body: String = body_text.chars().take(2000).collect();
                    let params = Self::extract_params(&node, source);
                    entities.push(ParsedEntity {
                        kind: "function".to_string(), name, file: file.to_string(),
                        line: node.start_position().row + 1, end_line: node.end_position().row + 1,
                        body, params, import_source: String::new(), import_names: vec![],
                    });
                }
            }
            "arrow_function" | "function_expression" => {
                // Check for variable assignment: const foo = () => {}
                if let Some(parent) = node.parent() {
                    if parent.kind() == "variable_declarator" || parent.kind() == "pair" {
                        if let Some(name_node) = parent.child_by_field_name("name").or_else(|| parent.child_by_field_name("key")) {
                            let name = name_node.utf8_text(source).unwrap_or("").to_string();
                            if !name.is_empty() {
                                let body_text = node.utf8_text(source).unwrap_or("");
                                let body: String = body_text.chars().take(2000).collect();
                                let params = Self::extract_params(&node, source);
                                entities.push(ParsedEntity {
                                    kind: "function".to_string(), name, file: file.to_string(),
                                    line: node.start_position().row + 1, end_line: node.end_position().row + 1,
                                    body, params, import_source: String::new(), import_names: vec![],
                                });
                            }
                        }
                    }
                }
            }
            // Classes/structs across all languages
            "class_declaration" | "class_definition" | "struct_item" | "enum_item"
            // Go
            | "type_declaration"
            // Java/C#
            | "enum_declaration" | "record_declaration"
            // Ruby
            | "class" | "module" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source).unwrap_or("").to_string();
                    entities.push(ParsedEntity {
                        kind: "class".to_string(), name, file: file.to_string(),
                        line: node.start_position().row + 1, end_line: node.end_position().row + 1,
                        body: String::new(), params: vec![], import_source: String::new(), import_names: vec![],
                    });
                }
            }
            // Interfaces/traits/types across all languages
            "interface_declaration" | "type_alias_declaration" | "trait_item" | "type_item"
            // Java/C#
            | "annotation_type_declaration"
            // Go
            | "type_spec" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source).unwrap_or("").to_string();
                    entities.push(ParsedEntity {
                        kind: "interface".to_string(), name, file: file.to_string(),
                        line: node.start_position().row + 1, end_line: node.end_position().row + 1,
                        body: String::new(), params: vec![], import_source: String::new(), import_names: vec![],
                    });
                }
            }
            // Imports across all languages
            "import_statement" | "use_declaration"
            // Go / Java / C#
            | "import_declaration" | "import_spec"
            // C/C++
            | "preproc_include" => {
                let text = node.utf8_text(source).unwrap_or("");
                let (source_mod, names) = Self::parse_import(text);
                if !source_mod.is_empty() {
                    entities.push(ParsedEntity {
                        kind: "import".to_string(), name: String::new(), file: file.to_string(),
                        line: node.start_position().row + 1, end_line: node.end_position().row + 1,
                        body: String::new(), params: vec![],
                        import_source: source_mod, import_names: names,
                    });
                }
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

    fn extract_params(node: &tree_sitter::Node, source: &[u8]) -> Vec<String> {
        let mut params = vec![];
        if let Some(params_node) = node.child_by_field_name("parameters") {
            let text = params_node.utf8_text(source).unwrap_or("()");
            let inner = text.trim_start_matches('(').trim_end_matches(')');
            if !inner.is_empty() {
                params = inner.split(',').map(|p| p.trim().to_string()).collect();
            }
        }
        params
    }

    fn parse_import(text: &str) -> (String, Vec<String>) {
        // import { foo, bar } from './module'
        let from_re = regex::Regex::new(r#"from\s+['"]([^'"]+)['"]"#).unwrap();
        let names_re = regex::Regex::new(r#"\{([^}]+)\}"#).unwrap();

        let source = from_re.captures(text)
            .map(|c| c[1].to_string())
            .unwrap_or_default();

        let names = names_re.captures(text)
            .map(|c| c[1].split(',').map(|n| {
                let n = n.trim();
                // Handle "foo as bar" → "foo"
                n.split(" as ").next().unwrap_or(n).trim().to_string()
            }).filter(|n| !n.is_empty()).collect())
            .unwrap_or_default();

        (source, names)
    }

    fn build_workspace_map(repo_path: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        // Walk looking for package.json files to build workspace map
        for entry in WalkDir::new(repo_path).max_depth(4).into_iter().filter_map(|e| e.ok()) {
            if entry.file_name() == "package.json" && !entry.path().to_string_lossy().contains("node_modules") {
                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                    if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(name) = pkg.get("name").and_then(|n| n.as_str()) {
                            let dir = entry.path().parent()
                                .unwrap_or(Path::new("."))
                                .strip_prefix(repo_path)
                                .unwrap_or(Path::new("."))
                                .to_string_lossy()
                                .to_string();
                            map.insert(name.to_string(), dir);
                        }
                    }
                }
            }
        }
        map
    }
}
