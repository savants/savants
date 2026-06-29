#![allow(dead_code)]
//! Temporal correlation engine for CAUSED_BY edge inference.
//!
//! Maintains a rolling window of recent cluster state changes (configmap
//! edits, deployment rollouts, pod restarts) and checks incoming LogEvents
//! against this window. When a LogEvent appears within `window_seconds`
//! of a state change in the same namespace, a `CAUSED_BY` edge is created
//! with `confidence: "candidate"` and `delta_seconds`.
//!
//! This is NOT causal inference -- it is temporal correlation. The edge says
//! "this error appeared N seconds after this configmap was edited" and
//! lets the human or AI decide if it is causal.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::graph::GraphClient;

/// A recorded cluster state change.
#[derive(Debug, Clone)]
pub struct StateChange {
    pub change_type: String,
    pub namespace: String,
    pub resource_name: String,
    pub timestamp: f64,
    pub label: String,
}

/// Rolling window of recent cluster state changes for CAUSED_BY correlation.
pub struct StateChangeTracker {
    graph: GraphClient,
    cluster: String,
    window_seconds: f64,
    changes: Mutex<VecDeque<StateChange>>,
    max_events: usize,
    edges_created: Mutex<u64>,
}

impl StateChangeTracker {
    pub fn new(graph: GraphClient, cluster: String) -> Self {
        Self {
            graph,
            cluster,
            window_seconds: 120.0,
            changes: Mutex::new(VecDeque::with_capacity(1000)),
            max_events: 1000,
            edges_created: Mutex::new(0),
        }
    }

    /// Record a state change in the rolling window.
    pub fn record(
        &self,
        change_type: &str,
        namespace: &str,
        resource_name: &str,
        label: &str,
    ) {
        let ts = now_secs();
        let change = StateChange {
            change_type: change_type.to_string(),
            namespace: namespace.to_string(),
            resource_name: resource_name.to_string(),
            timestamp: ts,
            label: label.to_string(),
        };

        let mut changes = self.changes.lock().unwrap();
        if changes.len() >= self.max_events {
            changes.pop_front();
        }
        changes.push_back(change);

        // Prune old entries
        let cutoff = ts - self.window_seconds;
        while let Some(front) = changes.front() {
            if front.timestamp < cutoff {
                changes.pop_front();
            } else {
                break;
            }
        }
    }

    /// Check if any recent state changes correlate with this LogEvent.
    /// Returns the number of CAUSED_BY edges created.
    pub fn correlate(
        &self,
        namespace: &str,
        pod: &str,
        template_hash: &str,
        event_ts: f64,
    ) -> u32 {
        let cutoff_lo = event_ts - self.window_seconds;
        let cutoff_hi = event_ts + 10.0; // small grace for clock skew

        let candidates: Vec<StateChange> = {
            let changes = self.changes.lock().unwrap();
            changes
                .iter()
                .filter(|c| {
                    c.timestamp >= cutoff_lo
                        && c.timestamp <= cutoff_hi
                        && (c.namespace.is_empty() || c.namespace == namespace)
                })
                .cloned()
                .collect()
        };

        if candidates.is_empty() {
            return 0;
        }

        let mut n = 0u32;
        for change in &candidates {
            if change.label.is_empty() {
                continue;
            }
            let delta = format!("{:.1}", event_ts - change.timestamp);

            let cypher = format!(
                "MATCH (e:LogEvent {{cluster: $cluster, namespace: $ns, pod: $pod, template_hash: $th}}) \
                 MATCH (x:{} {{name: $rname, namespace: $ns, cluster: $cluster}}) \
                 MERGE (e)-[r:CAUSED_BY]->(x) \
                 SET r.confidence = 'candidate', r.delta_seconds = $delta, r.change_type = $ctype",
                change.label
            );

            if self
                .graph
                .query(
                    &cypher,
                    &[
                        ("cluster", &self.cluster),
                        ("ns", namespace),
                        ("pod", pod),
                        ("th", template_hash),
                        ("rname", &change.resource_name),
                        ("delta", &delta),
                        ("ctype", &change.change_type),
                    ],
                )
                .is_ok()
            {
                n += 1;
            }
        }

        *self.edges_created.lock().unwrap() += n as u64;
        n
    }

    /// Total number of CAUSED_BY edges created since tracker creation.
    pub fn edges_created(&self) -> u64 {
        *self.edges_created.lock().unwrap()
    }

    /// Number of state changes currently in the rolling window.
    pub fn changes_in_window(&self) -> usize {
        self.changes.lock().unwrap().len()
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
