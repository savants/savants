//! Abstract memory engine trait.
//!
//! All commands talk to this trait, not to GraphClient or CloudClient
//! directly. This enables transparent local ↔ cloud switching:
//!
//! - Local mode: `LocalEngine` wraps `GraphClient` (Redis/graph protocol)
//! - Cloud mode: `CloudEngine` wraps HTTPS calls to api.savants.cloud
//! - The client binary contains NO raw queries in cloud mode
//!
//! This is the architectural boundary that protects our query IP.
//! The CloudEngine sends structured API calls (JSON over HTTPS).
//! The LocalEngine sends Cypher queries (Redis protocol, localhost only).
//! A reverse engineer sees the trait methods, not the query implementations.

/// Result type for memory engine operations.
pub type EngineResult<T> = Result<T, String>;

/// The abstract interface for Savants' knowledge store.
///
/// Every CLI command calls methods on this trait. The implementation
/// decides whether to query a local graph or the cloud API.
pub trait MemoryEngine {
    // ── Query methods (read) ──

    /// Full cluster health: pod counts by status, top namespaces, deployment count.
    fn cluster_state(&self, cluster: &str) -> EngineResult<String>;

    /// Pod story: top log events for a pod or cluster, with severity filtering.
    fn pod_story(&self, cluster: &str, pod: Option<&str>, namespace: Option<&str>,
                 since_minutes: u64, min_severity: &str, limit: usize) -> EngineResult<String>;

    /// Host state: CPU, memory, disk, systemd units, top processes.
    fn host_state(&self, hostname: Option<&str>) -> EngineResult<String>;

    /// Host story: significant journal/kernel events.
    fn host_story(&self, hostname: Option<&str>, since_minutes: u64,
                  min_severity: &str, limit: usize) -> EngineResult<String>;

    /// Node and edge counts.
    fn graph_stats(&self) -> EngineResult<(i64, i64)>;

    /// List pods with optional filters.
    fn list_pods(&self, cluster: &str, namespace: Option<&str>,
                 status: Option<&str>, name_contains: Option<&str>) -> EngineResult<String>;

    /// Search functions/classes by name pattern.
    fn search_code(&self, pattern: &str) -> EngineResult<String>;

    /// Function X-ray: callers, callees, history, metadata.
    fn function_xray(&self, name: &str, file_path: Option<&str>) -> EngineResult<String>;

    /// Impact analysis: transitive dependents of a function.
    fn impact_analysis(&self, name: &str, max_depth: usize) -> EngineResult<String>;

    /// Raw query escape hatch (local only — disabled in cloud mode).
    fn raw_query(&self, query: &str) -> EngineResult<String>;

    // ── Ingest methods (write) ──

    /// Push a set of graph operations (nodes + edges) to the store.
    fn ingest_delta(&self, delta: IngestDelta) -> EngineResult<IngestResult>;
}

/// A batch of graph operations to apply.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IngestDelta {
    pub scope_type: String,   // "k8s", "host", "code"
    pub scope_name: String,   // "prod-cluster", "astra", "backend-repo"
    pub operations: Vec<GraphOp>,
}

/// A single graph operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GraphOp {
    MergeNode {
        label: String,
        key: std::collections::HashMap<String, String>,
        props: std::collections::HashMap<String, serde_json::Value>,
    },
    MergeEdge {
        from_label: String,
        from_key: std::collections::HashMap<String, String>,
        to_label: String,
        to_key: std::collections::HashMap<String, String>,
        edge_type: String,
    },
    DeleteNode {
        label: String,
        key: std::collections::HashMap<String, String>,
    },
}

/// Result of an ingest operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IngestResult {
    pub ops_applied: usize,
    pub nodes_created: usize,
    pub edges_created: usize,
}
