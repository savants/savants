//! Cloud event sink for K8s watcher.
//!
//! Buffers K8s state changes and posts them to the cloud API
//! in batches. Used alongside (or instead of) the FalkorDB ingestor.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// A K8s event to send to the cloud.
#[derive(Debug, Clone, Serialize)]
pub struct K8sEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub resource_type: String,
    pub name: String,
    pub namespace: String,
    pub action: String,
    pub severity: String,
    pub detail: String,
    pub timestamp: u64,
}

/// Buffers events and flushes to the cloud API.
pub struct CloudEventSink {
    cloud_url: String,
    api_key: String,
    cluster: String,
    buffer: Mutex<Vec<K8sEvent>>,
    flush_threshold: usize,
}

impl CloudEventSink {
    pub fn new(cloud_url: &str, api_key: &str, cluster: &str) -> Self {
        Self {
            cloud_url: cloud_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            cluster: cluster.to_string(),
            buffer: Mutex::new(Vec::with_capacity(100)),
            flush_threshold: 5, // Flush every 5 events for near-real-time
        }
    }

    /// Record a K8s event. Flushes when buffer reaches threshold.
    pub fn record(
        &self,
        event_type: &str,
        resource_type: &str,
        name: &str,
        namespace: &str,
        action: &str,
        severity: &str,
        detail: &str,
    ) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let event = K8sEvent {
            event_type: event_type.to_string(),
            resource_type: resource_type.to_string(),
            name: name.to_string(),
            namespace: namespace.to_string(),
            action: action.to_string(),
            severity: severity.to_string(),
            detail: detail.to_string(),
            timestamp: ts,
        };

        let should_flush = {
            let mut buf = self.buffer.lock().unwrap();
            buf.push(event);
            buf.len() >= self.flush_threshold
        };

        if should_flush {
            self.flush();
        }
    }

    /// Flush all buffered events to the cloud API.
    pub fn flush(&self) {
        let events: Vec<K8sEvent> = {
            let mut buf = self.buffer.lock().unwrap();
            if buf.is_empty() {
                return;
            }
            std::mem::take(&mut *buf)
        };

        let count = events.len();
        let body = serde_json::json!({
            "cluster": self.cluster,
            "events": events,
        });

        let body_str = serde_json::to_string(&body).unwrap();
        let url = format!("{}/api/v1/events/k8s", self.cloud_url);

        // Fire and forget - use curl subprocess (will be replaced with reqwest later)
        match std::process::Command::new("curl")
            .args([
                "-s", "--max-time", "10",
                "-X", "POST",
                "-H", &format!("Authorization: Bearer {}", self.api_key),
                "-H", "Content-Type: application/json",
                "-d", &body_str,
                &url,
            ])
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    eprintln!("[k8s-cloud] Flushed {} events to cloud", count);
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    eprintln!("[k8s-cloud] Flush failed: {}", stderr.chars().take(100).collect::<String>());
                }
            }
            Err(e) => {
                eprintln!("[k8s-cloud] Flush error: {}", e);
            }
        }
    }
}

impl Drop for CloudEventSink {
    fn drop(&mut self) {
        // Flush remaining events on shutdown
        self.flush();
    }
}
