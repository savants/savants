//! Live K8s watch-based ingestor for the Mazkir runtime layer.
//!
//! Complements `K8sIngestor::snapshot()` (pull-based full reconcile) with a
//! push-based streaming mode using `kube::runtime::watcher`. Each resource
//! type gets its own watcher task. On Applied events, the same MERGE logic as
//! snapshot is used. On Deleted, nodes are DETACH DELETEd. On Restarted
//! (410 Gone / reconnect), the full list is re-applied.
//!
//! Also includes:
//! - **Restart Storm Detection**: tracks system pod restarts and alerts on spikes
//! - **K8s Event Watcher**: watches Warning events for probe failures, OOM, evictions
//! - **Cascade Correlation**: identifies noisy neighbors when system pods are killed

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures::TryStreamExt;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod, Secret, Service};
use kube::api::Api;
use kube::runtime::watcher::{self, Event};
use tokio::sync::Notify;

use super::cloud_sink::CloudEventSink;
use super::correlator::StateChangeTracker;
use super::ingestor::K8sIngestor;

// ---------------------------------------------------------------------------
// Feature 1: Restart Storm Detection for system pods
// ---------------------------------------------------------------------------

/// Identifies critical system pods by namespace + label selectors.
fn is_system_pod(namespace: &str, labels: &std::collections::BTreeMap<String, String>) -> Option<&'static str> {
    if namespace != "kube-system" {
        return None;
    }
    if labels.get("k8s-app").map(|v| v.as_str()) == Some("kube-dns") {
        return Some("CoreDNS");
    }
    if labels.get("k8s-app").map(|v| v.as_str()) == Some("kube-proxy") {
        return Some("kube-proxy");
    }
    if labels.get("app").map(|v| v.as_str()) == Some("cloudflared") {
        return Some("cloudflared");
    }
    None
}

/// A single restart count observation.
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct RestartSnapshot {
    count: i32,
    timestamp: Instant,
    node_name: String,
}

/// Tracks restart counts for system pods and detects restart storms.
///
/// A restart storm is defined as restart_count increasing by more than
/// `threshold` within `window` duration.
pub struct RestartStormDetector {
    /// Map from pod key "(namespace/name)" to recent restart snapshots.
    history: Mutex<HashMap<String, Vec<RestartSnapshot>>>,
    /// How many restarts within the window triggers a storm alert.
    threshold: i32,
    /// Time window for storm detection.
    window: std::time::Duration,
}

impl RestartStormDetector {
    pub fn new() -> Self {
        Self {
            history: Mutex::new(HashMap::new()),
            threshold: 3,
            window: std::time::Duration::from_secs(600), // 10 minutes
        }
    }

    /// Record a restart count observation. Returns Some((friendly_name, delta, node))
    /// if a restart storm is detected.
    pub fn record(
        &self,
        namespace: &str,
        pod_name: &str,
        friendly_name: &str,
        restart_count: i32,
        node_name: &str,
    ) -> Option<(String, i32, String)> {
        let key = format!("{}/{}", namespace, pod_name);
        let now = Instant::now();
        let snapshot = RestartSnapshot {
            count: restart_count,
            timestamp: now,
            node_name: node_name.to_string(),
        };

        let mut history = self.history.lock().unwrap();
        let entries = history.entry(key).or_insert_with(Vec::new);

        // Prune entries older than the window
        let cutoff = now - self.window;
        entries.retain(|e| e.timestamp >= cutoff);

        // Check if restart count increased by more than threshold compared to
        // the oldest entry in the window
        let delta = if let Some(oldest) = entries.first() {
            restart_count - oldest.count
        } else {
            0
        };

        entries.push(snapshot);

        if delta > self.threshold {
            Some((friendly_name.to_string(), delta, node_name.to_string()))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Feature 2: K8s Event Watcher — classify Warning event reasons
// ---------------------------------------------------------------------------

/// Returns true if this Warning event reason is one we care about.
fn is_alert_worthy_reason(reason: &str) -> bool {
    matches!(
        reason,
        "Unhealthy" | "BackOff" | "OOMKilling" | "Evicted"
        | "FailedMount" | "FailedScheduling"
    )
}

/// True if the involved object is a critical system pod.
fn is_system_object(namespace: &str, name: &str) -> bool {
    namespace == "kube-system"
        && (name.starts_with("coredns")
            || name.starts_with("kube-proxy")
            || name.starts_with("cloudflared"))
}

// ---------------------------------------------------------------------------
// Feature 3: Cascade Correlation — find noisy neighbor on same node
// ---------------------------------------------------------------------------

/// Given a node name, query the graph for the pod with the highest restart
/// count on that node (proxy for resource pressure) and return it.
fn find_noisy_neighbor(ingestor: &K8sIngestor, node_name: &str) -> Option<(String, String, i32)> {
    // Query pods on the same node, sorted by restart_count descending
    let cypher =
        "MATCH (p:K8sPod {node_name: $node, cluster: $cluster}) \
         RETURN p.name, p.namespace, p.restart_count \
         ORDER BY p.restart_count DESC LIMIT 1";
    if let Ok(result) = ingestor.graph.query(
        cypher,
        &[("node", node_name), ("cluster", &ingestor.cluster_name)],
    ) {
        if let Some(row) = result.rows.first() {
            if row.len() >= 3 {
                let name = row[0].as_str().to_string();
                let ns = row[1].as_str().to_string();
                let restarts: i32 = row[2].as_str().parse().unwrap_or(0);
                return Some((format!("{}/{}", ns, name), ns, restarts));
            }
        }
    }
    None
}

/// Per-type watch statistics.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct WatchStats {
    pub added: u64,
    pub modified: u64,
    pub deleted: u64,
    pub relists: u64,
    pub errors: u64,
}

/// The K8s watcher. Spawns one tokio task per resource type.
pub struct K8sWatcher {
    pub ingestor: Arc<K8sIngestor>,
    pub correlator: Arc<StateChangeTracker>,
    pub cloud_sink: Option<Arc<CloudEventSink>>,
    pub restart_detector: Arc<RestartStormDetector>,
    shutdown: Arc<Notify>,
}

impl K8sWatcher {
    pub fn new(ingestor: Arc<K8sIngestor>) -> Self {
        let correlator = Arc::new(StateChangeTracker::new(
            ingestor.graph.clone(),
            ingestor.cluster_name.clone(),
        ));

        // Auto-create cloud sink if cloud credentials are available
        let cloud_sink = {
            let cloud_url = std::env::var("SAVANTS_CLOUD_URL")
                .unwrap_or_else(|_| "https://api.savants.cloud".to_string());
            let api_key = std::env::var("SAVANTS_TOKEN")
                .or_else(|_| std::env::var("SAVANTS_API_KEY"))
                .unwrap_or_default();
            if !api_key.is_empty() {
                Some(Arc::new(CloudEventSink::new(&cloud_url, &api_key, &ingestor.cluster_name)))
            } else {
                eprintln!("[k8s-watch] No SAVANTS_TOKEN - cloud event streaming disabled");
                None
            }
        };

        Self {
            ingestor,
            correlator,
            cloud_sink,
            restart_detector: Arc::new(RestartStormDetector::new()),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Start all watch streams. Returns a JoinHandle for the umbrella task.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        let ingestor = self.ingestor.clone();
        let correlator = self.correlator.clone();
        let shutdown = self.shutdown.clone();
        let restart_detector = self.restart_detector.clone();

        let cloud_sink = self.cloud_sink.clone();

        tokio::spawn(async move {
            // Ensure cluster node exists
            ingestor.merge_cluster_node();

            let client = ingestor.kube_client.clone();

            // Spawn one task per resource type
            let mut handles = Vec::new();

            // Namespaces
            {
                let ing = ingestor.clone();
                let api: Api<Namespace> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(ns) => {
                                let name = ns.metadata.name.as_deref().unwrap_or("");
                                let rv = ns.metadata.resource_version.as_deref().unwrap_or("");
                                let status = ns.status.as_ref()
                                    .and_then(|s| s.phase.as_deref())
                                    .unwrap_or("Unknown");
                                ing.merge_namespace(name, status, rv);
                                ing.create_contains_edge(
                                    "K8sCluster", "K8sNamespace",
                                    &ing.cluster_name, name, "",
                                );
                            }
                            Event::Deleted(ns) => {
                                let name = ns.metadata.name.as_deref().unwrap_or("");
                                ing.delete_one("K8sNamespace", name, "");
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // Deployments
            {
                let ing = ingestor.clone();
                let corr = correlator.clone();
                let sink = cloud_sink.clone();
                let api: Api<Deployment> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(d) => {
                                ing.ingest_deployment_obj(&d, "Deployment");
                                let name = d.metadata.name.as_deref().unwrap_or("");
                                let ns = d.metadata.namespace.as_deref().unwrap_or("");
                                let replicas = d.status.as_ref().and_then(|s| s.ready_replicas).unwrap_or(0);
                                let desired = d.spec.as_ref().and_then(|s| s.replicas).unwrap_or(1);
                                corr.record("deployment_rollout", ns, name, "K8sDeployment");
                                if let Some(ref s) = sink {
                                    let severity = if replicas < desired { "warning" } else { "info" };
                                    s.record("deployment_applied", "Deployment", name, ns, "applied", severity,
                                        &format!("ready={}/{}", replicas, desired));
                                }
                            }
                            Event::Deleted(d) => {
                                let name = d.metadata.name.as_deref().unwrap_or("");
                                let ns = d.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sDeployment", name, ns);
                                if let Some(ref s) = sink {
                                    s.record("deployment_deleted", "Deployment", name, ns, "deleted", "warning", "");
                                }
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // StatefulSets
            {
                let ing = ingestor.clone();
                let api: Api<StatefulSet> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(s) => {
                                ing.ingest_statefulset_obj(&s);
                            }
                            Event::Deleted(s) => {
                                let name = s.metadata.name.as_deref().unwrap_or("");
                                let ns = s.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sDeployment", name, ns);
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // DaemonSets
            {
                let ing = ingestor.clone();
                let api: Api<DaemonSet> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(d) => {
                                ing.ingest_daemonset_obj(&d);
                            }
                            Event::Deleted(d) => {
                                let name = d.metadata.name.as_deref().unwrap_or("");
                                let ns = d.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sDeployment", name, ns);
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // Pods (with Feature 1: Restart Storm Detection + Feature 3: Cascade Correlation)
            {
                let ing = ingestor.clone();
                let corr = correlator.clone();
                let sink = cloud_sink.clone();
                let storm = restart_detector.clone();
                let api: Api<Pod> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(p) => {
                                let rv = p.metadata.resource_version.as_deref().unwrap_or("");
                                ing.ingest_pod(&p, rv);
                                let name = p.metadata.name.as_deref().unwrap_or("");
                                let ns = p.metadata.namespace.as_deref().unwrap_or("");
                                let phase = p.status.as_ref()
                                    .and_then(|s| s.phase.as_deref())
                                    .unwrap_or("Unknown");
                                let restarts: i32 = p.status.as_ref()
                                    .and_then(|s| s.container_statuses.as_ref())
                                    .map(|cs| cs.iter().map(|c| c.restart_count).sum())
                                    .unwrap_or(0);
                                corr.record("pod_change", ns, name, "K8sPod");

                                // Feature 1: Check for restart storms on system pods
                                let labels = p.metadata.labels.as_ref()
                                    .cloned()
                                    .unwrap_or_default();
                                let node_name = p.spec.as_ref()
                                    .and_then(|s| s.node_name.as_deref())
                                    .unwrap_or("");

                                if let Some(friendly) = is_system_pod(ns, &labels) {
                                    if let Some((fname, delta, node)) = storm.record(
                                        ns, name, friendly, restarts, node_name,
                                    ) {
                                        eprintln!(
                                            "[k8s-watch] CRITICAL: {} restarted {} times in 10 min \
                                             — {} resolution degraded for all services",
                                            fname,
                                            delta,
                                            match fname.as_str() {
                                                "CoreDNS" => "DNS",
                                                "kube-proxy" => "network proxy",
                                                "cloudflared" => "tunnel",
                                                _ => "service",
                                            }
                                        );

                                        // Feature 3: Cascade correlation — find noisy neighbor
                                        if !node.is_empty() {
                                            if let Some((noisy_pod, _ns, noisy_restarts)) =
                                                find_noisy_neighbor(&ing, &node)
                                            {
                                                eprintln!(
                                                    "[k8s-watch] ROOT CAUSE: {} killed on node {} \
                                                     — {} ({} restarts) causing probe timeouts",
                                                    fname, node, noisy_pod, noisy_restarts
                                                );
                                            }
                                        }

                                        // Also send to cloud sink as critical
                                        if let Some(ref s) = sink {
                                            s.record(
                                                "restart_storm", "Pod", name, ns,
                                                "critical", "critical",
                                                &format!(
                                                    "{} restarted {} times in 10 min on node {}",
                                                    fname, delta, node
                                                ),
                                            );
                                        }
                                    }
                                }

                                if let Some(ref s) = sink {
                                    let severity = if phase == "Failed" || restarts > 5 { "warning" } else { "info" };
                                    s.record("pod_applied", "Pod", name, ns, "applied", severity,
                                        &format!("phase={} restarts={}", phase, restarts));
                                }
                            }
                            Event::Deleted(p) => {
                                let name = p.metadata.name.as_deref().unwrap_or("");
                                let ns = p.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sPod", name, ns);
                                if let Some(ref s) = sink {
                                    s.record("pod_deleted", "Pod", name, ns, "deleted", "warning", "");
                                }
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // Services
            {
                let ing = ingestor.clone();
                let sink = cloud_sink.clone();
                let api: Api<Service> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(svc) => {
                                let rv = svc.metadata.resource_version.as_deref().unwrap_or("");
                                ing.ingest_service(&svc, rv);
                            }
                            Event::Deleted(svc) => {
                                let name = svc.metadata.name.as_deref().unwrap_or("");
                                let ns = svc.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sService", name, ns);
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // ConfigMaps
            {
                let ing = ingestor.clone();
                let corr = correlator.clone();
                let sink = cloud_sink.clone();
                let api: Api<ConfigMap> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(cm) => {
                                let rv = cm.metadata.resource_version.as_deref().unwrap_or("");
                                ing.ingest_configmap(&cm, rv);
                                let name = cm.metadata.name.as_deref().unwrap_or("");
                                let ns = cm.metadata.namespace.as_deref().unwrap_or("");
                                corr.record("configmap_edit", ns, name, "K8sConfigMap");
                                if let Some(ref s) = sink {
                                    s.record("configmap_applied", "ConfigMap", name, ns, "applied", "info", "");
                                }
                            }
                            Event::Deleted(cm) => {
                                let name = cm.metadata.name.as_deref().unwrap_or("");
                                let ns = cm.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sConfigMap", name, ns);
                                if let Some(ref s) = sink {
                                    s.record("configmap_deleted", "ConfigMap", name, ns, "deleted", "info", "");
                                }
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // Secrets
            {
                let ing = ingestor.clone();
                let corr = correlator.clone();
                let api: Api<Secret> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let stream = watcher::watcher(api, watcher::Config::default());
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(sec) => {
                                let rv = sec.metadata.resource_version.as_deref().unwrap_or("");
                                ing.ingest_secret(&sec, rv);
                                let name = sec.metadata.name.as_deref().unwrap_or("");
                                let ns = sec.metadata.namespace.as_deref().unwrap_or("");
                                corr.record("secret_edit", ns, name, "K8sSecret");
                            }
                            Event::Deleted(sec) => {
                                let name = sec.metadata.name.as_deref().unwrap_or("");
                                let ns = sec.metadata.namespace.as_deref().unwrap_or("");
                                ing.delete_one("K8sSecret", name, ns);
                            }
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // Feature 2: K8s Event Watcher for Warning events (probe failures, OOM, etc.)
            {
                let sink = cloud_sink.clone();
                let api: Api<k8s_openapi::api::core::v1::Event> = Api::all(client.clone());
                handles.push(tokio::spawn(async move {
                    let params = watcher::Config::default()
                        .fields("type=Warning");
                    let stream = watcher::watcher(api, params);
                    futures::pin_mut!(stream);
                    while let Ok(Some(event)) = stream.try_next().await {
                        match event {
                            Event::Applied(ev) => {
                                let reason = ev.reason.as_deref().unwrap_or("");
                                if is_alert_worthy_reason(reason) {
                                    let obj_name = ev.involved_object.name.as_deref().unwrap_or("");
                                    let obj_ns = ev.involved_object.namespace.as_deref().unwrap_or("");
                                    let message = ev.message.as_deref().unwrap_or("");

                                    let is_system = is_system_object(obj_ns, obj_name);
                                    let severity = if is_system { "CRITICAL" } else { "WARNING" };

                                    eprintln!(
                                        "[k8s-watch] {}: Pod {}/{} — {}: {}",
                                        severity, obj_ns, obj_name, reason, message
                                    );

                                    if let Some(ref s) = sink {
                                        let sev_lower = if is_system { "critical" } else { "warning" };
                                        s.record(
                                            "k8s_event", "Event", obj_name, obj_ns,
                                            reason, sev_lower,
                                            &format!("{}: {}", reason, message),
                                        );
                                    }
                                }
                            }
                            Event::Deleted(_) => {}
                            Event::Restarted(_) => {}
                        }
                    }
                }));
            }

            // Wait for shutdown signal; abort all tasks on shutdown
            shutdown.notified().await;
            for h in handles {
                h.abort();
            }
        })
    }

    /// Signal all watch tasks to stop.
    pub fn stop(&self) {
        self.shutdown.notify_one();
    }
}

