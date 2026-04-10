//! Delta protocol — wire format compatible with `src/savants/delta/schema.py`.

use crate::cpg::ParsedFile;
use crate::schema::canonical_node_id;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const PROTOCOL_VERSION: &str = "0.1";
pub const SCHEMA_ID: &str = "savants/delta/v0.1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaScope {
    pub org: String,
    pub repo: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
}

fn default_branch() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    AddNode {
        id: String,
        label: String,
        #[serde(default)]
        properties: HashMap<String, serde_json::Value>,
    },
    RemoveNode {
        id: String,
    },
    UpdateNode {
        id: String,
        #[serde(default)]
        set: HashMap<String, serde_json::Value>,
        #[serde(default)]
        unset: Vec<String>,
    },
    AddEdge {
        id: String,
        #[serde(rename = "type")]
        edge_type: String,
        from_id: String,
        to_id: String,
        #[serde(default)]
        properties: HashMap<String, serde_json::Value>,
    },
    RemoveEdge {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default = "default_schema_id")]
    pub schema_id: String,
    pub scope: DeltaScope,
    #[serde(default)]
    pub operations: Vec<Operation>,
}

fn default_version() -> String {
    PROTOCOL_VERSION.to_string()
}

fn default_schema_id() -> String {
    SCHEMA_ID.to_string()
}

impl Delta {
    pub fn new(scope: DeltaScope) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_string(),
            schema_id: SCHEMA_ID.to_string(),
            scope,
            operations: Vec::new(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Compute a Delta from a single file's before/after content. Mirrors the
/// Python `compute_file_delta` function exactly.
pub fn compute_file_delta(
    file_path: &str,
    before_content: Option<&str>,
    after_content: Option<&str>,
    scope: DeltaScope,
) -> Delta {
    let mut delta = Delta::new(scope);

    let before_parsed = before_content.and_then(|c| parse_in_memory(file_path, c));
    let after_parsed = after_content.and_then(|c| parse_in_memory(file_path, c));

    match (before_parsed, after_parsed) {
        (None, Some(after)) => emit_added_file(&mut delta, &after),
        (Some(_), None) => emit_removed_file(&mut delta, file_path),
        (Some(before), Some(after)) => emit_modified_file(&mut delta, &before, &after),
        (None, None) => {}
    }

    delta
}

/// Parse a single in-memory file (no FS access). Used by the delta computer.
fn parse_in_memory(file_path: &str, content: &str) -> Option<ParsedFile> {
    use tree_sitter::Parser;

    let ext = Path::new(file_path).extension()?.to_str()?;
    let language = match ext {
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        _ => return None,
    };

    let mut parser = Parser::new();
    let lang = match language {
        "python" => tree_sitter_python::LANGUAGE.into(),
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        _ => return None,
    };
    parser.set_language(&lang).ok()?;

    let tree = parser.parse(content, None)?;

    let mut parsed = ParsedFile::default();
    parsed.file = Some(crate::schema::FileNode {
        path: file_path.to_string(),
        language: language.to_string(),
        line_count: content.matches('\n').count() as u32 + 1,
        sha256: {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(content.as_bytes());
            hex::encode(h.finalize())
        },
        last_commit: String::new(),
    });

    crate::cpg_walk::walk_node_root(tree.root_node(), content, file_path, &mut parsed);

    Some(parsed)
}

fn emit_added_file(delta: &mut Delta, parsed: &ParsedFile) {
    if let Some(file_node) = &parsed.file {
        let id = canonical_node_id("File", Some(&file_node.path), None);
        let mut props = HashMap::new();
        props.insert("file_path".into(), serde_json::Value::String(file_node.path.clone()));
        props.insert("language".into(), serde_json::Value::String(file_node.language.clone()));
        props.insert("line_count".into(), serde_json::json!(file_node.line_count));
        props.insert("sha256".into(), serde_json::Value::String(file_node.sha256.clone()));
        delta.operations.push(Operation::AddNode {
            id,
            label: "File".to_string(),
            properties: props,
        });

        let file_id = canonical_node_id("File", Some(&file_node.path), None);
        for fn_node in &parsed.functions {
            let fn_id = canonical_node_id("Function", Some(&fn_node.file_path), Some(&fn_node.name));
            let mut props = HashMap::new();
            props.insert("name".into(), serde_json::Value::String(fn_node.name.clone()));
            props.insert("file_path".into(), serde_json::Value::String(fn_node.file_path.clone()));
            props.insert("start_line".into(), serde_json::json!(fn_node.start_line));
            props.insert("end_line".into(), serde_json::json!(fn_node.end_line));
            props.insert(
                "parameters".into(),
                serde_json::Value::Array(
                    fn_node.parameters.iter().map(|p| serde_json::Value::String(p.clone())).collect(),
                ),
            );
            props.insert(
                "decorators".into(),
                serde_json::Value::Array(
                    fn_node.decorators.iter().map(|d| serde_json::Value::String(d.clone())).collect(),
                ),
            );
            props.insert(
                "docstring".into(),
                serde_json::Value::String(fn_node.docstring.clone()),
            );
            props.insert(
                "class_name".into(),
                serde_json::Value::String(fn_node.class_name.clone()),
            );
            delta.operations.push(Operation::AddNode {
                id: fn_id.clone(),
                label: "Function".to_string(),
                properties: props,
            });

            let edge_id = format!("edge:{}\u{2192}{}:CONTAINS", file_id, fn_id);
            delta.operations.push(Operation::AddEdge {
                id: edge_id,
                edge_type: "CONTAINS".to_string(),
                from_id: file_id.clone(),
                to_id: fn_id,
                properties: HashMap::new(),
            });
        }
    }
}

fn emit_removed_file(delta: &mut Delta, file_path: &str) {
    delta.operations.push(Operation::RemoveNode {
        id: canonical_node_id("File", Some(file_path), None),
    });
}

fn emit_modified_file(delta: &mut Delta, before: &ParsedFile, after: &ParsedFile) {
    use std::collections::HashSet;

    let before_fns: HashSet<&str> = before.functions.iter().map(|f| f.name.as_str()).collect();
    let after_fns: HashSet<&str> = after.functions.iter().map(|f| f.name.as_str()).collect();
    let file_path = after
        .file
        .as_ref()
        .map(|f| f.path.as_str())
        .unwrap_or("");

    // Removed
    for name in before_fns.difference(&after_fns) {
        delta.operations.push(Operation::RemoveNode {
            id: canonical_node_id("Function", Some(file_path), Some(name)),
        });
    }
    // Added
    for fn_node in &after.functions {
        if !before_fns.contains(fn_node.name.as_str()) {
            let id = canonical_node_id("Function", Some(&fn_node.file_path), Some(&fn_node.name));
            let mut props = HashMap::new();
            props.insert("name".into(), serde_json::Value::String(fn_node.name.clone()));
            props.insert("file_path".into(), serde_json::Value::String(fn_node.file_path.clone()));
            props.insert("start_line".into(), serde_json::json!(fn_node.start_line));
            props.insert("end_line".into(), serde_json::json!(fn_node.end_line));
            delta.operations.push(Operation::AddNode {
                id,
                label: "Function".to_string(),
                properties: props,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> DeltaScope {
        DeltaScope {
            org: "test".into(),
            repo: "r".into(),
            branch: "main".into(),
            base_sha: None,
            head_sha: None,
        }
    }

    #[test]
    fn delta_for_added_file_emits_operations() {
        let delta = compute_file_delta(
            "src/main.py",
            None,
            Some("def hello():\n    pass\n"),
            scope(),
        );
        assert!(!delta.operations.is_empty());
        let has_function_add = delta.operations.iter().any(|op| {
            matches!(op, Operation::AddNode { label, .. } if label == "Function")
        });
        assert!(has_function_add);
    }

    #[test]
    fn delta_for_removed_file_emits_remove() {
        let delta = compute_file_delta("src/old.py", Some("def x(): pass"), None, scope());
        assert!(matches!(delta.operations.first(), Some(Operation::RemoveNode { .. })));
    }

    #[test]
    fn delta_round_trips_through_json() {
        let delta = compute_file_delta(
            "src/main.py",
            None,
            Some("def hello():\n    pass\n"),
            scope(),
        );
        let json = delta.to_json();
        let parsed = Delta::from_json(&json).expect("roundtrip");
        assert_eq!(parsed.scope.org, "test");
        assert_eq!(parsed.operations.len(), delta.operations.len());
    }
}
