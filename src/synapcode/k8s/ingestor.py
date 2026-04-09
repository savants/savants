"""Kubernetes cluster ingestor for the Mazkir runtime layer.

Reads the state of a Kubernetes cluster via the `kubernetes` Python client,
maps resources to Mazkir graph nodes, and writes them to a FalkorDB graph
via `GraphClient`. Supports both one-shot snapshot mode (for initial load
and manual refreshes) and — in the future — watch mode for continuous
updates.

This is the first concrete delivery of the Live Infrastructure Layer
designed in `docs/live-infrastructure-layer.md`. The schema and edge
model are defined in `src/synapcode/graph/schema.py`.

Deliberate scope limits:

- No secret values are ever stored (just secret names and key names).
- No ConfigMap values are stored — only key names — to avoid ingesting
  application config that might contain credentials.
- No pod logs, no metrics, no ephemeral events past "what's the current
  state." Metrics go in Prometheus, logs go in Loki, Mazkir is the index.
- Sub-resources like endpoint slices, replica sets, horizontal autoscalers
  are omitted from v1 for simplicity. They can be added later without
  breaking the existing schema (additive only).

Usage:

    from synapcode.k8s.ingestor import K8sIngestor
    from synapcode.graph.client import GraphClient
    from synapcode.config import FalkorDBConfig

    client = GraphClient(FalkorDBConfig(graph_name="astra_k3s"))
    ingestor = K8sIngestor(graph_client=client, cluster_name="astra-k3s")
    stats = ingestor.snapshot()
    print(stats)
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from typing import Any

from synapcode.graph.client import GraphClient
from synapcode.graph.schema import (
    K8sClusterNode,
    K8sConfigMapNode,
    K8sDeploymentNode,
    K8sNamespaceNode,
    K8sPodNode,
    K8sSecretNode,
    K8sServiceNode,
    create_k8s_cluster_query,
    create_k8s_configmap_query,
    create_k8s_deployment_query,
    create_k8s_namespace_query,
    create_k8s_pod_query,
    create_k8s_secret_query,
    create_k8s_service_query,
)

logger = logging.getLogger(__name__)


@dataclass
class IngestStats:
    """Per-run stats for a cluster ingest. Returned from snapshot()."""

    cluster: str = ""
    elapsed_seconds: float = 0.0
    namespaces: int = 0
    deployments: int = 0
    statefulsets: int = 0
    daemonsets: int = 0
    pods: int = 0
    services: int = 0
    configmaps: int = 0
    secrets: int = 0
    edges_created: int = 0
    errors: list[str] = field(default_factory=list)

    def summary(self) -> str:
        lines = [
            f"K8s ingest complete for cluster '{self.cluster}' in {self.elapsed_seconds:.1f}s",
            f"  Namespaces:   {self.namespaces}",
            f"  Deployments:  {self.deployments} (+{self.statefulsets} StatefulSets, +{self.daemonsets} DaemonSets)",
            f"  Pods:         {self.pods}",
            f"  Services:     {self.services}",
            f"  ConfigMaps:   {self.configmaps}",
            f"  Secrets:      {self.secrets}",
            f"  Edges:        {self.edges_created}",
        ]
        if self.errors:
            lines.append(f"  ⚠ {len(self.errors)} errors during ingest:")
            for e in self.errors[:5]:
                lines.append(f"    - {e}")
        return "\n".join(lines)


class K8sIngestor:
    """Reads a cluster via the kubernetes client and writes to FalkorDB.

    The cluster_name is the human-readable identity tag written to every
    node's `cluster` property. It should match the kubeconfig context
    name when possible for consistency, but Mazkir doesn't require it —
    the name is just a label for federation.
    """

    def __init__(
        self,
        graph_client: GraphClient,
        cluster_name: str,
        kube_context: str | None = None,
        kubeconfig_path: str | None = None,
    ):
        self.client = graph_client
        self.cluster_name = cluster_name
        self.kube_context = kube_context
        self.kubeconfig_path = kubeconfig_path
        self._k8s_client = None  # lazy init

    def _load_k8s_client(self):
        """Lazy load the kubernetes client config + API clients."""
        if self._k8s_client is not None:
            return self._k8s_client

        # Import inline so the k8s module is only loaded on first ingest —
        # keeps import time for the rest of synapcode fast.
        from kubernetes import client as k8s_client
        from kubernetes import config as k8s_config

        try:
            k8s_config.load_kube_config(
                config_file=self.kubeconfig_path,
                context=self.kube_context,
            )
        except Exception:
            # Fall back to in-cluster config (for running inside a pod as
            # an operator). The future k8s operator will use this path.
            k8s_config.load_incluster_config()

        self._k8s_client = {
            "core": k8s_client.CoreV1Api(),
            "apps": k8s_client.AppsV1Api(),
            "version": k8s_client.VersionApi(),
        }
        return self._k8s_client

    def snapshot(self) -> IngestStats:
        """Take a one-shot snapshot of the cluster and write it to the graph.

        This is idempotent — running it multiple times will MERGE nodes,
        so repeated runs update the graph in place. Used for manual
        refreshes and as the initial load before watch mode.
        """
        t0 = time.time()
        stats = IngestStats(cluster=self.cluster_name)

        k8s = self._load_k8s_client()

        # 1. Cluster node (top-level scope)
        try:
            version_info = k8s["version"].get_code()
            server_version = f"{version_info.major}.{version_info.minor}"
        except Exception as e:
            logger.warning("Could not fetch cluster version: %s", e)
            server_version = ""

        cluster_node = K8sClusterNode(
            name=self.cluster_name,
            version=server_version,
            context=self.kube_context or self.cluster_name,
        )
        self._merge(create_k8s_cluster_query(cluster_node))

        # 2. Namespaces
        try:
            ns_list = k8s["core"].list_namespace().items
        except Exception as e:
            stats.errors.append(f"list_namespace: {e}")
            return stats

        for ns in ns_list:
            meta = ns.metadata
            status = ns.status.phase if ns.status else "Unknown"
            age = self._age_seconds(meta.creation_timestamp)
            ns_node = K8sNamespaceNode(
                name=meta.name,
                cluster=self.cluster_name,
                status=status,
                age_seconds=age,
            )
            self._merge(create_k8s_namespace_query(ns_node))
            # Edge: K8sCluster CONTAINS K8sNamespace
            self.client.query(
                "MATCH (c:K8sCluster {name: $cluster}) "
                "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
                "MERGE (c)-[:CONTAINS]->(n)",
                {"cluster": self.cluster_name, "ns": meta.name},
            )
            stats.namespaces += 1
            stats.edges_created += 1

        # 3. Deployments (and StatefulSets, DaemonSets — same node type)
        try:
            deploy_list = k8s["apps"].list_deployment_for_all_namespaces().items
        except Exception as e:
            stats.errors.append(f"list_deployments: {e}")
            deploy_list = []

        for d in deploy_list:
            try:
                self._ingest_deployment(d, kind="Deployment")
                stats.deployments += 1
                stats.edges_created += 1  # CONTAINS edge from namespace
            except Exception as e:
                stats.errors.append(f"deployment {d.metadata.name}: {e}")

        try:
            sts_list = k8s["apps"].list_stateful_set_for_all_namespaces().items
            for s in sts_list:
                self._ingest_deployment(s, kind="StatefulSet")
                stats.statefulsets += 1
                stats.edges_created += 1
        except Exception as e:
            stats.errors.append(f"list_stateful_sets: {e}")

        try:
            ds_list = k8s["apps"].list_daemon_set_for_all_namespaces().items
            for d in ds_list:
                self._ingest_deployment(d, kind="DaemonSet")
                stats.daemonsets += 1
                stats.edges_created += 1
        except Exception as e:
            stats.errors.append(f"list_daemon_sets: {e}")

        # 4. Pods
        try:
            pod_list = k8s["core"].list_pod_for_all_namespaces().items
        except Exception as e:
            stats.errors.append(f"list_pods: {e}")
            pod_list = []

        for p in pod_list:
            try:
                self._ingest_pod(p)
                stats.pods += 1
                # edges: namespace CONTAINS pod, owner RUNS pod, pod READS configmap/secret
                stats.edges_created += 1  # CONTAINS (+ ownership counted inside)
            except Exception as e:
                stats.errors.append(f"pod {p.metadata.name}: {e}")

        # 5. Services
        try:
            svc_list = k8s["core"].list_service_for_all_namespaces().items
        except Exception as e:
            stats.errors.append(f"list_services: {e}")
            svc_list = []

        for svc in svc_list:
            try:
                self._ingest_service(svc)
                stats.services += 1
                stats.edges_created += 1
            except Exception as e:
                stats.errors.append(f"service {svc.metadata.name}: {e}")

        # 6. ConfigMaps (names + key names only, never values)
        try:
            cm_list = k8s["core"].list_config_map_for_all_namespaces().items
        except Exception as e:
            stats.errors.append(f"list_config_maps: {e}")
            cm_list = []

        for cm in cm_list:
            try:
                self._ingest_configmap(cm)
                stats.configmaps += 1
                stats.edges_created += 1
            except Exception as e:
                stats.errors.append(f"configmap {cm.metadata.name}: {e}")

        # 7. Secrets (names + key names + type, NEVER values)
        try:
            sec_list = k8s["core"].list_secret_for_all_namespaces().items
        except Exception as e:
            stats.errors.append(f"list_secrets: {e}")
            sec_list = []

        for sec in sec_list:
            try:
                self._ingest_secret(sec)
                stats.secrets += 1
                stats.edges_created += 1
            except Exception as e:
                stats.errors.append(f"secret {sec.metadata.name}: {e}")

        stats.elapsed_seconds = time.time() - t0
        logger.info(stats.summary())
        return stats

    # ------------------------------------------------------------------
    # Per-resource ingest helpers
    # ------------------------------------------------------------------

    def _ingest_deployment(self, d: Any, kind: str) -> None:
        meta = d.metadata
        spec = d.spec
        status = d.status or None

        # Primary container image (first container of the pod template)
        image = ""
        try:
            containers = spec.template.spec.containers
            if containers:
                image = containers[0].image or ""
        except Exception:
            pass

        labels = self._flatten_labels(meta.labels)

        node = K8sDeploymentNode(
            name=meta.name,
            namespace=meta.namespace,
            cluster=self.cluster_name,
            kind=kind,
            replicas_desired=getattr(spec, "replicas", 0) or 0,
            replicas_ready=(getattr(status, "ready_replicas", 0) or 0) if status else 0,
            replicas_available=(
                getattr(status, "available_replicas", 0) or 0
            ) if status else 0,
            image=image,
            labels=labels,
        )
        self._merge(create_k8s_deployment_query(node))

        # Edge: Namespace CONTAINS Deployment
        self.client.query(
            "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
            "MATCH (d:K8sDeployment {name: $name, namespace: $ns, cluster: $cluster}) "
            "MERGE (n)-[:CONTAINS]->(d)",
            {"ns": meta.namespace, "name": meta.name, "cluster": self.cluster_name},
        )

    def _ingest_pod(self, p: Any) -> None:
        meta = p.metadata
        spec = p.spec
        status = p.status

        image = ""
        try:
            if spec.containers:
                image = spec.containers[0].image or ""
        except Exception:
            pass

        restart_count = 0
        ready = False
        try:
            if status and status.container_statuses:
                restart_count = sum(cs.restart_count for cs in status.container_statuses)
                ready = all(cs.ready for cs in status.container_statuses)
        except Exception:
            pass

        # Parse phase/status. Pods in CrashLoopBackOff show phase=Running
        # but a waiting reason of CrashLoopBackOff on the container status.
        phase = status.phase if status else "Unknown"
        try:
            if status and status.container_statuses:
                for cs in status.container_statuses:
                    if cs.state and cs.state.waiting:
                        reason = cs.state.waiting.reason or ""
                        if reason in ("CrashLoopBackOff", "ImagePullBackOff", "ErrImagePull"):
                            phase = reason
                            break
        except Exception:
            pass

        # Owner reference (ReplicaSet / StatefulSet / DaemonSet / Job / etc.)
        owner_kind = ""
        owner_name = ""
        try:
            if meta.owner_references:
                owner_kind = meta.owner_references[0].kind
                owner_name = meta.owner_references[0].name
        except Exception:
            pass

        node = K8sPodNode(
            name=meta.name,
            namespace=meta.namespace,
            cluster=self.cluster_name,
            status=phase,
            node_name=spec.node_name or "",
            restart_count=restart_count,
            ready=ready,
            image=image,
            owner_kind=owner_kind,
            owner_name=owner_name,
        )
        self._merge(create_k8s_pod_query(node))

        # Edge: Namespace CONTAINS Pod
        self.client.query(
            "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
            "MATCH (p:K8sPod {name: $name, namespace: $ns, cluster: $cluster}) "
            "MERGE (n)-[:CONTAINS]->(p)",
            {"ns": meta.namespace, "name": meta.name, "cluster": self.cluster_name},
        )

        # Edge: Pod READS ConfigMap / Secret (from volumes + envFrom)
        try:
            self._link_pod_dependencies(p)
        except Exception as e:
            logger.debug("Could not link pod deps for %s: %s", meta.name, e)

    def _link_pod_dependencies(self, p: Any) -> None:
        """Create READS edges from a Pod to the ConfigMaps/Secrets it uses.

        We check two places:
        1. Volume mounts: spec.volumes[].config_map / .secret
        2. Env references: spec.containers[].env_from / env.value_from
        """
        meta = p.metadata
        spec = p.spec

        configmap_refs: set[str] = set()
        secret_refs: set[str] = set()

        # Volume-based refs
        if spec.volumes:
            for v in spec.volumes:
                if v.config_map and v.config_map.name:
                    configmap_refs.add(v.config_map.name)
                if v.secret and v.secret.secret_name:
                    secret_refs.add(v.secret.secret_name)

        # envFrom refs on each container
        if spec.containers:
            for c in spec.containers:
                if c.env_from:
                    for ef in c.env_from:
                        if ef.config_map_ref and ef.config_map_ref.name:
                            configmap_refs.add(ef.config_map_ref.name)
                        if ef.secret_ref and ef.secret_ref.name:
                            secret_refs.add(ef.secret_ref.name)
                if c.env:
                    for env in c.env:
                        if env.value_from:
                            if env.value_from.config_map_key_ref:
                                configmap_refs.add(
                                    env.value_from.config_map_key_ref.name
                                )
                            if env.value_from.secret_key_ref:
                                secret_refs.add(env.value_from.secret_key_ref.name)

        for cm_name in configmap_refs:
            self.client.query(
                "MATCH (p:K8sPod {name: $pod, namespace: $ns, cluster: $cluster}) "
                "MERGE (cm:K8sConfigMap {name: $cm, namespace: $ns, cluster: $cluster}) "
                "MERGE (p)-[:READS]->(cm)",
                {
                    "pod": meta.name,
                    "ns": meta.namespace,
                    "cluster": self.cluster_name,
                    "cm": cm_name,
                },
            )
        for sec_name in secret_refs:
            self.client.query(
                "MATCH (p:K8sPod {name: $pod, namespace: $ns, cluster: $cluster}) "
                "MERGE (sec:K8sSecret {name: $sec, namespace: $ns, cluster: $cluster}) "
                "MERGE (p)-[:READS]->(sec)",
                {
                    "pod": meta.name,
                    "ns": meta.namespace,
                    "cluster": self.cluster_name,
                    "sec": sec_name,
                },
            )

    def _ingest_service(self, svc: Any) -> None:
        meta = svc.metadata
        spec = svc.spec

        ports: list[str] = []
        if spec.ports:
            for p in spec.ports:
                proto = p.protocol or "TCP"
                ports.append(f"{p.port}/{proto}")

        selector = self._flatten_labels(spec.selector or {})

        node = K8sServiceNode(
            name=meta.name,
            namespace=meta.namespace,
            cluster=self.cluster_name,
            type=spec.type or "ClusterIP",
            cluster_ip=spec.cluster_ip or "",
            ports=ports,
            selector=selector,
        )
        self._merge(create_k8s_service_query(node))

        # Edge: Namespace CONTAINS Service
        self.client.query(
            "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
            "MATCH (s:K8sService {name: $name, namespace: $ns, cluster: $cluster}) "
            "MERGE (n)-[:CONTAINS]->(s)",
            {"ns": meta.namespace, "name": meta.name, "cluster": self.cluster_name},
        )

    def _ingest_configmap(self, cm: Any) -> None:
        meta = cm.metadata
        keys = list(cm.data.keys()) if cm.data else []

        node = K8sConfigMapNode(
            name=meta.name,
            namespace=meta.namespace,
            cluster=self.cluster_name,
            key_names=keys,
        )
        self._merge(create_k8s_configmap_query(node))

        self.client.query(
            "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
            "MATCH (cm:K8sConfigMap {name: $name, namespace: $ns, cluster: $cluster}) "
            "MERGE (n)-[:CONTAINS]->(cm)",
            {"ns": meta.namespace, "name": meta.name, "cluster": self.cluster_name},
        )

    def _ingest_secret(self, sec: Any) -> None:
        meta = sec.metadata
        keys = list(sec.data.keys()) if sec.data else []

        node = K8sSecretNode(
            name=meta.name,
            namespace=meta.namespace,
            cluster=self.cluster_name,
            type=sec.type or "Opaque",
            key_names=keys,  # key names only, NEVER values
        )
        self._merge(create_k8s_secret_query(node))

        self.client.query(
            "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
            "MATCH (sec:K8sSecret {name: $name, namespace: $ns, cluster: $cluster}) "
            "MERGE (n)-[:CONTAINS]->(sec)",
            {"ns": meta.namespace, "name": meta.name, "cluster": self.cluster_name},
        )

    # ------------------------------------------------------------------
    # Small helpers
    # ------------------------------------------------------------------

    def _merge(self, cypher_and_params: tuple[str, dict]) -> None:
        """Run a MERGE query. Wraps the two-tuple shape returned by create_* helpers."""
        cypher, params = cypher_and_params
        self.client.query(cypher, params)

    @staticmethod
    def _age_seconds(ts) -> int:
        """Convert a k8s creation timestamp to age in seconds."""
        if ts is None:
            return 0
        try:
            from datetime import datetime, timezone

            now = datetime.now(timezone.utc)
            return int((now - ts).total_seconds())
        except Exception:
            return 0

    @staticmethod
    def _flatten_labels(labels: dict | None) -> list[str]:
        """Flatten a k8s labels dict into ['key=value', ...] for storage."""
        if not labels:
            return []
        return [f"{k}={v}" for k, v in sorted(labels.items())]
