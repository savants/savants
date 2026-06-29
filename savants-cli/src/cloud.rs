//! Cloud engine — talks to api.savants.cloud over HTTPS.
//!
//! Sends structured JSON API calls, never raw Cypher. The query logic
//! lives entirely on the server. This binary contains zero query IP
//! when operating in cloud mode.

use crate::config::{State, CLOUD_ENDPOINT};
use crate::engine::*;

pub struct CloudEngine {
    token: String,
    endpoint: String,
}

impl CloudEngine {
    pub fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            endpoint: CLOUD_ENDPOINT.to_string(),
        }
    }

    pub fn from_state() -> Option<Self> {
        let state = State::load();
        let token = state.cloud_token()?;
        Some(Self::new(&token))
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> EngineResult<serde_json::Value> {
        // Synchronous HTTP for now — can be made async later
        let url = format!("{}{}", self.endpoint, path);
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(&url)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!("API error {}: {}", status, body));
        }

        resp.json::<serde_json::Value>()
            .map_err(|e| format!("JSON parse error: {}", e))
    }

    fn get(&self, path: &str) -> EngineResult<serde_json::Value> {
        let url = format!("{}{}", self.endpoint, path);
        let client = reqwest::blocking::Client::new();
        let resp = client
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .map_err(|e| format!("HTTP error: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("API error {}", resp.status()));
        }

        resp.json::<serde_json::Value>()
            .map_err(|e| format!("JSON parse error: {}", e))
    }
}

impl MemoryEngine for CloudEngine {
    fn cluster_state(&self, cluster: &str) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/cluster_state", &serde_json::json!({
            "cluster": cluster
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn pod_story(&self, cluster: &str, pod: Option<&str>, namespace: Option<&str>,
                 since_minutes: u64, min_severity: &str, limit: usize) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/pod_story", &serde_json::json!({
            "cluster": cluster,
            "pod": pod,
            "namespace": namespace,
            "since_minutes": since_minutes,
            "min_severity": min_severity,
            "limit": limit,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn host_state(&self, hostname: Option<&str>) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/host_state", &serde_json::json!({
            "hostname": hostname,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn host_story(&self, hostname: Option<&str>, since_minutes: u64,
                  min_severity: &str, limit: usize) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/host_story", &serde_json::json!({
            "hostname": hostname,
            "since_minutes": since_minutes,
            "min_severity": min_severity,
            "limit": limit,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn graph_stats(&self) -> EngineResult<(i64, i64)> {
        let resp = self.get("/api/v1/graphs/stats")?;
        let nodes = resp["nodes"].as_i64().unwrap_or(0);
        let edges = resp["edges"].as_i64().unwrap_or(0);
        Ok((nodes, edges))
    }

    fn list_pods(&self, cluster: &str, namespace: Option<&str>,
                 status: Option<&str>, name_contains: Option<&str>) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/list_pods", &serde_json::json!({
            "cluster": cluster,
            "namespace": namespace,
            "status": status,
            "name_contains": name_contains,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn search_code(&self, pattern: &str) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/search_code", &serde_json::json!({
            "pattern": pattern,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn function_xray(&self, name: &str, file_path: Option<&str>) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/function_xray", &serde_json::json!({
            "function_name": name,
            "file_path": file_path,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn impact_analysis(&self, name: &str, max_depth: usize) -> EngineResult<String> {
        let resp = self.post("/api/v1/mcp/impact_analysis", &serde_json::json!({
            "function_name": name,
            "max_depth": max_depth,
        }))?;
        Ok(resp["result"].as_str().unwrap_or("").to_string())
    }

    fn raw_query(&self, _query: &str) -> EngineResult<String> {
        Err("Raw queries are not supported in cloud mode. Use the structured tools instead.".to_string())
    }

    fn ingest_delta(&self, delta: IngestDelta) -> EngineResult<IngestResult> {
        let resp = self.post("/api/v1/ingest/delta", &serde_json::to_value(&delta)
            .map_err(|e| format!("serialize error: {}", e))?)?;
        serde_json::from_value(resp)
            .map_err(|e| format!("deserialize error: {}", e))
    }
}
