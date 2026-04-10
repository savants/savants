//! Config file indexing: YAML / TOML / JSON -> flat list of ConfigKey nodes.
//!
//! Mirrors `parse_config_file` + `_flatten_config` in
//! `src/savants/graph/cpg.py`. The goal is to capture *infrastructure*
//! config (mongod settings, helm values, CI env vars, feature flags) so
//! questions like "is the Mongo profiler enabled?" don't go unanswered
//! just because the setting lives outside Python/JS/TS source.

use crate::schema::ConfigKeyNode;
use serde_json::Value as JsonValue;
use std::path::Path;

const VALUE_MAX_LEN: usize = 200;
const KEY_PATH_MAX_DEPTH: usize = 8;

/// Lockfiles / generated configs we deliberately skip.
const EXCLUDE_NAMES: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "Cargo.lock",
    "tsconfig.tsbuildinfo",
    ".prettierrc.json",
];

/// Recognized config file extensions and their format tag.
pub fn config_format(ext: &str) -> Option<&'static str> {
    match ext {
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "json" => Some("json"),
        _ => None,
    }
}

/// True if this file should be indexed as a config file.
pub fn is_indexable_config(path: &Path) -> bool {
    let Some(fname) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if EXCLUDE_NAMES.contains(&fname) {
        return false;
    }
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    if config_format(ext).is_none() {
        return false;
    }
    // Skip files larger than 1 MB — that's data, not config.
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > 1_000_000 {
            return false;
        }
    }
    true
}

/// Parse a config file and flatten it into `ConfigKeyNode` leaves.
///
/// `rel_path` is the path relative to the repo root, stored on every
/// returned node. Returns an empty vec on parse errors — we'd rather
/// silently skip a malformed config than abort the whole index.
pub fn parse_config_file(path: &Path, rel_path: &str) -> Vec<ConfigKeyNode> {
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return Vec::new();
    };
    let Some(fmt) = config_format(ext) else {
        return Vec::new();
    };

    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };

    // Parse into a common JsonValue shape so the flattener only has one
    // branch. serde_yaml and toml both support converting to serde_json::Value.
    let json: JsonValue = match fmt {
        "yaml" => match serde_yaml::from_str::<serde_yaml::Value>(&text) {
            Ok(v) => yaml_to_json(v),
            Err(_) => return Vec::new(),
        },
        "toml" => match toml::from_str::<toml::Value>(&text) {
            Ok(v) => toml_to_json(v),
            Err(_) => return Vec::new(),
        },
        "json" => match serde_json::from_str::<JsonValue>(&text) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        },
        _ => return Vec::new(),
    };

    let mut out = Vec::new();
    flatten(&json, "", rel_path, fmt, &mut out, 0);
    out
}

fn flatten(
    node: &JsonValue,
    prefix: &str,
    file_path: &str,
    fmt: &str,
    out: &mut Vec<ConfigKeyNode>,
    depth: usize,
) {
    if depth > KEY_PATH_MAX_DEPTH {
        return;
    }
    match node {
        JsonValue::Object(map) => {
            for (k, v) in map {
                let new_prefix = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(v, &new_prefix, file_path, fmt, out, depth + 1);
            }
        }
        JsonValue::Array(items) => {
            // If every element is scalar, collapse into one joined leaf.
            // Otherwise index positionally like docker-compose services etc.
            let all_scalar = items.iter().all(|x| !x.is_object() && !x.is_array());
            if all_scalar && !prefix.is_empty() {
                let joined = items
                    .iter()
                    .map(scalar_to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                push_leaf(out, prefix, file_path, fmt, &joined);
            } else {
                for (i, item) in items.iter().enumerate() {
                    let new_prefix = if prefix.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{prefix}[{i}]")
                    };
                    flatten(item, &new_prefix, file_path, fmt, out, depth + 1);
                }
            }
        }
        _ => {
            // Scalar leaf
            if !prefix.is_empty() {
                let s = scalar_to_string(node);
                push_leaf(out, prefix, file_path, fmt, &s);
            }
        }
    }
}

fn push_leaf(
    out: &mut Vec<ConfigKeyNode>,
    name: &str,
    file_path: &str,
    fmt: &str,
    value: &str,
) {
    let truncated = if value.len() > VALUE_MAX_LEN {
        value[..VALUE_MAX_LEN].to_string()
    } else {
        value.to_string()
    };
    out.push(ConfigKeyNode {
        name: name.to_string(),
        file_path: file_path.to_string(),
        value: truncated,
        format: fmt.to_string(),
        line: 0,
    });
}

fn scalar_to_string(v: &JsonValue) -> String {
    match v {
        JsonValue::Null => String::new(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(s) => s.clone(),
        // Non-scalar (shouldn't hit this in `all_scalar` path) — fall back to JSON
        other => other.to_string(),
    }
}

/// Convert a `serde_yaml::Value` into `serde_json::Value`. YAML allows
/// non-string keys; we coerce them to strings (same as the Python `str(k)`
/// path) so the flattener has uniform map keys.
fn yaml_to_json(v: serde_yaml::Value) -> JsonValue {
    use serde_yaml::Value as Y;
    match v {
        Y::Null => JsonValue::Null,
        Y::Bool(b) => JsonValue::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                JsonValue::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null)
            } else {
                JsonValue::Null
            }
        }
        Y::String(s) => JsonValue::String(s),
        Y::Sequence(items) => JsonValue::Array(items.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in map {
                let key = yaml_key_to_string(k);
                obj.insert(key, yaml_to_json(val));
            }
            JsonValue::Object(obj)
        }
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

fn yaml_key_to_string(k: serde_yaml::Value) -> String {
    use serde_yaml::Value as Y;
    match k {
        Y::String(s) => s,
        Y::Number(n) => n.to_string(),
        Y::Bool(b) => b.to_string(),
        Y::Null => "null".to_string(),
        other => format!("{:?}", other),
    }
}

/// Convert a `toml::Value` into `serde_json::Value`. TOML dates become
/// their canonical string representation.
fn toml_to_json(v: toml::Value) -> JsonValue {
    use toml::Value as T;
    match v {
        T::String(s) => JsonValue::String(s),
        T::Integer(i) => JsonValue::Number(i.into()),
        T::Float(f) => serde_json::Number::from_f64(f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        T::Boolean(b) => JsonValue::Bool(b),
        T::Datetime(dt) => JsonValue::String(dt.to_string()),
        T::Array(items) => JsonValue::Array(items.into_iter().map(toml_to_json).collect()),
        T::Table(tbl) => {
            let mut obj = serde_json::Map::new();
            for (k, val) in tbl {
                obj.insert(k, toml_to_json(val));
            }
            JsonValue::Object(obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &tempfile::TempDir, name: &str, contents: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    #[test]
    fn parse_toml_pyproject_like() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            &dir,
            "pyproject.toml",
            "[project]\nname = \"savants\"\nversion = \"0.1.0\"\n[project.optional-dependencies]\ndev = [\"pytest\", \"ruff\"]\n",
        );
        let keys = parse_config_file(&p, "pyproject.toml");
        let by_name: std::collections::HashMap<&str, &ConfigKeyNode> =
            keys.iter().map(|k| (k.name.as_str(), k)).collect();
        assert_eq!(by_name["project.name"].value, "savants");
        assert_eq!(by_name["project.version"].value, "0.1.0");
        assert_eq!(
            by_name["project.optional-dependencies.dev"].value,
            "pytest, ruff"
        );
        assert!(keys.iter().all(|k| k.format == "toml"));
    }

    #[test]
    fn parse_yaml_mongo_style() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            &dir,
            "mongod.yaml",
            "operationProfiling:\n  mode: slowOp\n  slowOpThresholdMs: 100\nsecurity:\n  javascriptEnabled: false\n",
        );
        let keys = parse_config_file(&p, "mongod.yaml");
        let by_name: std::collections::HashMap<&str, &ConfigKeyNode> =
            keys.iter().map(|k| (k.name.as_str(), k)).collect();
        assert_eq!(by_name["operationProfiling.mode"].value, "slowOp");
        assert_eq!(
            by_name["operationProfiling.slowOpThresholdMs"].value,
            "100"
        );
        assert_eq!(by_name["security.javascriptEnabled"].value, "false");
    }

    #[test]
    fn parse_json_arrays_and_scalars() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(
            &dir,
            "cfg.json",
            r#"{"ports": [80, 443], "services": [{"name":"a"},{"name":"b"}]}"#,
        );
        let keys = parse_config_file(&p, "cfg.json");
        let by_name: std::collections::HashMap<&str, &ConfigKeyNode> =
            keys.iter().map(|k| (k.name.as_str(), k)).collect();
        // scalar array collapses
        assert_eq!(by_name["ports"].value, "80, 443");
        // object array indexes positionally
        assert_eq!(by_name["services[0].name"].value, "a");
        assert_eq!(by_name["services[1].name"].value, "b");
    }

    #[test]
    fn excludes_lockfiles() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(&dir, "package-lock.json", r#"{"lockfileVersion": 3}"#);
        assert!(!is_indexable_config(&p));
    }

    #[test]
    fn handles_malformed_file_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let p = write(&dir, "broken.yaml", "this: : : not valid:\n  - [unclosed");
        let keys = parse_config_file(&p, "broken.yaml");
        assert!(keys.is_empty());
    }
}
