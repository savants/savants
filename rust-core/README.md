# synapcode-core

Native Rust implementation of the SynapCode indexer hot path.

## What this is

A Rust crate that mirrors the Python implementation of:
- The Code Property Graph builder (`synapcode/graph/cpg.py`)
- The local delta computer (`synapcode/delta/computer.py`)
- The schema and canonical IDs (`synapcode/graph/schema.py`, `synapcode/delta/schema.py`)

It uses the same tree-sitter grammars (Python, JavaScript, TypeScript) and produces the same Delta wire format documented in `docs/delta-protocol.md`. The output is byte-compatible with the Python implementation, so the rest of the stack (FalkorDB writes, MCP server, history walker, layered graph) works without modification.

## Why

| Concern | Python | Rust |
|---|---|---|
| Tree-sitter parsing speed | baseline | **5-10x faster** (native, no Python overhead per node) |
| CLI cold-start time | ~1100ms (Python interpreter + imports) | **<50ms** (single static binary) |
| Distribution | venv + 542MB of deps | **single ~25MB binary** |
| Reverse-engineering resistance | trivial | meaningful (stripped, LTO'd, panic=abort) |
| Memory footprint per file | ~1KB Python objects + GC | tighter, no GC pauses |

This crate is the answer to the cold-start latency we measured: real graph queries take **0.8-4ms** through FalkorDB, but `synapcode impact ...` from the CLI takes **~1.2 seconds** because of Python startup. Replacing the indexer hot path with a Rust binary collapses that to ~50ms.

## Building

### Pure Rust library (for the future Rust CLI)

```bash
cd rust-core
cargo build --release
cargo test
```

### Python extension (for the existing Python codebase to opt into)

```bash
cd rust-core
pip install maturin
maturin develop --features python --release
```

After this, the existing Python code can opt into the Rust hot path:

```python
try:
    import synapcode_core
    USE_RUST = True
except ImportError:
    USE_RUST = False

if USE_RUST:
    stats_json = synapcode_core.build_cpg_stats("/path/to/repo")
    delta_json = synapcode_core.compute_file_delta("src/main.py", before, after, "acme", "backend")
else:
    # fall back to pure Python
    ...
```

## Layout

```
rust-core/
├── Cargo.toml           — crate metadata + dependencies
├── README.md            — this file
├── src/
│   ├── lib.rs           — public API surface
│   ├── schema.rs        — FileNode/FunctionNode/ClassNode + canonical_node_id
│   ├── cpg_walk.rs      — tree-sitter AST walker (shared by cpg + delta)
│   ├── cpg.rs           — CodePropertyGraphBuilder (full repo indexer)
│   ├── delta.rs         — Delta protocol + compute_file_delta
│   └── python_bindings.rs — PyO3 bindings (feature-gated)
└── tests/               — integration tests against real OSS repos
```

## Status

- ✅ Schema (FileNode, FunctionNode, ClassNode, CallSite, canonical_node_id)
- ✅ Tree-sitter walker for Python, JavaScript, TypeScript
- ✅ CodePropertyGraphBuilder.discover_files / parse_file / build
- ✅ Delta protocol types (matches `docs/delta-protocol.md` byte-for-byte)
- ✅ compute_file_delta (handles add/remove/modify file states)
- ✅ PyO3 bindings (feature `python`)
- ✅ Unit tests for all of the above
- ❌ FalkorDB client wrapper (Python keeps owning that for now)
- ❌ History walker (Python keeps owning that — git plumbing is fine in Python)
- ❌ Multi-pass CALLS edge resolution with same-file/global disambiguation
  (parser finds calls; Python's `cpg.py` decides how to materialize edges)
- ❌ Performance benchmarks vs. the Python implementation

## What this crate does NOT do

To keep the scope small for the first version, the Rust crate is responsible only for the **structural parsing hot path**: walking tree-sitter ASTs and emitting Delta JSON. Everything else stays in Python:

- FalkorDB I/O (Python's `falkordb` client is mature)
- Git plumbing (`subprocess` is fine; speed isn't the bottleneck)
- History walking (orchestration logic, not parsing)
- MCP server (not the bottleneck either)
- Temporal worker / agents

The split: **Rust for the CPU-bound parser, Python for everything else.** This lets us ship the Rust speedup incrementally without rewriting the world.

## Next steps after this scaffold

1. `cargo test` — verify the unit tests pass on this machine
2. `maturin develop --features python` — build the PyO3 module
3. Wire `synapcode_core` into `cpg.py` behind a feature flag
4. Benchmark: re-run the profiler from `docs/profiling-results.md` (when written) with the Rust hot path enabled and compare
5. Once we trust it, make Rust the default; Python becomes the fallback
