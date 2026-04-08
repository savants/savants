//! Code Property Graph builder — native tree-sitter port of `cpg.py`.

use crate::cpg_walk::walk_node_root;
use crate::schema::{CallSite, ClassNode, FileNode, FunctionNode, StringRef};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, Tree};
use walkdir::WalkDir;

const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "__pycache__",
    "venv",
    ".venv",
    "dist",
    "build",
    ".synapcode",
];

#[derive(Debug, Default, Clone)]
pub struct ParsedFile {
    pub file: Option<FileNode>,
    pub functions: Vec<FunctionNode>,
    pub classes: Vec<ClassNode>,
    pub imports: Vec<String>,
    pub calls: Vec<CallSite>,
    pub string_refs: Vec<StringRef>,
}

#[derive(Debug, Default, Clone)]
pub struct ParseStats {
    pub files: usize,
    pub functions: usize,
    pub classes: usize,
    pub calls: usize,
    pub failed: usize,
    pub config_files: usize,
    pub config_keys: usize,
}

pub struct CodePropertyGraphBuilder {
    repo_path: PathBuf,
}

impl CodePropertyGraphBuilder {
    pub fn new<P: Into<PathBuf>>(repo_path: P) -> Self {
        Self { repo_path: repo_path.into() }
    }

    /// Walk the repo and return all source files we want to index.
    pub fn discover_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&self.repo_path)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !EXCLUDED_DIRS.contains(&name.as_ref())
            })
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "py" | "js" | "ts" | "tsx" | "jsx") {
                    files.push(path.to_path_buf());
                }
            }
        }
        files
    }

    /// Walk the repo and return config files worth indexing
    /// (YAML / TOML / JSON, minus lockfiles and anything > 1 MB).
    pub fn discover_config_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&self.repo_path)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !EXCLUDED_DIRS.contains(&name.as_ref())
            })
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if crate::config::is_indexable_config(entry.path()) {
                files.push(entry.path().to_path_buf());
            }
        }
        files
    }

    /// Parse a single file into structural form.
    pub fn parse_file(&self, file_path: &Path) -> Option<ParsedFile> {
        let ext = file_path.extension()?.to_str()?;
        let language = match ext {
            "py" => "python",
            "js" | "jsx" => "javascript",
            "ts" | "tsx" => "typescript",
            _ => return None,
        };

        let content = std::fs::read_to_string(file_path).ok()?;
        let rel_path = file_path.strip_prefix(&self.repo_path).ok()?
            .to_string_lossy()
            .to_string();

        let mut parser = Parser::new();
        let lang = match language {
            "python" => tree_sitter_python::LANGUAGE.into(),
            "javascript" => tree_sitter_javascript::LANGUAGE.into(),
            "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            _ => return None,
        };
        parser.set_language(&lang).ok()?;

        let tree: Tree = parser.parse(&content, None)?;

        let mut parsed = ParsedFile {
            file: Some(FileNode {
                path: rel_path.clone(),
                language: language.to_string(),
                line_count: content.matches('\n').count() as u32 + 1,
                sha256: sha256_hex(&content),
                last_commit: String::new(),
            }),
            ..Default::default()
        };

        walk_node_root(tree.root_node(), &content, &rel_path, &mut parsed);

        Some(parsed)
    }

    /// Build the full CPG for the repo. Returns aggregated stats.
    pub fn build(&self) -> ParseStats {
        let mut stats = ParseStats::default();
        for path in self.discover_files() {
            match self.parse_file(&path) {
                Some(parsed) => {
                    stats.files += 1;
                    stats.functions += parsed.functions.len();
                    stats.classes += parsed.classes.len();
                    stats.calls += parsed.calls.len();
                }
                None => stats.failed += 1,
            }
        }
        // Config pass: YAML / TOML / JSON -> flat ConfigKey list. Unlike
        // source files, we only count them here; writing to FalkorDB
        // happens on the Python side (or in a future Rust graph writer).
        for path in self.discover_config_files() {
            let rel = path
                .strip_prefix(&self.repo_path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| path.to_string_lossy().to_string());
            let keys = crate::config::parse_config_file(&path, &rel);
            if !keys.is_empty() {
                stats.config_files += 1;
                stats.config_keys += keys.len();
            }
        }
        stats
    }
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_file(name: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        dir
    }

    #[test]
    fn parse_simple_python_function() {
        let dir = write_temp_file("hello.py", "def hello():\n    return 'world'\n");
        let builder = CodePropertyGraphBuilder::new(dir.path());
        let parsed = builder.parse_file(&dir.path().join("hello.py")).unwrap();
        assert_eq!(parsed.functions.len(), 1);
        assert_eq!(parsed.functions[0].name, "hello");
        assert_eq!(parsed.functions[0].start_line, 1);
    }

    #[test]
    fn parse_python_class_with_methods() {
        let dir = write_temp_file(
            "models.py",
            "class User:\n    def login(self):\n        return self.validate()\n\n    def validate(self):\n        return True\n",
        );
        let builder = CodePropertyGraphBuilder::new(dir.path());
        let parsed = builder.parse_file(&dir.path().join("models.py")).unwrap();
        assert_eq!(parsed.classes.len(), 1);
        assert_eq!(parsed.classes[0].name, "User");
        // Both methods should be in functions
        let names: Vec<&str> = parsed.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"login"));
        assert!(names.contains(&"validate"));
    }

    #[test]
    fn extract_call_with_caller_function() {
        let dir = write_temp_file(
            "calls.py",
            "def caller():\n    helper()\n    other.method()\n",
        );
        let builder = CodePropertyGraphBuilder::new(dir.path());
        let parsed = builder.parse_file(&dir.path().join("calls.py")).unwrap();
        let calls_in_caller: Vec<&CallSite> = parsed
            .calls
            .iter()
            .filter(|c| c.caller_function == "caller")
            .collect();
        assert!(calls_in_caller.iter().any(|c| c.callee_name == "helper"));
        assert!(calls_in_caller.iter().any(|c| c.callee_name == "method"));
    }

    #[test]
    fn extract_decorators_on_functions() {
        let dir = write_temp_file(
            "app.py",
            "from functools import lru_cache\n\
             @lru_cache\n\
             def cached():\n    return 42\n\
             \n\
             @app.route('/health')\n\
             def health():\n    return 'ok'\n",
        );
        let builder = CodePropertyGraphBuilder::new(dir.path());
        let parsed = builder.parse_file(&dir.path().join("app.py")).unwrap();
        let by_name: std::collections::HashMap<&str, &FunctionNode> =
            parsed.functions.iter().map(|f| (f.name.as_str(), f)).collect();
        assert!(by_name["cached"].decorators.iter().any(|d| d == "lru_cache"));
        assert!(by_name["health"].decorators.iter().any(|d| d == "app.route"));
    }

    #[test]
    fn extract_string_literal_symbol_refs() {
        let dir = write_temp_file(
            "registry.py",
            "def setup():\n    handlers = {\n        'HandleTsCoinTransfer': None,\n        'utf-8': None,\n        'get': None,\n    }\n    return handlers\n",
        );
        let builder = CodePropertyGraphBuilder::new(dir.path());
        let parsed = builder.parse_file(&dir.path().join("registry.py")).unwrap();
        let values: Vec<&str> = parsed.string_refs.iter().map(|r| r.value.as_str()).collect();
        assert!(values.contains(&"HandleTsCoinTransfer"), "got: {:?}", values);
        assert!(!values.contains(&"utf-8"));
        assert!(!values.contains(&"get"));
        // Must be attributed to the enclosing function
        let r = parsed.string_refs.iter().find(|r| r.value == "HandleTsCoinTransfer").unwrap();
        assert_eq!(r.caller_function, "setup");
    }

    #[test]
    fn discover_files_excludes_node_modules() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/foo")).unwrap();
        std::fs::write(dir.path().join("src/a.py"), "def a(): pass").unwrap();
        std::fs::write(dir.path().join("node_modules/foo/b.py"), "def b(): pass").unwrap();

        let builder = CodePropertyGraphBuilder::new(dir.path());
        let files = builder.discover_files();
        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().ends_with("a.py"));
    }
}
