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
    /// Maps workspace package names (e.g., "@prisma/sdk") to their repo-relative directory paths
    workspace_map: HashMap<String, String>,
}

#[derive(Debug)]
struct CodeEntity {
    kind: String,       // "function", "class", "method", "arrow_function", "import"
    name: String,
    file: String,
    line: usize,
    end_line: usize,
    body: String,       // first 500 chars of body for search
    params: Vec<String>,
    /// For imports: the source module path (e.g., "../utils/llm-validation")
    import_source: String,
    /// For imports: the names imported (e.g., ["validateLLMOutput", "LLMParseError"])
    import_names: Vec<String>,
}

impl CodeIndexer {
    pub fn new(graph: GraphClient, repo_name: &str) -> Self {
        Self { graph, repo_name: repo_name.to_string(), workspace_map: HashMap::new() }
    }

    /// Index an entire repository.
    pub fn index_repo(&mut self, repo_path: &str) -> IndexStats {
        // Build workspace map from package.json files before indexing
        self.workspace_map = Self::build_workspace_map(repo_path);
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

        // Resolve cross-file CALLS edges using import data
        self.resolve_cross_file_calls();

        // Index git history
        let git_indexer = GitHistoryIndexer::new(self.graph.clone(), &self.repo_name);
        let (commit_count, change_count) = git_indexer.index(repo_path, 500);
        stats.commits = commit_count;
        stats.file_changes = change_count;

        // Create repo node
        let _ = self.graph.query(
            &format!("MERGE (r:CodeRepo {{name: '{}'}}) SET r.path = '{}', r.files = {}, r.functions = {}, r.classes = {}, r.commits = {}",
                esc(&self.repo_name), esc(repo_path), stats.files, stats.functions, stats.classes, stats.commits),
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
                    let body: String = body_text.chars().take(2000).collect();
                    let params = self.extract_params(&node, source);

                    entities.push(CodeEntity {
                        kind: "function".to_string(),
                        name,
                        file: file.to_string(),
                        line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        body,
                        params,
                        import_source: String::new(),
                        import_names: vec![],
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
                                let body: String = body_text.chars().take(2000).collect();
                                let params = self.extract_params(&value_node, source);

                                entities.push(CodeEntity {
                                    kind: "function".to_string(),
                                    name,
                                    file: file.to_string(),
                                    line: node.start_position().row + 1,
                                    end_line: value_node.end_position().row + 1,
                                    body,
                                    params,
                                    import_source: String::new(),
                                    import_names: vec![],
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
                        import_source: String::new(),
                        import_names: vec![],
                    });
                }
            }

            // TypeScript interfaces and type aliases
            "interface_declaration" | "type_alias_declaration" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node.utf8_text(source).unwrap_or("").to_string();
                    let body_text = node.utf8_text(source).unwrap_or("");
                    let body: String = body_text.chars().take(2000).collect();

                    entities.push(CodeEntity {
                        kind: "interface".to_string(),
                        name,
                        file: file.to_string(),
                        line: node.start_position().row + 1,
                        end_line: node.end_position().row + 1,
                        body,
                        params: vec![],
                        import_source: String::new(),
                        import_names: vec![],
                    });
                }
            }

            // Export statements — extract the exported name
            "export_statement" => {
                // Walk children to find the declaration inside
            }

            // Import statements — track what's imported from where
            "import_statement" => {
                let full_text = node.utf8_text(source).unwrap_or("").to_string();
                // Extract source: import { foo } from './bar'
                if let Some(source_node) = node.child_by_field_name("source") {
                    let import_path = source_node.utf8_text(source).unwrap_or("")
                        .trim_matches(|c| c == '\'' || c == '"')
                        .to_string();
                    // Extract imported names
                    let mut names = vec![];
                    let re = regex::Regex::new(r"(?:import\s+\{([^}]+)\}|import\s+(\w+))").unwrap();
                    if let Some(caps) = re.captures(&full_text) {
                        if let Some(named) = caps.get(1) {
                            for n in named.as_str().split(',') {
                                let n = n.trim().split(" as ").next().unwrap_or("").trim();
                                if !n.is_empty() {
                                    names.push(n.to_string());
                                }
                            }
                        }
                        if let Some(default) = caps.get(2) {
                            names.push(default.as_str().to_string());
                        }
                    }
                    if !names.is_empty() {
                        entities.push(CodeEntity {
                            kind: "import".to_string(),
                            name: names.join(", "),
                            file: file.to_string(),
                            line: node.start_position().row + 1,
                            end_line: node.end_position().row + 1,
                            body: String::new(),
                            params: vec![],
                            import_source: import_path,
                            import_names: names,
                        });
                    }
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
                "interface" => {
                    stats.classes += 1; // count interfaces with classes
                    let _ = self.graph.query(
                        &format!(
                            "MERGE (i:CodeInterface {{repo: '{}', file: '{}', name: '{}'}}) \
                             SET i.line = {}, i.end_line = {}, i.body = '{}'",
                            esc(&self.repo_name), esc(&e.file), esc(&e.name),
                            e.line, e.end_line, esc(&e.body),
                        ),
                        &[],
                    );

                    // File contains interface edge
                    let _ = self.graph.query(
                        &format!(
                            "MERGE (fi:CodeFile {{repo: '{}', path: '{}'}}) \
                             MERGE (i:CodeInterface {{repo: '{}', file: '{}', name: '{}'}}) \
                             MERGE (fi)-[:CONTAINS]->(i)",
                            esc(&self.repo_name), esc(&e.file),
                            esc(&self.repo_name), esc(&e.file), esc(&e.name),
                        ),
                        &[],
                    );
                }
                "import" => {
                    // Resolve import path to actual file
                    // e.g., "../utils/llm-validation" → "server/utils/llm-validation.ts"
                    let resolved = self.resolve_import_path(&e.file, &e.import_source);

                    // Create IMPORTS edges: importing file → imported function
                    for name in &e.import_names {
                        let _ = self.graph.query(
                            &format!(
                                "MATCH (importer:CodeFile {{repo: '{}', path: '{}'}}) \
                                 MATCH (fn:CodeFunction {{repo: '{}', name: '{}'}}) \
                                 WHERE fn.file STARTS WITH '{}' \
                                 MERGE (importer)-[:IMPORTS]->(fn)",
                                esc(&self.repo_name), esc(&e.file),
                                esc(&self.repo_name), esc(name),
                                esc(&resolved),
                            ),
                            &[],
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// Resolve cross-file function calls using import graph.
    /// If function A calls B, and A's file imports B from another file,
    /// create a CALLS edge from A to the actual definition of B.
    fn resolve_cross_file_calls(&self) {
        // For each function, look at calls in its body.
        // If the called name matches an imported function (via IMPORTS edge),
        // create a CALLS edge to the imported function's definition.
        let _ = self.graph.query(
            &format!(
                "MATCH (caller:CodeFunction {{repo: '{}'}}) \
                 MATCH (fi:CodeFile {{repo: '{}'}})-[:IMPORTS]->(callee:CodeFunction {{repo: '{}'}}) \
                 WHERE fi.path = caller.file AND toLower(caller.body) CONTAINS toLower(callee.name) \
                 AND caller <> callee \
                 MERGE (caller)-[:CALLS]->(callee)",
                esc(&self.repo_name), esc(&self.repo_name), esc(&self.repo_name),
            ),
            &[],
        );

        // Dynamically detect validation/error-handling functions by name pattern
        // and build USES_VALIDATION edges for functions that call them
        let validation_patterns = ["validate", "retry", "guard", "assert", "check", "sanitize", "verify"];
        for pattern in &validation_patterns {
            let _ = self.graph.query(
                &format!(
                    "MATCH (v:CodeFunction {{repo: '{}'}}) \
                     WHERE toLower(v.name) CONTAINS '{}' \
                     MATCH (f:CodeFunction {{repo: '{}'}}) \
                     WHERE f <> v AND toLower(f.body) CONTAINS toLower(v.name) \
                     MERGE (f)-[:USES_VALIDATION]->(v)",
                    esc(&self.repo_name), pattern,
                    esc(&self.repo_name),
                ),
                &[],
            );
        }
    }

    /// Build a map of workspace package names to their repo-relative directory paths.
    /// Reads the root package.json "workspaces" field, then each workspace's package.json "name".
    /// Supports: ["packages/*"], ["packages/sdk", "packages/migrate"], pnpm-workspace.yaml
    fn build_workspace_map(repo_path: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();

        // Try root package.json first
        let root_pkg = std::path::Path::new(repo_path).join("package.json");
        let workspace_globs = if let Ok(contents) = std::fs::read_to_string(&root_pkg) {
            Self::extract_workspace_globs(&contents)
        } else {
            vec![]
        };

        // Also try pnpm-workspace.yaml
        let workspace_globs = if workspace_globs.is_empty() {
            let pnpm_ws = std::path::Path::new(repo_path).join("pnpm-workspace.yaml");
            if let Ok(contents) = std::fs::read_to_string(&pnpm_ws) {
                Self::extract_pnpm_workspace_globs(&contents)
            } else {
                vec![]
            }
        } else {
            workspace_globs
        };

        if workspace_globs.is_empty() {
            return map;
        }

        // Expand globs and find package.json in each workspace directory
        for glob_pattern in &workspace_globs {
            let full_pattern = format!("{}/{}/package.json", repo_path, glob_pattern);
            if let Ok(entries) = glob::glob(&full_pattern) {
                for entry in entries.flatten() {
                    if let Ok(pkg_contents) = std::fs::read_to_string(&entry) {
                        if let Some(name) = Self::extract_package_name(&pkg_contents) {
                            // Get the directory relative to repo root
                            if let Some(pkg_dir) = entry.parent() {
                                if let Ok(rel) = pkg_dir.strip_prefix(repo_path) {
                                    map.insert(name, rel.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        map
    }

    /// Extract workspace glob patterns from package.json content.
    /// Handles: "workspaces": ["packages/*"] and "workspaces": {"packages": ["packages/*"]}
    fn extract_workspace_globs(json_str: &str) -> Vec<String> {
        // Simple JSON extraction - avoid pulling in serde_json just for this
        let mut globs = vec![];
        // Look for "workspaces" key
        if let Some(ws_start) = json_str.find("\"workspaces\"") {
            let rest = &json_str[ws_start..];
            // Find the value after the colon
            if let Some(colon) = rest.find(':') {
                let value_part = rest[colon + 1..].trim_start();
                if value_part.starts_with('[') {
                    // Direct array: "workspaces": ["packages/*", ...]
                    Self::extract_string_array(value_part, &mut globs);
                } else if value_part.starts_with('{') {
                    // Object form: "workspaces": {"packages": ["packages/*"]}
                    if let Some(pkg_key) = value_part.find("\"packages\"") {
                        let inner = &value_part[pkg_key..];
                        if let Some(c) = inner.find(':') {
                            let arr_part = inner[c + 1..].trim_start();
                            Self::extract_string_array(arr_part, &mut globs);
                        }
                    }
                }
            }
        }
        globs
    }

    fn extract_string_array(text: &str, out: &mut Vec<String>) {
        if !text.starts_with('[') { return; }
        let end = text.find(']').unwrap_or(text.len());
        let inner = &text[1..end];
        for item in inner.split(',') {
            let trimmed = item.trim().trim_matches(|c| c == '"' || c == '\'');
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }

    /// Extract workspace patterns from pnpm-workspace.yaml
    fn extract_pnpm_workspace_globs(yaml_str: &str) -> Vec<String> {
        let mut globs = vec![];
        let mut in_packages = false;
        for line in yaml_str.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("packages:") {
                in_packages = true;
                continue;
            }
            if in_packages {
                if trimmed.starts_with("- ") {
                    let pattern = trimmed[2..].trim().trim_matches(|c| c == '"' || c == '\'');
                    if !pattern.is_empty() {
                        globs.push(pattern.to_string());
                    }
                } else if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    break; // end of packages list
                }
            }
        }
        globs
    }

    fn extract_package_name(json_str: &str) -> Option<String> {
        // Find "name": "value" in package.json
        let name_key = json_str.find("\"name\"")?;
        let rest = &json_str[name_key..];
        let colon = rest.find(':')?;
        let value_part = rest[colon + 1..].trim_start();
        let quote_start = value_part.find('"')?;
        let inner = &value_part[quote_start + 1..];
        let quote_end = inner.find('"')?;
        Some(inner[..quote_end].to_string())
    }

    /// Resolve a relative import path to a repo-relative file path.
    /// e.g., file="server/routes/resume.ts", import="../utils/llm-validation"
    /// -> "server/utils/llm-validation"
    /// Also resolves workspace imports: "@prisma/sdk" -> "packages/sdk/src"
    fn resolve_import_path(&self, from_file: &str, import_path: &str) -> String {
        if import_path.starts_with('.') {
            // Relative import - resolve against from_file directory
            let from_dir = std::path::Path::new(from_file).parent().unwrap_or(std::path::Path::new(""));
            let resolved = from_dir.join(import_path);

            // Normalize: resolve ".." and "." components
            let mut parts: Vec<&str> = vec![];
            for component in resolved.components() {
                match component {
                    std::path::Component::ParentDir => { parts.pop(); }
                    std::path::Component::CurDir => {}
                    std::path::Component::Normal(p) => { parts.push(p.to_str().unwrap_or("")); }
                    _ => {}
                }
            }
            return parts.join("/");
        }

        // Check workspace map for scoped packages (@org/pkg) and bare packages
        // Try exact match first: "@prisma/sdk" -> "packages/sdk"
        if let Some(dir) = self.workspace_map.get(import_path) {
            return dir.clone();
        }

        // Try with subpath: "@prisma/sdk/utils" -> "packages/sdk" + "/utils"
        // Split on first / after the scope (if scoped) or first / (if bare)
        let pkg_name = if import_path.starts_with('@') {
            // Scoped: @org/pkg/subpath -> package is @org/pkg
            let parts: Vec<&str> = import_path.splitn(3, '/').collect();
            if parts.len() >= 2 {
                format!("{}/{}", parts[0], parts[1])
            } else {
                import_path.to_string()
            }
        } else {
            // Bare: pkg/subpath -> package is pkg
            import_path.split('/').next().unwrap_or(import_path).to_string()
        };

        if let Some(dir) = self.workspace_map.get(&pkg_name) {
            // Return workspace dir + any subpath
            let subpath = import_path.strip_prefix(&pkg_name).unwrap_or("").trim_start_matches('/');
            if subpath.is_empty() {
                return dir.clone();
            }
            return format!("{}/{}", dir, subpath);
        }

        // Not a workspace package - return as-is (node_modules)
        import_path.to_string()
    }
}

#[derive(Default)]
pub struct IndexStats {
    pub files: usize,
    pub functions: usize,
    pub classes: usize,
    pub commits: usize,
    pub file_changes: usize,
}

impl IndexStats {
    pub fn summary(&self) -> String {
        format!("{} files, {} functions, {} classes, {} commits, {} file changes",
            self.files, self.functions, self.classes, self.commits, self.file_changes)
    }
}

/// Index git history — commits, authors, file changes, and links to code entities.
pub struct GitHistoryIndexer {
    graph: GraphClient,
    repo_name: String,
}

impl GitHistoryIndexer {
    pub fn new(graph: GraphClient, repo_name: &str) -> Self {
        Self { graph, repo_name: repo_name.to_string() }
    }

    /// Index git log into the graph. Creates Commit, Author nodes and
    /// AUTHORED, MODIFIED_FILE, MODIFIED edges.
    pub fn index(&self, repo_path: &str, max_commits: usize) -> (usize, usize) {
        use std::process::Command;

        // git log with file changes: hash, author, date, subject, files
        let output = match Command::new("git")
            .args([
                "log",
                &format!("--max-count={}", max_commits),
                "--format=COMMIT_START%n%H%n%an%n%ae%n%aI%n%s",
                "--name-status",
            ])
            .current_dir(repo_path)
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
            _ => return (0, 0),
        };

        let mut commits = 0;
        let mut file_changes = 0;

        let mut current_hash = String::new();
        let mut current_author = String::new();
        let mut current_email = String::new();
        let mut current_date = String::new();
        let mut current_message = String::new();
        let mut current_files: Vec<(String, String)> = vec![]; // (status, path)
        let mut in_commit = false;
        let mut line_idx = 0; // line within current commit block

        for line in output.lines() {
            if line == "COMMIT_START" {
                // Process previous commit
                if in_commit && !current_hash.is_empty() {
                    self.ingest_commit(
                        &current_hash, &current_author, &current_email,
                        &current_date, &current_message, &current_files,
                    );
                    commits += 1;
                    file_changes += current_files.len();
                }
                in_commit = true;
                line_idx = 0;
                current_files.clear();
                continue;
            }

            if in_commit {
                line_idx += 1;
                match line_idx {
                    1 => current_hash = line.to_string(),
                    2 => current_author = line.to_string(),
                    3 => current_email = line.to_string(),
                    4 => current_date = line.to_string(),
                    5 => current_message = line.to_string(),
                    _ => {
                        // File change lines: M\tpath, A\tpath, D\tpath
                        let trimmed = line.trim();
                        if trimmed.len() > 2 {
                            let status = &trimmed[..1];
                            let path = trimmed[1..].trim();
                            if !path.is_empty() && (status == "M" || status == "A" || status == "D" || status == "R") {
                                current_files.push((status.to_string(), path.to_string()));
                            }
                        }
                    }
                }
            }
        }

        // Process last commit
        if in_commit && !current_hash.is_empty() {
            self.ingest_commit(
                &current_hash, &current_author, &current_email,
                &current_date, &current_message, &current_files,
            );
            commits += 1;
            file_changes += current_files.len();
        }

        // Create MODIFIED edges: Commit → CodeFunction (if the commit touched a file containing that function)
        self.link_commits_to_functions();

        (commits, file_changes)
    }

    fn ingest_commit(
        &self,
        hash: &str, author: &str, email: &str,
        date: &str, message: &str, files: &[(String, String)],
    ) {
        let short_hash = &hash[..std::cmp::min(12, hash.len())];

        // Create commit node
        let _ = self.graph.query(
            &format!(
                "MERGE (c:Commit {{hash: '{}', repo: '{}'}}) \
                 SET c.short_hash = '{}', c.author = '{}', c.email = '{}', \
                 c.date = '{}', c.message = '{}', c.files_changed = {}",
                esc(hash), esc(&self.repo_name),
                esc(short_hash), esc(author), esc(email),
                esc(date), esc(&message.chars().take(200).collect::<String>()),
                files.len(),
            ),
            &[],
        );

        // Create author node + edge
        let _ = self.graph.query(
            &format!(
                "MERGE (a:Author {{email: '{}', repo: '{}'}}) \
                 SET a.name = '{}' \
                 WITH a \
                 MATCH (c:Commit {{hash: '{}', repo: '{}'}}) \
                 MERGE (a)-[:AUTHORED]->(c)",
                esc(email), esc(&self.repo_name),
                esc(author),
                esc(hash), esc(&self.repo_name),
            ),
            &[],
        );

        // File change edges
        for (status, path) in files {
            let change_type = match status.as_str() {
                "A" => "added",
                "D" => "deleted",
                "M" => "modified",
                "R" => "renamed",
                _ => "changed",
            };

            let _ = self.graph.query(
                &format!(
                    "MATCH (c:Commit {{hash: '{}', repo: '{}'}}) \
                     MERGE (f:CodeFile {{repo: '{}', path: '{}'}}) \
                     MERGE (c)-[:MODIFIED_FILE {{change: '{}'}}]->(f)",
                    esc(hash), esc(&self.repo_name),
                    esc(&self.repo_name), esc(path),
                    change_type,
                ),
                &[],
            );
        }
    }

    /// Link commits to functions: if a commit modified a file, and that file
    /// contains a function, create a MODIFIED edge from the commit to the function.
    fn link_commits_to_functions(&self) {
        let _ = self.graph.query(
            &format!(
                "MATCH (c:Commit {{repo: '{}'}})-[:MODIFIED_FILE]->(fi:CodeFile {{repo: '{}'}})-[:CONTAINS]->(fn:CodeFunction {{repo: '{}'}}) \
                 MERGE (c)-[:MODIFIED]->(fn)",
                esc(&self.repo_name), esc(&self.repo_name), esc(&self.repo_name),
            ),
            &[],
        );
    }
}

/// Analyze open PR branches: checkout each, diff against base, index changed
/// functions, and create PRChange nodes with risk signals.
pub struct PRAnalyzer {
    graph: GraphClient,
    repo_name: String,
}

impl PRAnalyzer {
    pub fn new(graph: GraphClient, repo_name: &str) -> Self {
        Self { graph, repo_name: repo_name.to_string() }
    }

    /// Analyze all open PRs in the graph by checking out their branches.
    pub fn analyze_open_prs(&self, repo_path: &str) -> usize {
        use std::process::Command;

        // Get open PRs from the graph
        let prs = match self.graph.query(
            &format!(
                "MATCH (p:GitHubPR {{repo: '{}', state: 'OPEN'}}) RETURN p.number, p.branch, p.title",
                esc(&self.repo_name)
            ),
            &[],
        ) {
            Ok(r) => r.rows,
            Err(_) => return 0,
        };

        // Save current branch
        let current = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(repo_path)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        let mut analyzed = 0;

        for pr_row in &prs {
            let pr_number = pr_row[0].as_i64();
            let branch = pr_row[1].as_str();
            let title = pr_row[2].as_str();

            if branch.is_empty() { continue; }

            // Fetch the branch
            let _ = Command::new("git")
                .args(["fetch", "origin", &format!("{}:{}", branch, branch)])
                .current_dir(repo_path)
                .output();

            // Get the diff: files changed between develop and this branch
            let diff_output = match Command::new("git")
                .args(["diff", "--name-status", &format!("origin/develop...{}", branch)])
                .current_dir(repo_path)
                .output()
            {
                Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
                _ => continue,
            };

            let mut changed_files: Vec<(String, String)> = vec![];
            for line in diff_output.lines() {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 2 {
                    changed_files.push((parts[0].to_string(), parts[1].to_string()));
                }
            }

            if changed_files.is_empty() { continue; }

            // Get the actual code diff for risk analysis
            let code_diff = Command::new("git")
                .args(["diff", &format!("origin/develop...{}", branch)])
                .current_dir(repo_path)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                .unwrap_or_default();

            // Analyze risks
            let mut risks: Vec<String> = vec![];
            let mut deleted_null_guards = 0;
            let mut added_without_null_guard = 0;
            let mut schema_changes = false;
            let mut deleted_files: Vec<String> = vec![];
            let mut high_churn_files: Vec<String> = vec![];

            for line in code_diff.lines() {
                // Deleted null guards
                if line.starts_with('-') && !line.starts_with("---") {
                    if line.contains("??") || line.contains("?.") || line.contains("!= null") || line.contains("!== null") || line.contains("!== undefined") {
                        deleted_null_guards += 1;
                    }
                }
                // Added code calling methods without null guard
                if line.starts_with('+') && !line.starts_with("+++") {
                    if (line.contains(".split(") || line.contains(".map(") || line.contains(".filter(") || line.contains(".forEach("))
                        && !line.contains("??") && !line.contains("?.") && !line.contains("|| []") && !line.contains("|| ''") {
                        added_without_null_guard += 1;
                    }
                }
            }

            // Check for schema/migration changes
            for (status, file) in &changed_files {
                if file.contains("migration") || file.contains("schema.prisma") {
                    schema_changes = true;
                }
                if status == "D" {
                    deleted_files.push(file.clone());
                }
            }

            if deleted_null_guards > 0 {
                risks.push(format!("Removes {} null/undefined safety checks", deleted_null_guards));
            }
            if added_without_null_guard > 3 {
                risks.push(format!("{} method calls without null guards (.split/.map/.filter)", added_without_null_guard));
            }
            if schema_changes {
                risks.push("Database schema/migration changes".to_string());
            }
            if !deleted_files.is_empty() {
                risks.push(format!("Deletes {} files (check imports)", deleted_files.len()));
            }

            // Calculate risk level
            let risk_level = if deleted_null_guards > 3 || (!deleted_files.is_empty() && schema_changes) {
                "HIGH"
            } else if deleted_null_guards > 0 || schema_changes || added_without_null_guard > 3 {
                "MEDIUM"
            } else {
                "LOW"
            };

            let risk_text = if risks.is_empty() {
                "No significant risks detected".to_string()
            } else {
                risks.join("; ")
            };

            // Store in graph
            let _ = self.graph.query(
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}}) \
                     SET p.files_changed = {}, p.risk_level = '{}', \
                     p.risk_details = '{}', p.analyzed = true",
                    pr_number, esc(&self.repo_name),
                    changed_files.len(), risk_level, esc(&risk_text),
                ),
                &[],
            );

            // Create PRChange nodes for each changed file, linked to CodeFunctions
            for (status, file) in &changed_files {
                let change_type = match status.as_str() {
                    "A" => "added", "D" => "deleted", "M" => "modified", _ => "changed",
                };
                let _ = self.graph.query(
                    &format!(
                        "MATCH (p:GitHubPR {{number: {}, repo: '{}'}}) \
                         MERGE (ch:PRChange {{pr: {}, file: '{}', repo: '{}'}}) \
                         SET ch.change_type = '{}' \
                         MERGE (p)-[:CHANGES]->(ch)",
                        pr_number, esc(&self.repo_name),
                        pr_number, esc(file), esc(&self.repo_name),
                        change_type,
                    ),
                    &[],
                );

                // Link PRChange to CodeFunctions in that file
                let _ = self.graph.query(
                    &format!(
                        "MATCH (ch:PRChange {{pr: {}, file: '{}', repo: '{}'}}) \
                         MATCH (f:CodeFunction {{repo: '{}', file: '{}'}}) \
                         MERGE (ch)-[:AFFECTS]->(f)",
                        pr_number, esc(file), esc(&self.repo_name),
                        esc(&self.repo_name), esc(file),
                    ),
                    &[],
                );

                // Check if any affected function has prod errors in Slack
                let _ = self.graph.query(
                    &format!(
                        "MATCH (ch:PRChange {{pr: {}, file: '{}', repo: '{}'}})-[:AFFECTS]->(f:CodeFunction) \
                         MATCH (m:SlackMessage) WHERE m.has_symptom = true AND toLower(m.text) CONTAINS toLower(f.name) \
                         MERGE (ch)-[:HAS_KNOWN_ISSUE {{source: 'slack'}}]->(m)",
                        pr_number, esc(file), esc(&self.repo_name),
                    ),
                    &[],
                );
            }

            // Check high churn: functions this PR touches that have been modified many times
            let _ = self.graph.query(
                &format!(
                    "MATCH (p:GitHubPR {{number: {}, repo: '{}'}})-[:CHANGES]->(ch:PRChange)-[:AFFECTS]->(f:CodeFunction) \
                     MATCH (c:Commit)-[:MODIFIED]->(f) \
                     WITH p, f, count(c) AS churn WHERE churn > 5 \
                     SET p.has_high_churn = true",
                    pr_number, esc(&self.repo_name),
                ),
                &[],
            );

            analyzed += 1;
        }

        // Restore original branch
        if !current.is_empty() && current != "HEAD" {
            let _ = Command::new("git")
                .args(["checkout", &current])
                .current_dir(repo_path)
                .output();
        }

        analyzed
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'").replace('\n', "\\n").replace('\r', "")
}
