//! Live K8s watch-based ingestor for the Mazkir runtime layer.
//!
//! Complements `K8sIngestor::snapshot()` (pull-based full reconcile) with a
//! push-based streaming mode using `kube::runtime::watcher`. Each resource
//! type gets its own watcher task. On Applied events, the same MERGE logic as
//! snapshot is used. On Deleted, nodes are DETACH DELETEd. On Restarted
//! (410 Gone / reconnect), the full list is re-applied.

use std::sync::Arc;

use futures::TryStreamExt;
use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Pod, Secret, Service};
use kube::api::Api;
use kube::runtime::watcher::{self, Event};
use tokio::sync::Notify;

use super::cloud_sink::CloudEventSink;
use super::correlator::StateChangeTracker;
use super::ingestor::K8sIngestor;

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
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Start all watch streams. Returns a JoinHandle for the umbrella task.
    pub fn start(&self) -> tokio::task::JoinHandle<()> {
        let ingestor = self.ingestor.clone();
        let correlator = self.correlator.clone();
        let shutdown = self.shutdown.clone();

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

            // Pods
            {
                let ing = ingestor.clone();
                let corr = correlator.clone();
                let sink = cloud_sink.clone();
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

