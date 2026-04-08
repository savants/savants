//! Public re-export of the AST walker so the delta module can use it
//! without depending on private cpg internals.

use crate::cpg::ParsedFile;
use crate::schema::{ClassNode, FunctionNode, CallSite, StringRef};
use tree_sitter::Node;

/// Walk an AST root and populate the parsed file structure.
pub fn walk_node_root(node: Node, source: &str, file_path: &str, out: &mut ParsedFile) {
    walk(node, source, file_path, out, "");
}

/// True if `s` could plausibly be a symbol name or dotted path.
/// Mirror of `_looks_like_symbol` in `src/synapcode/graph/cpg.py`.
fn looks_like_symbol(s: &str) -> bool {
    if s.is_empty() || s.len() > 120 || s.contains(' ') || s.contains('\n') {
        return false;
    }
    // Must have at least one uppercase letter or a dot — filters "get", "utf-8".
    if !s.chars().any(|c| c.is_ascii_uppercase()) && !s.contains('.') {
        return false;
    }
    // Validate identifier-or-dotted-identifier shape.
    for seg in s.split('.') {
        if seg.is_empty() {
            return false;
        }
        let mut chars = seg.chars();
        let first = chars.next().unwrap();
        if !(first.is_ascii_alphabetic() || first == '_') {
            return false;
        }
        if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

/// Extract a decorator's callable name, e.g. `@workflow.defn` -> `workflow.defn`.
fn decorator_name(dec_node: Node, source: &str) -> String {
    let mut cursor = dec_node.walk();
    for child in dec_node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "attribute" | "dotted_name" => {
                return child.utf8_text(source.as_bytes()).unwrap_or("").to_string();
            }
            "call" => {
                if let Some(fn_node) = child.child_by_field_name("function") {
                    return fn_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                }
            }
            _ => {}
        }
    }
    // Fallback: strip '@' and any call args
    let raw = dec_node.utf8_text(source.as_bytes()).unwrap_or("");
    let trimmed = raw.trim_start_matches('@');
    match trimmed.find('(') {
        Some(idx) => trimmed[..idx].to_string(),
        None => trimmed.to_string(),
    }
}

fn collect_decorators(fn_node: Node, source: &str) -> Vec<String> {
    let mut decs = Vec::new();
    // Python: function_definition is wrapped in `decorated_definition`;
    // decorators are earlier children of the parent.
    if let Some(parent) = fn_node.parent() {
        if parent.kind() == "decorated_definition" {
            let mut cursor = parent.walk();
            for sib in parent.children(&mut cursor) {
                if sib.kind() == "decorator" {
                    decs.push(decorator_name(sib, source));
                }
            }
        }
    }
    // JS/TS: decorators can be direct children of the function.
    let mut cursor = fn_node.walk();
    for child in fn_node.children(&mut cursor) {
        if child.kind() == "decorator" {
            decs.push(decorator_name(child, source));
        }
    }
    decs
}

fn walk(node: Node, source: &str, file_path: &str, out: &mut ParsedFile, enclosing: &str) {
    let mut current = enclosing.to_string();

    match node.kind() {
        "function_definition" | "function_declaration" | "method_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = name_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                current = name.clone();

                let mut params = Vec::new();
                if let Some(params_node) = node.child_by_field_name("parameters") {
                    let mut cursor = params_node.walk();
                    for child in params_node.children(&mut cursor) {
                        if matches!(
                            child.kind(),
                            "identifier" | "typed_parameter" | "typed_default_parameter"
                        ) {
                            let pname = child
                                .child_by_field_name("name")
                                .map(|n| n.utf8_text(source.as_bytes()).unwrap_or(""))
                                .unwrap_or_else(|| child.utf8_text(source.as_bytes()).unwrap_or(""));
                            params.push(pname.to_string());
                        }
                    }
                }

                let decorators = collect_decorators(node, source);

                out.functions.push(FunctionNode {
                    name,
                    file_path: file_path.to_string(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    parameters: params,
                    return_type: String::new(),
                    decorators,
                });
            }
        }
        "class_definition" | "class_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = name_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                out.classes.push(ClassNode {
                    name,
                    file_path: file_path.to_string(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    bases: Vec::new(),
                });
            }
        }
        "import_statement" | "import_from_statement" => {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                out.imports.push(text.to_string());
            }
        }
        "string" | "string_literal" => {
            // Capture string literals that look like symbol names. This closes
            // the registry-dispatch blind spot (e.g. Temporal activity names,
            // handler registry keys) that grep-based search handles poorly.
            let raw = node.utf8_text(source.as_bytes()).unwrap_or("");
            let stripped = raw.trim_matches(|c| c == '\'' || c == '"' || c == '`');
            if looks_like_symbol(stripped) {
                out.string_refs.push(StringRef {
                    caller_file: file_path.to_string(),
                    caller_function: enclosing.to_string(),
                    value: stripped.to_string(),
                    line: node.start_position().row as u32 + 1,
                });
            }
        }
        "call" => {
            if let Some(fn_node) = node.child_by_field_name("function") {
                let mut callee = fn_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                if let Some(idx) = callee.rfind('.') {
                    callee = callee[idx + 1..].to_string();
                }
                out.calls.push(CallSite {
                    caller_file: file_path.to_string(),
                    caller_function: enclosing.to_string(),
                    callee_name: callee,
                    line: node.start_position().row as u32 + 1,
                });
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, file_path, out, &current);
    }
}
