//! Kubernetes cluster ingestor for the Mazkir runtime layer.
//!
//! Reads the state of a Kubernetes cluster via the `kube` crate,
//! maps resources to Mazkir graph nodes, and writes them to a Savants memory graph
//! via `GraphClient`. Supports snapshot mode (full reconcile), watch mode
//! (live streaming), log intelligence, and temporal correlation.

pub mod cloud_sink;
pub mod correlator;
pub mod ingestor;
pub mod logs;
pub mod watcher;

pub use ingestor::K8sIngestor;
pub use watcher::K8sWatcher;
