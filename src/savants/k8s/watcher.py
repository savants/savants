"""Live K8s watch-based ingestor for the Mazkir runtime layer.

Complements `K8sIngestor.snapshot()` (pull-based full reconcile) with a
push-based streaming mode that holds a long-lived HTTP watch connection
per resource type. End-to-end staleness drops from "poll interval" to
~1–2 seconds (the kubelet → apiserver reporting lag).

Design (per resource type, one thread each):

    1. LIST the collection. This gives us a starting `resourceVersion`
       and lets us reconcile any drift that happened while we were down.
    2. WATCH from that rv with `allow_watch_bookmarks=True`. Handle
       ADDED / MODIFIED / DELETED / BOOKMARK events. Every event is
       applied through the same idempotent `_ingest_*` / `_delete_one`
       helpers on K8sIngestor.
    3. When the watch ends (server-side timeout, EOF, or 410 Gone),
       loop back to step 1 and relist.

The watch loop is intentionally single-process, multi-threaded — one
thread per resource type. For one cluster this is cheap (6 sockets,
nearly idle). For many clusters the right pattern is to run this as
an in-cluster operator and stream over the Mazkir federation API; that
is a later concern.

All writes go through K8sIngestor helpers, which are MERGE-based and
therefore safe to replay if a relist repeats events we already applied.
"""

from __future__ import annotations

import logging
import threading
import time
from dataclasses import dataclass, field
from typing import Any, Callable

from savants.k8s.correlator import StateChangeTracker
from savants.k8s.ingestor import K8sIngestor
from savants.k8s.log_watcher import LogWatcher

logger = logging.getLogger(__name__)


@dataclass
class WatchStats:
    added: int = 0
    modified: int = 0
    deleted: int = 0
    bookmarks: int = 0
    relists: int = 0
    errors: int = 0
    last_event_ts: float = 0.0
    last_resource_version: str = ""

    def __str__(self) -> str:
        return (
            f"+{self.added} ~{self.modified} -{self.deleted} "
            f"bookmarks={self.bookmarks} relists={self.relists} errors={self.errors}"
        )


@dataclass
class _TypeSpec:
    label: str                    # graph node label (e.g. "K8sPod")
    api_attr: str                 # "core" or "apps"
    list_method: str              # e.g. "list_pod_for_all_namespaces"
    handler: Callable[[Any, str], None]  # (obj, rv) → applies ingest
    namespaced: bool = True


class K8sWatcher:
    """Runs one watch stream per resource type against a live cluster.

    Reuses a K8sIngestor for all graph writes; the ingestor owns the
    FalkorDB client and the per-resource MERGE helpers.
    """

    def __init__(
        self,
        ingestor: K8sIngestor,
        backoff_seconds: float = 2.0,
        max_backoff_seconds: float = 60.0,
        watch_timeout_seconds: int = 300,
        log_watcher: LogWatcher | None = None,
    ):
        self.ingestor = ingestor
        self.backoff = backoff_seconds
        self.max_backoff = max_backoff_seconds
        self.watch_timeout = watch_timeout_seconds
        # Optional log intelligence layer: when present, every ADDED pod
        # event (including the initial reconcile list) gets tailed, and
        # every DELETED pod event triggers a purge of its LogEvent nodes.
        self.log_watcher = log_watcher
        # Temporal correlator: records state changes and creates CAUSED_BY
        # edges when LogEvents appear within the correlation window.
        self.correlator = StateChangeTracker(
            graph=ingestor.client,
            cluster=ingestor.cluster_name,
        )
        # Wire the correlator into the log watcher's writer if present
        if log_watcher is not None:
            log_watcher.writer.correlator = self.correlator
        self._stop = threading.Event()
        self._threads: list[threading.Thread] = []
        self.stats: dict[str, WatchStats] = {}

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def start(self) -> None:
        """Start one background thread per resource type."""
        # Lazy ensure cluster node + k8s api handles exist
        self.ingestor._load_k8s_client()
        # Also ensure the top-level cluster node exists so FK-style edges
        # from namespaces don't dangle. snapshot() creates it; we can
        # just do a lightweight version here.
        self._ensure_cluster_node()

        specs = self._build_specs()
        for spec in specs:
            self.stats[spec.label] = WatchStats()
            t = threading.Thread(
                target=self._run_type,
                args=(spec,),
                name=f"mazkir-watch-{spec.label}",
                daemon=True,
            )
            t.start()
            self._threads.append(t)
        logger.info("K8sWatcher started %d streams for cluster '%s'",
                    len(self._threads), self.ingestor.cluster_name)

    def stop(self, join_timeout: float = 0.5) -> None:
        self._stop.set()
        # Threads are daemon and may be blocked on a streaming HTTP read;
        # don't wait long — they'll die with the process.
        for t in self._threads:
            t.join(timeout=join_timeout)
        self._threads.clear()

    def run_forever(self) -> None:
        """Convenience for CLI use — start then block on the main thread."""
        self.start()
        try:
            while not self._stop.is_set():
                time.sleep(1.0)
        except KeyboardInterrupt:
            pass
        finally:
            self.stop()

    # ------------------------------------------------------------------
    # Type specs — which API method feeds which ingest handler
    # ------------------------------------------------------------------

    def _build_specs(self) -> list[_TypeSpec]:
        ing = self.ingestor
        return [
            _TypeSpec(
                label="K8sNamespace",
                api_attr="core",
                list_method="list_namespace",
                handler=lambda obj, rv: self._apply_namespace(obj, rv),
                namespaced=False,
            ),
            _TypeSpec(
                label="K8sDeployment",
                api_attr="apps",
                list_method="list_deployment_for_all_namespaces",
                handler=lambda obj, rv: ing._ingest_deployment(obj, "Deployment", rv=rv),
            ),
            # StatefulSets and DaemonSets share the K8sDeployment label.
            # They get their own watch streams because the API is separate.
            _TypeSpec(
                label="K8sDeployment",
                api_attr="apps",
                list_method="list_stateful_set_for_all_namespaces",
                handler=lambda obj, rv: ing._ingest_deployment(obj, "StatefulSet", rv=rv),
            ),
            _TypeSpec(
                label="K8sDeployment",
                api_attr="apps",
                list_method="list_daemon_set_for_all_namespaces",
                handler=lambda obj, rv: ing._ingest_deployment(obj, "DaemonSet", rv=rv),
            ),
            _TypeSpec(
                label="K8sPod",
                api_attr="core",
                list_method="list_pod_for_all_namespaces",
                handler=lambda obj, rv: self._handle_pod(obj, rv),
            ),
            _TypeSpec(
                label="K8sService",
                api_attr="core",
                list_method="list_service_for_all_namespaces",
                handler=lambda obj, rv: ing._ingest_service(obj, rv=rv),
            ),
            _TypeSpec(
                label="K8sConfigMap",
                api_attr="core",
                list_method="list_config_map_for_all_namespaces",
                handler=lambda obj, rv: ing._ingest_configmap(obj, rv=rv),
            ),
            _TypeSpec(
                label="K8sSecret",
                api_attr="core",
                list_method="list_secret_for_all_namespaces",
                handler=lambda obj, rv: ing._ingest_secret(obj, rv=rv),
            ),
        ]

    # ------------------------------------------------------------------
    # Per-type list + watch loop
    # ------------------------------------------------------------------

    def _run_type(self, spec: _TypeSpec) -> None:
        from kubernetes import watch as k8s_watch
        from kubernetes.client.exceptions import ApiException

        ing = self.ingestor
        api = ing._k8s_client[spec.api_attr]
        list_fn = getattr(api, spec.list_method)
        stats = self.stats[spec.label]
        backoff = self.backoff

        while not self._stop.is_set():
            try:
                # --- LIST (full reconcile + start rv) ---
                listing = list_fn()
                live_keys: set[tuple] = set()
                for obj in listing.items:
                    meta = obj.metadata
                    rv = meta.resource_version or ""
                    spec.handler(obj, rv)
                    live_keys.add((meta.name, meta.namespace or ""))
                    stats.last_resource_version = rv
                    stats.last_event_ts = time.time()

                # Delete anything in the graph that isn't in this listing.
                # We only reconcile within this spec's label *and* kind
                # scope by using the existing rv map keyed on (name, ns).
                # For labels with multiple specs (Deployment / StatefulSet
                # / DaemonSet all share K8sDeployment), we skip the prune
                # pass — snapshot() is the correct tool for that and we
                # already ran it at bootstrap. Pruning here would delete
                # StatefulSets when the Deployment stream reconciles.
                if spec.label not in ("K8sDeployment",):
                    existing = ing._current_rv_map(spec.label)
                    stale = set(existing.keys()) - live_keys
                    if stale:
                        ing._delete_stale(spec.label, stale)
                        stats.deleted += len(stale)

                stats.relists += 1
                start_rv = listing.metadata.resource_version
                backoff = self.backoff  # reset after a successful list

                # --- WATCH ---
                w = k8s_watch.Watch()
                try:
                    for event in w.stream(
                        list_fn,
                        resource_version=start_rv,
                        allow_watch_bookmarks=True,
                        timeout_seconds=self.watch_timeout,
                        _request_timeout=self.watch_timeout + 30,
                    ):
                        if self._stop.is_set():
                            w.stop()
                            break

                        etype = event.get("type", "")
                        obj = event.get("object")
                        if obj is None:
                            continue

                        # BOOKMARK events only carry an updated rv — no
                        # object body to apply. Track it so our next
                        # watch resume uses fresh rv.
                        if etype == "BOOKMARK":
                            try:
                                stats.last_resource_version = (
                                    obj.metadata.resource_version or ""
                                )
                            except Exception:
                                pass
                            stats.bookmarks += 1
                            continue

                        try:
                            meta = obj.metadata
                            rv = meta.resource_version or ""
                            if etype == "DELETED":
                                ing._delete_one(
                                    spec.label, meta.name, meta.namespace or ""
                                )
                                # Pod-specific: stop tailing logs and mark
                                # LogEvents as orphaned so the story tool
                                # can show "(pod deleted)". Events are NOT
                                # purged here — retention handles that so
                                # recent history from a crashed pod stays
                                # available for post-mortem queries.
                                if spec.label == "K8sPod" and self.log_watcher:
                                    try:
                                        self.log_watcher.mark_pod_deleted(
                                            meta.name, meta.namespace or ""
                                        )
                                    except Exception as e:
                                        logger.debug(
                                            "mark_pod_deleted error: %s", e
                                        )
                                stats.deleted += 1
                            elif etype == "ADDED":
                                spec.handler(obj, rv)
                                stats.added += 1
                                # Pod additions during watch (not reconcile)
                                # often indicate a restart after a crash.
                                if spec.label == "K8sPod":
                                    self.correlator.record(
                                        "pod_restart", meta.namespace or "",
                                        meta.name, time.time(), "K8sPod",
                                    )
                            elif etype == "MODIFIED":
                                spec.handler(obj, rv)
                                stats.modified += 1
                                # Record state changes for CAUSED_BY correlation.
                                # ConfigMap/Secret edits and Deployment rollouts
                                # are the most common causes of downstream errors.
                                _change_types = {
                                    "K8sConfigMap": ("configmap_edit", "K8sConfigMap"),
                                    "K8sSecret": ("secret_edit", "K8sSecret"),
                                    "K8sDeployment": ("deployment_rollout", "K8sDeployment"),
                                }
                                if spec.label in _change_types:
                                    ctype, clabel = _change_types[spec.label]
                                    self.correlator.record(
                                        ctype, meta.namespace or "",
                                        meta.name, time.time(), clabel,
                                    )
                            else:
                                logger.debug("Unknown watch event type: %s", etype)

                            stats.last_resource_version = rv
                            stats.last_event_ts = time.time()
                        except Exception as e:
                            stats.errors += 1
                            logger.warning(
                                "watch[%s] handler error: %s", spec.list_method, e
                            )
                finally:
                    try:
                        w.stop()
                    except Exception:
                        pass

            except ApiException as e:
                # 410 Gone = rv too old, etcd compacted it. Relist.
                if getattr(e, "status", None) == 410:
                    logger.info("watch[%s] 410 Gone — relisting", spec.list_method)
                    continue
                stats.errors += 1
                logger.warning("watch[%s] ApiException: %s", spec.list_method, e)
            except Exception as e:
                stats.errors += 1
                logger.warning("watch[%s] error: %s", spec.list_method, e)

            # Backoff on unexpected failures (not on clean watch expiry,
            # which falls straight through to the next loop iteration).
            if self._stop.is_set():
                break
            time.sleep(backoff)
            backoff = min(backoff * 2, self.max_backoff)

    # ------------------------------------------------------------------
    # Small helpers
    # ------------------------------------------------------------------

    def _ensure_cluster_node(self) -> None:
        """Create the top-level K8sCluster node if it isn't there yet."""
        from savants.graph.schema import K8sClusterNode, create_k8s_cluster_query

        ing = self.ingestor
        version = ""
        try:
            v = ing._k8s_client["version"].get_code()
            version = f"{v.major}.{v.minor}"
        except Exception:
            pass
        node = K8sClusterNode(
            name=ing.cluster_name,
            version=version,
            context=ing.kube_context or ing.cluster_name,
        )
        ing._merge(create_k8s_cluster_query(node))

    def _handle_pod(self, obj: Any, rv: str) -> None:
        """Pod handler: write the node *and* subscribe the log watcher.

        `log_watcher.add_pod` is idempotent — calling it on every ADDED
        and MODIFIED event (and on every reconcile list item) is safe.
        """
        self.ingestor._ingest_pod(obj, rv=rv)
        if self.log_watcher is not None:
            try:
                meta = obj.metadata
                # Only tail pods that are (or were) actually running.
                # Pending pods with no container yet have no logs, and
                # Succeeded batch jobs will emit "connection closed"
                # noise if we tail them after they've exited.
                phase = obj.status.phase if obj.status else ""
                if phase in ("Running", "Succeeded", "Failed"):
                    self.log_watcher.add_pod(meta.name, meta.namespace)
            except Exception as e:
                logger.debug("log_watcher add_pod failed: %s", e)

    def _apply_namespace(self, obj: Any, rv: str) -> None:
        """Inline handler for namespaces (ingestor doesn't expose one)."""
        from savants.graph.schema import (
            K8sNamespaceNode,
            create_k8s_namespace_query,
        )

        ing = self.ingestor
        meta = obj.metadata
        status = obj.status.phase if obj.status else "Unknown"
        age = ing._age_seconds(meta.creation_timestamp)
        node = K8sNamespaceNode(
            name=meta.name,
            cluster=ing.cluster_name,
            status=status,
            age_seconds=age,
            resource_version=rv,
        )
        ing._merge(create_k8s_namespace_query(node))
        ing.client.query(
            "MATCH (c:K8sCluster {name: $cluster}) "
            "MATCH (n:K8sNamespace {name: $ns, cluster: $cluster}) "
            "MERGE (c)-[:CONTAINS]->(n)",
            {"cluster": ing.cluster_name, "ns": meta.name},
        )
