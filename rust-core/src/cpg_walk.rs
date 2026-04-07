//! Public re-export of the AST walker so the delta module can use it
//! without depending on private cpg internals.

use crate::cpg::ParsedFile;
use crate::schema::{ClassNode, FunctionNode, CallSite};
use tree_sitter::Node;

/// Walk an AST root and populate the parsed file structure.
pub fn walk_node_root(node: Node, source: &str, file_path: &str, out: &mut ParsedFile) {
    walk(node, source, file_path, out, "");
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

                out.functions.push(FunctionNode {
                    name,
                    file_path: file_path.to_string(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    parameters: params,
                    return_type: String::new(),
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
