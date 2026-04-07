//! PyO3 bindings — exposes the Rust hot path to Python.
//!
//! Built with `maturin develop --features python` from this directory.
//! After building, the existing Python code can opt in via:
//!
//!     try:
//!         from synapcode_core import build_cpg, compute_file_delta
//!         USE_RUST = True
//!     except ImportError:
//!         from synapcode.graph.cpg import CodePropertyGraphBuilder
//!         USE_RUST = False
//!
//! Wire format for return values is the same JSON as the Python delta protocol,
//! so the rest of the stack (FalkorDB writes, MCP server, history walker)
//! works without modification.

use crate::cpg::CodePropertyGraphBuilder;
use crate::delta::{compute_file_delta as rust_compute_file_delta, DeltaScope};
use pyo3::prelude::*;

/// Build a CPG for a repo. Returns a JSON string with stats and a list of
/// parsed files (Python side decides how to write into FalkorDB).
#[pyfunction]
fn build_cpg_stats(repo_path: &str) -> PyResult<String> {
    let builder = CodePropertyGraphBuilder::new(repo_path);
    let stats = builder.build();
    let json = serde_json::json!({
        "files": stats.files,
        "functions": stats.functions,
        "classes": stats.classes,
        "calls": stats.calls,
        "failed": stats.failed,
    });
    Ok(json.to_string())
}

/// Compute a Delta from a single file's before/after content. Returns the
/// JSON wire format defined in `docs/delta-protocol.md`.
#[pyfunction]
fn compute_file_delta(
    file_path: &str,
    before_content: Option<&str>,
    after_content: Option<&str>,
    org: &str,
    repo: &str,
    branch: Option<&str>,
) -> PyResult<String> {
    let scope = DeltaScope {
        org: org.to_string(),
        repo: repo.to_string(),
        branch: branch.unwrap_or("main").to_string(),
        base_sha: None,
        head_sha: None,
    };
    let delta = rust_compute_file_delta(file_path, before_content, after_content, scope);
    Ok(delta.to_json())
}

/// PyO3 module entry point. Built as `synapcode_core` Python extension.
#[pymodule]
fn synapcode_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(build_cpg_stats, m)?)?;
    m.add_function(wrap_pyfunction!(compute_file_delta, m)?)?;
    Ok(())
}
