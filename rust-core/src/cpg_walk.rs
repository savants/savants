//! Public re-export of the AST walker so the delta module can use it
//! without depending on private cpg internals.

use crate::cpg::ParsedFile;
use crate::schema::{CallSite, ClassNode, EnvVarNode, FunctionNode, StringRef};
use tree_sitter::Node;

/// Walk an AST root and populate the parsed file structure.
pub fn walk_node_root(node: Node, source: &str, file_path: &str, out: &mut ParsedFile) {
    walk(node, source, file_path, out, "", "");
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

/// Pull the first string literal from a function/class body.
/// Python-style docstring; noop for other languages where the body's first
/// child isn't a bare expression-statement string.
fn extract_docstring(node: Node, source: &str) -> String {
    let Some(body) = node.child_by_field_name("body") else {
        return String::new();
    };
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "comment" | ":" | "{" | "\n" | "newline" => continue,
            "expression_statement" => {
                let mut gc = child.walk();
                for grand in child.children(&mut gc) {
                    if matches!(grand.kind(), "string" | "string_literal") {
                        let raw = grand.utf8_text(source.as_bytes()).unwrap_or("");
                        let stripped = strip_string_delimiters(raw);
                        let trimmed = stripped.trim();
                        return trimmed.chars().take(500).collect();
                    }
                }
                return String::new();
            }
            _ => return String::new(),
        }
    }
    String::new()
}

fn strip_string_delimiters(raw: &str) -> &str {
    for marker in ["\"\"\"", "'''"] {
        if raw.starts_with(marker) && raw.ends_with(marker) && raw.len() >= marker.len() * 2 {
            return &raw[marker.len()..raw.len() - marker.len()];
        }
    }
    for marker in ['"', '\'', '`'] {
        let m = marker.to_string();
        if raw.starts_with(&m) && raw.ends_with(&m) && raw.len() >= 2 {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

/// Extract base classes. Python: `superclasses` argument_list. JS/TS:
/// `class_heritage` / `superclass`.
fn extract_bases(class_node: Node, source: &str) -> Vec<String> {
    let mut bases = Vec::new();

    if let Some(sup) = class_node.child_by_field_name("superclasses") {
        let mut cursor = sup.walk();
        for child in sup.children(&mut cursor) {
            if matches!(child.kind(), "identifier" | "attribute" | "dotted_name") {
                bases.push(child.utf8_text(source.as_bytes()).unwrap_or("").to_string());
            }
        }
    }
    if let Some(h) = class_node
        .child_by_field_name("class_heritage")
        .or_else(|| class_node.child_by_field_name("superclass"))
    {
        let text = h.utf8_text(source.as_bytes()).unwrap_or("").trim().to_string();
        let cleaned = text.trim_start_matches("extends ").trim();
        for part in cleaned.split(',') {
            let p = part.trim().to_string();
            if !p.is_empty() && !bases.contains(&p) {
                bases.push(p);
            }
        }
    }
    bases
}

/// Recognize os.getenv / os.environ.get / getenv calls and pull out the
/// key name + optional default value. Returns None if not an env call.
fn extract_env_var_call(
    full_callee: &str,
    call_node: Node,
    source: &str,
) -> Option<(String, String)> {
    let parts: Vec<&str> = full_callee.rsplitn(3, '.').collect();
    let last = *parts.first()?;
    let parent = parts.get(1).copied().unwrap_or("");

    let is_env = last == "getenv"
        || (last == "get" && (parent == "environ" || parent == "os.environ"));
    if !is_env {
        return None;
    }

    let args = call_node.child_by_field_name("arguments")?;
    let mut strings = Vec::new();
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if matches!(child.kind(), "string" | "string_literal") {
            let raw = child.utf8_text(source.as_bytes()).unwrap_or("");
            strings.push(strip_string_delimiters(raw).to_string());
        }
    }
    let name = strings.first()?.clone();
    if name.is_empty() || name.len() > 100 || name.contains(' ') {
        return None;
    }
    let default = strings.get(1).cloned().unwrap_or_default();
    let default: String = default.chars().take(200).collect();
    Some((name, default))
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

fn walk(
    node: Node,
    source: &str,
    file_path: &str,
    out: &mut ParsedFile,
    enclosing_fn: &str,
    enclosing_class: &str,
) {
    let mut current_fn = enclosing_fn.to_string();
    let mut current_class = enclosing_class.to_string();

    match node.kind() {
        "function_definition" | "function_declaration" | "method_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = name_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                current_fn = name.clone();

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
                let docstring = extract_docstring(node, source);

                out.functions.push(FunctionNode {
                    name,
                    file_path: file_path.to_string(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    parameters: params,
                    return_type: String::new(),
                    decorators,
                    docstring,
                    class_name: enclosing_class.to_string(),
                });
            }
        }
        "class_definition" | "class_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = name_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                current_class = name.clone();

                let bases = extract_bases(node, source);
                let docstring = extract_docstring(node, source);

                out.classes.push(ClassNode {
                    name,
                    file_path: file_path.to_string(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    bases,
                    docstring,
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
                    caller_function: enclosing_fn.to_string(),
                    value: stripped.to_string(),
                    line: node.start_position().row as u32 + 1,
                });
            }
        }
        "call" => {
            if let Some(fn_node) = node.child_by_field_name("function") {
                let full_callee = fn_node.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                let callee = match full_callee.rfind('.') {
                    Some(idx) => full_callee[idx + 1..].to_string(),
                    None => full_callee.clone(),
                };
                out.calls.push(CallSite {
                    caller_file: file_path.to_string(),
                    caller_function: enclosing_fn.to_string(),
                    callee_name: callee,
                    line: node.start_position().row as u32 + 1,
                });

                // Env var extraction: os.getenv("X") / os.environ.get("X")
                if let Some((name, default)) = extract_env_var_call(&full_callee, node, source) {
                    out.env_vars.push(EnvVarNode {
                        name,
                        file_path: file_path.to_string(),
                        default_value: default,
                    });
                }
            }
        }
        // os.environ["X"]
        "subscript" => {
            if let Some(val) = node.child_by_field_name("value") {
                let var_text = val.utf8_text(source.as_bytes()).unwrap_or("");
                if var_text == "os.environ" || var_text == "environ" {
                    // Find the subscript argument — tree-sitter-python uses a
                    // `subscript` field for the index expression.
                    if let Some(sub) = node.child_by_field_name("subscript") {
                        if sub.kind() == "string" || sub.kind() == "string_literal" {
                            let raw = sub.utf8_text(source.as_bytes()).unwrap_or("");
                            let stripped = raw.trim_matches(|c| c == '\'' || c == '"' || c == '`');
                            if !stripped.is_empty() && stripped.len() <= 100 {
                                out.env_vars.push(EnvVarNode {
                                    name: stripped.to_string(),
                                    file_path: file_path.to_string(),
                                    default_value: String::new(),
                                });
                            }
                        }
                    }
                }
            }
        }
        // process.env.DATABASE_URL  (JS/TS)
        "member_expression" | "attribute" => {
            let text = node.utf8_text(source.as_bytes()).unwrap_or("");
            if let Some(rest) = text.strip_prefix("process.env.") {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.env_vars.push(EnvVarNode {
                        name,
                        file_path: file_path.to_string(),
                        default_value: String::new(),
                    });
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, file_path, out, &current_fn, &current_class);
    }
}
