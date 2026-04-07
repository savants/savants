//! Schema mirrors `src/synapcode/graph/schema.py` and the delta protocol IDs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileNode {
    pub path: String,
    pub language: String,
    pub line_count: u32,
    pub sha256: String,
    #[serde(default)]
    pub last_commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionNode {
    pub name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(default)]
    pub return_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassNode {
    pub name: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(default)]
    pub bases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallSite {
    pub caller_file: String,
    pub caller_function: String,
    pub callee_name: String,
    pub line: u32,
}

/// Compute a deterministic node ID matching the Python `canonical_node_id`.
pub fn canonical_node_id(label: &str, file_path: Option<&str>, name: Option<&str>) -> String {
    let short = match label {
        "File" => "f",
        "Function" => "fn",
        "Class" => "c",
        "Module" => "m",
        "Variable" => "v",
        "Episode" => "ep",
        "Entity" => "e",
        _ => label,
    };
    let mut parts = vec![short.to_string()];
    if let Some(fp) = file_path {
        parts.push(fp.to_string());
    }
    if let Some(n) = name {
        parts.push(n.to_string());
    }
    parts.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_id_file() {
        assert_eq!(canonical_node_id("File", Some("src/main.py"), None), "f:src/main.py");
    }

    #[test]
    fn canonical_id_function() {
        assert_eq!(
            canonical_node_id("Function", Some("src/main.py"), Some("process")),
            "fn:src/main.py:process"
        );
    }

    #[test]
    fn canonical_id_class() {
        assert_eq!(
            canonical_node_id("Class", Some("src/models.py"), Some("User")),
            "c:src/models.py:User"
        );
    }
}
