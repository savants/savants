//! synapcode-core — native Rust hot path for SynapCode.
//!
//! Mirrors the Python CPG builder and delta computer with these wins:
//!   - 5-10x faster tree-sitter parsing (native, no Python overhead)
//!   - Zero Python startup cost when called from a single Rust binary
//!   - Single dependency-free binary for the closed-source distribution
//!   - Same Delta protocol wire format (`docs/delta-protocol.md`)
//!
//! Two consumption modes:
//!   1. Pure Rust library (`use synapcode_core::*`) — for the future Rust CLI
//!   2. Python extension (`pip install` via maturin, feature `python`) —
//!      so the existing Python code can opt into the Rust indexer with one
//!      import change

pub mod config;
pub mod cpg;
pub mod cpg_walk;
pub mod delta;
pub mod schema;

#[cfg(feature = "python")]
pub mod python_bindings;

pub use cpg::{CodePropertyGraphBuilder, ParseStats};
pub use delta::{compute_file_delta, Delta, DeltaScope, Operation};
pub use schema::{ClassNode, ConfigKeyNode, FileNode, FunctionNode};
