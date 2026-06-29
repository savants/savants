//! Snapshot-based K8s ingestor: full diff-based reconciliation.
//!
//! Lists all resource types from the cluster, compares `resourceVersion`
//! with what is stored in the graph, and only MERGEs resources that are
//! new or changed. Resources in the graph but gone from the cluster are
//! DETACH DELETEd (stale cleanup).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Instant;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{
    ConfigMap, Namespace, Pod, Secret, Service,
};
use kube::api::{Api, ListParams};
use kube::Client;

use crate::graph::GraphClient;

/// Per-type diff counters.
#[derive(Debug, Default, Clone)]
pub struct DiffCounts {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub deleted: u32,
}

impl fmt::Display for DiffCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "+{} ~{} ={} -{}",
            self.added, self.updated, self.unchanged, self.deleted
        )
    }
}

/// Aggregate stats for a single snapshot run.
#[derive(Debug, Default, Clone)]
pub struct IngestStats {
    pub cluster: String,
    pub elapsed_seconds: f64,
    pub namespaces: DiffCounts,
    pub deployments: DiffCounts,
    pub pods: DiffCounts,
    pub services: DiffCounts,
    pub configmaps: DiffCounts,
    pub secrets: DiffCounts,
    pub edges_created: u32,
    pub errors: Vec<String>,
}

impl IngestStats {
    pub fn summary(&self) -> String {
        let mut lines = vec![
            format!(
                "K8s ingest complete for cluster '{}' in {:.1}s",
                self.cluster, self.elapsed_seconds
            ),
            format!("  Namespaces:   {}", self.namespaces),
            format!("  Deployments:  {}", self.deployments),
            format!("  Pods:         {}", self.pods),
            format!("  Services:     {}", self.services),
            format!("  ConfigMaps:   {}", self.configmaps),
            format!("  Secrets:      {}", self.secrets),
            format!("  Edges:        {}", self.edges_created),
        ];
        if !self.errors.is_empty() {
            lines.push(format!("  ! {} errors during ingest:", self.errors.len()));
            for e in self.errors.iter().take(5) {
                lines.push(format!("    - {}", e));
            }
        }
        lines.join("\n")
    }
}

/// Key type for (name, namespace) resource identity.
type ResKey = (String, String);

/// The main K8s ingestor. Owns a kube Client and a GraphClient.
pub struct K8sIngestor {
    pub graph: GraphClient,
    pub cluster_name: String,
    pub kube_client: Client,
}

impl K8sIngestor {
    /// Create a new ingestor from existing kube and graph clients.
    pub fn new(graph: GraphClient, cluster_name: String, kube_client: Client) -> Self {
        Self {
            graph,
            cluster_name,
            kube_client,
        }
    }

    /// Build a kube::Client from default kubeconfig or in-cluster config.
    pub async fn kube_client_from_kubeconfig(
        context: Option<&str>,
    ) -> Result<Client, Box<dyn std::error::Error>> {
        let mut config = if let Some(ctx) = context {
            let opts = kube::config::KubeConfigOptions {
                context: Some(ctx.to_string()),
                ..Default::default()
            };
            kube::Config::from_kubeconfig(&opts).await?
        } else {
            kube::Config::infer().await?
        };
        // Reasonable timeout for list calls
        config.read_timeout = Some(std::time::Duration::from_secs(30));
        Ok(Client::try_from(config)?)
    }

    /// Take a diff-based snapshot of the cluster and apply the delta.
    ///
    /// Uses K8s `metadata.resource_version` to detect changes: unchanged
    /// resources are skipped, changed resources are re-merged, and resources
    /// that disappeared from the cluster are deleted from the graph.
    pub async fn snapshot(&self) -> IngestStats {
        let t0 = Instant::now();
        let mut stats = IngestStats {
            cluster: self.cluster_name.clone(),
            ..Default::default()
        };

        // Load existing resource versions from graph for each type
        let existing = self.load_existing_rv_maps();
        let mut seen: HashMap<&str, HashSet<ResKey>> = HashMap::new();
        for label in &[
            "K8sNamespace",
            "K8sDeployment",
            "K8sPod",
            "K8sService",
            "K8sConfigMap",
            "K8sSecret",
        ] {
            seen.insert(label, HashSet::new());
        }

        // 1. Cluster node
        self.merge_cluster_node();

        // 2. Namespaces
        let ns_api: Api<Namespace> = Api::all(self.kube_client.clone());
        match ns_api.list(&ListParams::default()).await {
            Ok(ns_list) => {
                for ns in &ns_list {
                    let meta = match &ns.metadata.name {
                        Some(name) => name.clone(),
                        None => continue,
                    };
                    let key: ResKey = (meta.clone(), String::new());
                    seen.get_mut("K8sNamespace").unwrap().insert(key.clone());

                    let rv = ns
                        .metadata
                        .resource_version
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    let prev = existing.get("K8sNamespace").and_then(|m| m.get(&key));

                    match prev {
                        None => stats.namespaces.added += 1,
                        Some(prv) if prv == &rv => {
                            stats.namespaces.unchanged += 1;
                            continue;
                        }
                        Some(_) => stats.namespaces.updated += 1,
                    }

                    let status = ns
                        .status
                        .as_ref()
                        .and_then(|s| s.phase.as_deref())
                        .unwrap_or("Unknown");

                    self.merge_namespace(&meta, status, &rv);
                    self.create_contains_edge("K8sCluster", "K8sNamespace", &self.cluster_name, &meta, "");
                    stats.edges_created += 1;
                }
            }
            Err(e) => {
                stats.errors.push(format!("list_namespace: {}", e));
                stats.elapsed_seconds = t0.elapsed().as_secs_f64();
                return stats;
            }
        }

        // 3. Deployments
        let deploy_api: Api<Deployment> = Api::all(self.kube_client.clone());
        match deploy_api.list(&ListParams::default()).await {
            Ok(list) => {
                for d in &list {
                    if let Err(e) = self.diff_workload(
                        d.metadata.name.as_deref(),
                        d.metadata.namespace.as_deref(),
                        d.metadata.resource_version.as_deref(),
                        "Deployment",
                        &existing,
                        seen.get_mut("K8sDeployment").unwrap(),
                        &mut stats.deployments,
                        || self.ingest_deployment_obj(d, "Deployment"),
                    ) {
                        stats.errors.push(e);
                    } else {
                        stats.edges_created += 1;
                    }
                }
            }
            Err(e) => stats.errors.push(format!("list_deployments: {}", e)),
        }

        // StatefulSets
        let sts_api: Api<StatefulSet> = Api::all(self.kube_client.clone());
        match sts_api.list(&ListParams::default()).await {
            Ok(list) => {
                for s in &list {
                    if let Err(e) = self.diff_workload(
                        s.metadata.name.as_deref(),
                        s.metadata.namespace.as_deref(),
                        s.metadata.resource_version.as_deref(),
                        "StatefulSet",
                        &existing,
                        seen.get_mut("K8sDeployment").unwrap(),
                        &mut stats.deployments,
                        || self.ingest_statefulset_obj(s),
                    ) {
                        stats.errors.push(e);
                    }
                }
            }
            Err(e) => stats.errors.push(format!("list_statefulsets: {}", e)),
        }

        // DaemonSets
        let ds_api: Api<DaemonSet> = Api::all(self.kube_client.clone());
        match ds_api.list(&ListParams::default()).await {
            Ok(list) => {
                for d in &list {
                    if let Err(e) = self.diff_workload(
                        d.metadata.name.as_deref(),
                        d.metadata.namespace.as_deref(),
                        d.metadata.resource_version.as_deref(),
                        "DaemonSet",
                        &existing,
                        seen.get_mut("K8sDeployment").unwrap(),
                        &mut stats.deployments,
                        || self.ingest_daemonset_obj(d),
                    ) {
                        stats.errors.push(e);
                    }
                }
            }
            Err(e) => stats.errors.push(format!("list_daemonsets: {}", e)),
        }

        // 4. Pods
        let pod_api: Api<Pod> = Api::all(self.kube_client.clone());
        match pod_api.list(&ListParams::default()).await {
            Ok(list) => {
                for p in &list {
                    let name = match &p.metadata.name {
                        Some(n) => n.clone(),
                        None => continue,
                    };
                    let ns = p.metadata.namespace.as_deref().unwrap_or("").to_string();
                    let key: ResKey = (name.clone(), ns.clone());
                    seen.get_mut("K8sPod").unwrap().insert(key.clone());

                    let rv = p
                        .metadata
                        .resource_version
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    let prev = existing.get("K8sPod").and_then(|m| m.get(&key));

                    match prev {
                        None => stats.pods.added += 1,
                        Some(prv) if prv == &rv => {
                            stats.pods.unchanged += 1;
                            continue;
                        }
                        Some(_) => stats.pods.updated += 1,
                    }

                    self.ingest_pod(p, &rv);
                    stats.edges_created += 1;
                }
            }
            Err(e) => stats.errors.push(format!("list_pods: {}", e)),
        }

        // 5. Services
        let svc_api: Api<Service> = Api::all(self.kube_client.clone());
        match svc_api.list(&ListParams::default()).await {
            Ok(list) => {
                for svc in &list {
                    let name = match &svc.metadata.name {
                        Some(n) => n.clone(),
                        None => continue,
                    };
                    let ns = svc.metadata.namespace.as_deref().unwrap_or("").to_string();
                    let key: ResKey = (name.clone(), ns.clone());
                    seen.get_mut("K8sService").unwrap().insert(key.clone());

                    let rv = svc
                        .metadata
                        .resource_version
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    let prev = existing.get("K8sService").and_then(|m| m.get(&key));

                    match prev {
                        None => stats.services.added += 1,
                        Some(prv) if prv == &rv => {
                            stats.services.unchanged += 1;
                            continue;
                        }
                        Some(_) => stats.services.updated += 1,
                    }

                    self.ingest_service(svc, &rv);
                    stats.edges_created += 1;
                }
            }
            Err(e) => stats.errors.push(format!("list_services: {}", e)),
        }

        // 6. ConfigMaps (names + key names only, never values)
        let cm_api: Api<ConfigMap> = Api::all(self.kube_client.clone());
        match cm_api.list(&ListParams::default()).await {
            Ok(list) => {
                for cm in &list {
                    let name = match &cm.metadata.name {
                        Some(n) => n.clone(),
                        None => continue,
                    };
                    let ns = cm.metadata.namespace.as_deref().unwrap_or("").to_string();
                    let key: ResKey = (name.clone(), ns.clone());
                    seen.get_mut("K8sConfigMap").unwrap().insert(key.clone());

                    let rv = cm
                        .metadata
                        .resource_version
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    let prev = existing.get("K8sConfigMap").and_then(|m| m.get(&key));

                    match prev {
                        None => stats.configmaps.added += 1,
                        Some(prv) if prv == &rv => {
                            stats.configmaps.unchanged += 1;
                            continue;
                        }
                        Some(_) => stats.configmaps.updated += 1,
                    }

                    self.ingest_configmap(cm, &rv);
                    stats.edges_created += 1;
                }
            }
            Err(e) => stats.errors.push(format!("list_configmaps: {}", e)),
        }

        // 7. Secrets (names + key names + type, NEVER values)
        let sec_api: Api<Secret> = Api::all(self.kube_client.clone());
        match sec_api.list(&ListParams::default()).await {
            Ok(list) => {
                for sec in &list {
                    let name = match &sec.metadata.name {
                        Some(n) => n.clone(),
                        None => continue,
                    };
                    let ns = sec.metadata.namespace.as_deref().unwrap_or("").to_string();
                    let key: ResKey = (name.clone(), ns.clone());
                    seen.get_mut("K8sSecret").unwrap().insert(key.clone());

                    let rv = sec
                        .metadata
                        .resource_version
                        .as_deref()
                        .unwrap_or("")
                        .to_string();
                    let prev = existing.get("K8sSecret").and_then(|m| m.get(&key));

                    match prev {
                        None => stats.secrets.added += 1,
                        Some(prv) if prv == &rv => {
                            stats.secrets.unchanged += 1;
                            continue;
                        }
                        Some(_) => stats.secrets.updated += 1,
                    }

                    self.ingest_secret(sec, &rv);
                    stats.edges_created += 1;
                }
            }
            Err(e) => stats.errors.push(format!("list_secrets: {}", e)),
        }

        // 8. Delete stale nodes (in graph but gone from cluster)
        let label_counts: Vec<(&str, &mut DiffCounts)> = vec![];
        // We need mutable access to stats fields, so handle one by one
        let stale_labels = [
            "K8sNamespace",
            "K8sDeployment",
            "K8sPod",
            "K8sService",
            "K8sConfigMap",
            "K8sSecret",
        ];
        for label in &stale_labels {
            if let (Some(ex), Some(sn)) = (existing.get(*label), seen.get(*label)) {
                let stale: HashSet<&ResKey> = ex.keys().filter(|k| !sn.contains(*k)).collect();
                let deleted = self.delete_stale(label, &stale);
                match *label {
                    "K8sNamespace" => stats.namespaces.deleted = deleted,
                    "K8sDeployment" => stats.deployments.deleted = deleted,
                    "K8sPod" => stats.pods.deleted = deleted,
                    "K8sService" => stats.services.deleted = deleted,
                    "K8sConfigMap" => stats.configmaps.deleted = deleted,
                    "K8sSecret" => stats.secrets.deleted = deleted,
                    _ => {}
                }
            }
        }
        let _ = label_counts; // suppress unused warning

        stats.elapsed_seconds = t0.elapsed().as_secs_f64();
        stats
    }

    // ------------------------------------------------------------------
    // Graph query helpers
    // ------------------------------------------------------------------

    /// Load {(name, namespace): resource_version} for a K8s label in graph.
    fn current_rv_map(&self, label: &str) -> HashMap<ResKey, String> {
        let cypher = format!(
            "MATCH (x:{} {{cluster: $cluster}}) RETURN x.name, x.namespace, x.resource_version",
            label
        );
        let mut out = HashMap::new();
        if let Ok(result) = self.graph.query(&cypher, &[("cluster", &self.cluster_name)]) {
            for row in &result.rows {
                if row.len() >= 3 {
                    let name = row[0].as_str().to_string();
                    let ns = row[1].as_str().to_string();
                    let rv = row[2].as_str().to_string();
                    out.insert((name, ns), rv);
                }
            }
        }
        out
    }

    /// Load all existing RV maps at once for all resource types.
    fn load_existing_rv_maps(&self) -> HashMap<&'static str, HashMap<ResKey, String>> {
        let mut m = HashMap::new();
        for label in &[
            "K8sNamespace",
            "K8sDeployment",
            "K8sPod",
            "K8sService",
            "K8sConfigMap",
            "K8sSecret",
        ] {
            m.insert(*label, self.current_rv_map(label));
        }
        m
    }

    /// Delete a single resource from the graph.
    pub fn delete_one(&self, label: &str, name: &str, namespace: &str) {
        let cypher = format!(
            "MATCH (x:{} {{name: $name, namespace: $ns, cluster: $cluster}}) DETACH DELETE x",
            label
        );
        let _ = self.graph.query(
            &cypher,
            &[
                ("name", name),
                ("ns", namespace),
                ("cluster", &self.cluster_name),
            ],
        );
    }

    /// Delete stale nodes (in graph but gone from cluster). Returns count.
    fn delete_stale(&self, label: &str, keys: &HashSet<&ResKey>) -> u32 {
        let mut n = 0u32;
        for (name, ns) in keys {
            self.delete_one(label, name, ns);
            n += 1;
        }
        n
    }

    /// Create the top-level K8sCluster node.
    pub(crate) fn merge_cluster_node(&self) {
        let _ = self.graph.query(
            "MERGE (c:K8sCluster {name: $name}) \
             SET c.context = $context",
            &[
                ("name", &self.cluster_name),
                ("context", &self.cluster_name),
            ],
        );
    }

    /// MERGE a namespace node.
    pub(crate) fn merge_namespace(&self, name: &str, status: &str, rv: &str) {
        let _ = self.graph.query(
            "MERGE (n:K8sNamespace {name: $name, cluster: $cluster}) \
             SET n.status = $status, n.resource_version = $rv",
            &[
                ("name", name),
                ("cluster", &self.cluster_name),
                ("status", status),
                ("rv", rv),
            ],
        );
    }

    /// Create a CONTAINS edge between parent and child nodes.
    pub(crate) fn create_contains_edge(
        &self,
        parent_label: &str,
        child_label: &str,
        parent_name: &str,
        child_name: &str,
        child_ns: &str,
    ) {
        // For cluster→namespace, parent key is just name
        // For namespace→resource, parent key is name + cluster, child has namespace
        if parent_label == "K8sCluster" {
            let cypher = format!(
                "MATCH (p:{} {{name: $pname}}) \
                 MATCH (c:{} {{name: $cname, cluster: $cluster}}) \
                 MERGE (p)-[:CONTAINS]->(c)",
                parent_label, child_label
            );
            let _ = self.graph.query(
                &cypher,
                &[
                    ("pname", parent_name),
                    ("cname", child_name),
                    ("cluster", &self.cluster_name),
                ],
            );
        } else {
            let cypher = format!(
                "MATCH (p:{} {{name: $ns, cluster: $cluster}}) \
                 MATCH (c:{} {{name: $cname, namespace: $ns, cluster: $cluster}}) \
                 MERGE (p)-[:CONTAINS]->(c)",
                parent_label, child_label
            );
            let _ = self.graph.query(
                &cypher,
                &[
                    ("ns", child_ns),
                    ("cname", child_name),
                    ("cluster", &self.cluster_name),
                ],
            );
        }
    }

    // ------------------------------------------------------------------
    // Diff helper for workloads (Deployment/StatefulSet/DaemonSet)
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn diff_workload(
        &self,
        name: Option<&str>,
        namespace: Option<&str>,
        resource_version: Option<&str>,
        kind: &str,
        existing: &HashMap<&str, HashMap<ResKey, String>>,
        seen: &mut HashSet<ResKey>,
        counts: &mut DiffCounts,
        ingest_fn: impl FnOnce(),
    ) -> Result<(), String> {
        let name = name.ok_or_else(|| format!("{}: missing name", kind))?;
        let ns = namespace.unwrap_or("");
        let key: ResKey = (name.to_string(), ns.to_string());
        seen.insert(key.clone());

        let rv = resource_version.unwrap_or("").to_string();
        let prev = existing.get("K8sDeployment").and_then(|m| m.get(&key));

        match prev {
            None => counts.added += 1,
            Some(prv) if prv == &rv => {
                counts.unchanged += 1;
                return Ok(());
            }
            Some(_) => counts.updated += 1,
        }

        ingest_fn();
        Ok(())
    }

    // ------------------------------------------------------------------
    // Per-resource ingest helpers
    // ------------------------------------------------------------------

    /// Ingest a Deployment object.
    pub(crate) fn ingest_deployment_obj(&self, d: &Deployment, kind: &str) {
        let meta = &d.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");
        let rv = meta.resource_version.as_deref().unwrap_or("");

        let spec = d.spec.as_ref();
        let status = d.status.as_ref();

        let replicas_desired = spec.and_then(|s| s.replicas).unwrap_or(0);
        let replicas_ready = status.and_then(|s| s.ready_replicas).unwrap_or(0);
        let replicas_available = status.and_then(|s| s.available_replicas).unwrap_or(0);

        // Primary container image
        let image = spec
            .and_then(|s| s.template.spec.as_ref())
            .and_then(|ps| ps.containers.first())
            .and_then(|c| c.image.as_deref())
            .unwrap_or("");

        let labels = flatten_labels(meta.labels.as_ref());

        self.merge_deployment(name, ns, kind, replicas_desired, replicas_ready, replicas_available, image, &labels, rv);
        self.create_contains_edge("K8sNamespace", "K8sDeployment", "", name, ns);
    }

    /// Ingest a StatefulSet (mapped to K8sDeployment with kind=StatefulSet).
    pub(crate) fn ingest_statefulset_obj(&self, s: &StatefulSet) {
        let meta = &s.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");
        let rv = meta.resource_version.as_deref().unwrap_or("");

        let spec = s.spec.as_ref();
        let status = s.status.as_ref();

        let replicas_desired = spec.and_then(|s| s.replicas).unwrap_or(0);
        let replicas_ready = status.map(|s| s.ready_replicas.unwrap_or(0)).unwrap_or(0);
        let replicas_available = status.and_then(|s| s.available_replicas).unwrap_or(0);

        let image = spec
            .and_then(|s| s.template.spec.as_ref())
            .and_then(|ps| ps.containers.first())
            .and_then(|c| c.image.as_deref())
            .unwrap_or("");

        let labels = flatten_labels(meta.labels.as_ref());

        self.merge_deployment(name, ns, "StatefulSet", replicas_desired, replicas_ready, replicas_available, image, &labels, rv);
        self.create_contains_edge("K8sNamespace", "K8sDeployment", "", name, ns);
    }

    /// Ingest a DaemonSet (mapped to K8sDeployment with kind=DaemonSet).
    pub(crate) fn ingest_daemonset_obj(&self, d: &DaemonSet) {
        let meta = &d.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");
        let rv = meta.resource_version.as_deref().unwrap_or("");

        let status = d.status.as_ref();
        let replicas_desired = status.map(|s| s.desired_number_scheduled).unwrap_or(0);
        let replicas_ready = status.map(|s| s.number_ready).unwrap_or(0);
        let replicas_available = status.and_then(|s| s.number_available).unwrap_or(0);

        let image = d
            .spec
            .as_ref()
            .and_then(|s| s.template.spec.as_ref())
            .and_then(|ps| ps.containers.first())
            .and_then(|c| c.image.as_deref())
            .unwrap_or("");

        let labels = flatten_labels(meta.labels.as_ref());

        self.merge_deployment(name, ns, "DaemonSet", replicas_desired, replicas_ready, replicas_available, image, &labels, rv);
        self.create_contains_edge("K8sNamespace", "K8sDeployment", "", name, ns);
    }

    /// MERGE a deployment/statefulset/daemonset node.
    #[allow(clippy::too_many_arguments)]
    fn merge_deployment(
        &self,
        name: &str,
        namespace: &str,
        kind: &str,
        replicas_desired: i32,
        replicas_ready: i32,
        replicas_available: i32,
        image: &str,
        labels: &str,
        rv: &str,
    ) {
        let desired_s = replicas_desired.to_string();
        let ready_s = replicas_ready.to_string();
        let avail_s = replicas_available.to_string();

        let _ = self.graph.query(
            "MERGE (d:K8sDeployment {name: $name, namespace: $ns, cluster: $cluster}) \
             SET d.kind = $kind, \
                 d.replicas_desired = $desired, \
                 d.replicas_ready = $ready, \
                 d.replicas_available = $avail, \
                 d.image = $image, \
                 d.labels = $labels, \
                 d.resource_version = $rv",
            &[
                ("name", name),
                ("ns", namespace),
                ("cluster", &self.cluster_name),
                ("kind", kind),
                ("desired", &desired_s),
                ("ready", &ready_s),
                ("avail", &avail_s),
                ("image", image),
                ("labels", labels),
                ("rv", rv),
            ],
        );
    }

    /// Ingest a Pod.
    pub fn ingest_pod(&self, p: &Pod, rv: &str) {
        let meta = &p.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");
        let spec = p.spec.as_ref();
        let status = p.status.as_ref();

        // Primary container image
        let image = spec
            .and_then(|s| s.containers.first())
            .and_then(|c| c.image.as_deref())
            .unwrap_or("");

        // Restart count and readiness
        let mut restart_count: i32 = 0;
        let mut ready = true;
        if let Some(st) = status {
            if let Some(css) = &st.container_statuses {
                restart_count = css.iter().map(|cs| cs.restart_count).sum();
                ready = css.iter().all(|cs| cs.ready);
            }
        }

        // Phase with CrashLoopBackOff detection
        let mut phase = status
            .and_then(|s| s.phase.as_deref())
            .unwrap_or("Unknown")
            .to_string();

        if let Some(st) = status {
            if let Some(css) = &st.container_statuses {
                for cs in css {
                    if let Some(state) = &cs.state {
                        if let Some(waiting) = &state.waiting {
                            if let Some(reason) = &waiting.reason {
                                if matches!(
                                    reason.as_str(),
                                    "CrashLoopBackOff" | "ImagePullBackOff" | "ErrImagePull"
                                ) {
                                    phase = reason.clone();
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Owner reference
        let mut owner_kind = String::new();
        let mut owner_name = String::new();
        if let Some(refs) = &meta.owner_references {
            if let Some(first) = refs.first() {
                owner_kind = first.kind.clone();
                owner_name = first.name.clone();
            }
        }

        let node_name = spec.and_then(|s| s.node_name.as_deref()).unwrap_or("");
        let restart_s = restart_count.to_string();
        let ready_s = if ready { "true" } else { "false" };

        let _ = self.graph.query(
            "MERGE (p:K8sPod {name: $name, namespace: $ns, cluster: $cluster}) \
             SET p.status = $status, \
                 p.node_name = $node_name, \
                 p.restart_count = $restart_count, \
                 p.ready = $ready, \
                 p.image = $image, \
                 p.owner_kind = $owner_kind, \
                 p.owner_name = $owner_name, \
                 p.resource_version = $rv",
            &[
                ("name", name),
                ("ns", ns),
                ("cluster", &self.cluster_name),
                ("status", &phase),
                ("node_name", node_name),
                ("restart_count", &restart_s),
                ("ready", ready_s),
                ("image", image),
                ("owner_kind", &owner_kind),
                ("owner_name", &owner_name),
                ("rv", rv),
            ],
        );

        // Edge: Namespace CONTAINS Pod
        self.create_contains_edge("K8sNamespace", "K8sPod", "", name, ns);

        // Edge: Pod READS ConfigMap / Secret (from volumes + envFrom)
        self.link_pod_dependencies(p);
    }

    /// Create READS edges from a Pod to the ConfigMaps/Secrets it uses.
    fn link_pod_dependencies(&self, p: &Pod) {
        let meta = &p.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");
        let spec = match p.spec.as_ref() {
            Some(s) => s,
            None => return,
        };

        let mut configmap_refs: HashSet<String> = HashSet::new();
        let mut secret_refs: HashSet<String> = HashSet::new();

        // Volume-based refs
        if let Some(volumes) = &spec.volumes {
            for v in volumes {
                if let Some(cm) = &v.config_map {
                    if let Some(cm_name) = &cm.name {
                        configmap_refs.insert(cm_name.clone());
                    }
                }
                if let Some(sec) = &v.secret {
                    if let Some(sec_name) = &sec.secret_name {
                        secret_refs.insert(sec_name.clone());
                    }
                }
            }
        }

        // envFrom refs on each container
        for c in &spec.containers {
            if let Some(env_from) = &c.env_from {
                for ef in env_from {
                    if let Some(cm_ref) = &ef.config_map_ref {
                        if let Some(cm_name) = &cm_ref.name {
                            configmap_refs.insert(cm_name.clone());
                        }
                    }
                    if let Some(sec_ref) = &ef.secret_ref {
                        if let Some(sec_name) = &sec_ref.name {
                            secret_refs.insert(sec_name.clone());
                        }
                    }
                }
            }
            if let Some(env) = &c.env {
                for e in env {
                    if let Some(vf) = &e.value_from {
                        if let Some(cm_key) = &vf.config_map_key_ref {
                            if let Some(cm_name) = &cm_key.name {
                                configmap_refs.insert(cm_name.clone());
                            }
                        }
                        if let Some(sec_key) = &vf.secret_key_ref {
                            if let Some(sec_name) = &sec_key.name {
                                secret_refs.insert(sec_name.clone());
                            }
                        }
                    }
                }
            }
        }

        for cm_name in &configmap_refs {
            let _ = self.graph.query(
                "MATCH (p:K8sPod {name: $pod, namespace: $ns, cluster: $cluster}) \
                 MERGE (cm:K8sConfigMap {name: $cm, namespace: $ns, cluster: $cluster}) \
                 MERGE (p)-[:READS]->(cm)",
                &[
                    ("pod", name),
                    ("ns", ns),
                    ("cluster", &self.cluster_name),
                    ("cm", cm_name.as_str()),
                ],
            );
        }
        for sec_name in &secret_refs {
            let _ = self.graph.query(
                "MATCH (p:K8sPod {name: $pod, namespace: $ns, cluster: $cluster}) \
                 MERGE (sec:K8sSecret {name: $sec, namespace: $ns, cluster: $cluster}) \
                 MERGE (p)-[:READS]->(sec)",
                &[
                    ("pod", name),
                    ("ns", ns),
                    ("cluster", &self.cluster_name),
                    ("sec", sec_name.as_str()),
                ],
            );
        }
    }

    /// Ingest a Service.
    pub fn ingest_service(&self, svc: &Service, rv: &str) {
        let meta = &svc.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");

        let spec = svc.spec.as_ref();
        let svc_type = spec.and_then(|s| s.type_.as_deref()).unwrap_or("ClusterIP");
        let cluster_ip = spec.and_then(|s| s.cluster_ip.as_deref()).unwrap_or("");

        let mut ports_strs: Vec<String> = Vec::new();
        if let Some(ports) = spec.and_then(|s| s.ports.as_ref()) {
            for p in ports {
                let proto = p.protocol.as_deref().unwrap_or("TCP");
                ports_strs.push(format!("{}/{}", p.port, proto));
            }
        }
        let ports_joined = ports_strs.join(",");

        let selector_str = spec
            .and_then(|s| s.selector.as_ref())
            .map(|sel| flatten_labels(Some(sel)))
            .unwrap_or_default();

        let _ = self.graph.query(
            "MERGE (s:K8sService {name: $name, namespace: $ns, cluster: $cluster}) \
             SET s.type = $type, \
                 s.cluster_ip = $cip, \
                 s.ports = $ports, \
                 s.selector = $selector, \
                 s.resource_version = $rv",
            &[
                ("name", name),
                ("ns", ns),
                ("cluster", &self.cluster_name),
                ("type", svc_type),
                ("cip", cluster_ip),
                ("ports", &ports_joined),
                ("selector", &selector_str),
                ("rv", rv),
            ],
        );

        self.create_contains_edge("K8sNamespace", "K8sService", "", name, ns);
    }

    /// Ingest a ConfigMap (key names only, never values).
    pub fn ingest_configmap(&self, cm: &ConfigMap, rv: &str) {
        let meta = &cm.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");

        let keys: Vec<String> = cm
            .data
            .as_ref()
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();
        let keys_joined = keys.join(",");

        let _ = self.graph.query(
            "MERGE (cm:K8sConfigMap {name: $name, namespace: $ns, cluster: $cluster}) \
             SET cm.key_names = $keys, \
                 cm.resource_version = $rv",
            &[
                ("name", name),
                ("ns", ns),
                ("cluster", &self.cluster_name),
                ("keys", &keys_joined),
                ("rv", rv),
            ],
        );

        self.create_contains_edge("K8sNamespace", "K8sConfigMap", "", name, ns);
    }

    /// Ingest a Secret (key names + type only, NEVER values).
    pub fn ingest_secret(&self, sec: &Secret, rv: &str) {
        let meta = &sec.metadata;
        let name = meta.name.as_deref().unwrap_or("");
        let ns = meta.namespace.as_deref().unwrap_or("");

        let sec_type = sec.type_.as_deref().unwrap_or("Opaque");
        let keys: Vec<String> = sec
            .data
            .as_ref()
            .map(|d| d.keys().cloned().collect())
            .unwrap_or_default();
        let keys_joined = keys.join(",");

        let _ = self.graph.query(
            "MERGE (sec:K8sSecret {name: $name, namespace: $ns, cluster: $cluster}) \
             SET sec.type = $type, \
                 sec.key_names = $keys, \
                 sec.resource_version = $rv",
            &[
                ("name", name),
                ("ns", ns),
                ("cluster", &self.cluster_name),
                ("type", sec_type),
                ("keys", &keys_joined),
                ("rv", rv),
            ],
        );

        self.create_contains_edge("K8sNamespace", "K8sSecret", "", name, ns);
    }
}

/// Flatten a k8s labels map into "key=value,key=value" for storage.
fn flatten_labels(labels: Option<&std::collections::BTreeMap<String, String>>) -> String {
    match labels {
        None => String::new(),
        Some(m) => {
            let mut pairs: Vec<String> = m.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            pairs.sort();
            pairs.join(",")
        }
    }
}
