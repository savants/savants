use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Internal state — not a user-facing config file.
///
/// Like gcloud, all configuration happens via commands:
///   savants connect      → authenticates to savants.cloud
///   savants mcp install  → configures MCP for AI tools
///   savants up            → auto-detects everything
///
/// Users never edit this file. It's stored at ~/.savants/state.json
/// and managed entirely by the CLI. Env vars override for CI/automation.
pub const CLOUD_ENDPOINT: &str = "https://api.savants.cloud";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct State {
    /// Graph connection (auto-detected or set via env vars)
    #[serde(default)]
    pub graph_host: String,
    #[serde(default)]
    pub graph_port: u16,
    #[serde(default)]
    pub graph_name: String,

    /// Cloud authentication (set by `savants connect`)
    pub cloud_device_token: Option<String>,
    pub cloud_device_id: Option<String>,
    pub cloud_org: Option<String>,
}

impl State {
    fn state_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".savants")
            .join("state.json")
    }

    pub fn load() -> Self {
        let path = Self::state_path();
        if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::state_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)
    }

    /// Graph host — env var overrides stored state.
    pub fn graph_host(&self) -> String {
        env::var("FALKORDB_HOST").unwrap_or_else(|_| {
            if self.graph_host.is_empty() {
                "localhost".to_string()
            } else {
                self.graph_host.clone()
            }
        })
    }

    /// Graph port — env var overrides stored state.
    pub fn graph_port(&self) -> u16 {
        env::var("FALKORDB_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                if self.graph_port == 0 { 16379 } else { self.graph_port }
            })
    }

    /// Graph name — env var overrides stored state.
    pub fn graph_name(&self) -> String {
        env::var("FALKORDB_GRAPH").unwrap_or_else(|_| {
            if self.graph_name.is_empty() {
                "savants".to_string()
            } else {
                self.graph_name.clone()
            }
        })
    }

    /// Cloud token — env var overrides stored state.
    pub fn cloud_token(&self) -> Option<String> {
        env::var("SAVANTS_TOKEN").ok().or_else(|| self.cloud_device_token.clone())
    }

    pub fn is_cloud_authenticated(&self) -> bool {
        self.cloud_token().is_some()
    }

    /// Graph name for a cluster (convention: hyphens → underscores).
    pub fn cluster_graph_name(cluster: &str) -> String {
        cluster.replace('-', "_")
    }
}
