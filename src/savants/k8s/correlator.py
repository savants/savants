"""Temporal correlation engine for CAUSED_BY edge inference.

Maintains a rolling window of recent cluster state changes (configmap
edits, deployment rollouts, pod restarts, node state changes) and
checks incoming LogEvents against this window. When a LogEvent appears
within `correlation_window_seconds` of a state change in the same
namespace, a `CAUSED_BY` edge is created with `confidence: "candidate"`.

This is NOT causal inference — it's temporal correlation. The edge says
"this error appeared 30 seconds after this configmap was edited" and
lets the human or AI decide if it's causal. False positives are possible
and expected; the `confidence` property makes that explicit.

Usage:

    tracker = StateChangeTracker(graph_client, cluster)
    tracker.record("configmap_edit", "prod", "api-config", time.time())
    # ... later, when a LogEvent flushes:
    tracker.correlate("prod", "api-gateway-xyz", template_hash, event_ts)
"""

from __future__ import annotations

import logging
import threading
import time
from collections import deque
from dataclasses import dataclass

from savants.graph.client import GraphClient

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class StateChange:
    """A recorded cluster state change."""
    change_type: str  # configmap_edit, deployment_rollout, pod_restart, secret_edit, node_not_ready
    namespace: str
    resource_name: str
    timestamp: float
    label: str = ""  # graph node label for CAUSED_BY target (e.g. K8sConfigMap)


class StateChangeTracker:
    """Rolling window of recent cluster state changes for CAUSED_BY correlation."""

    def __init__(
        self,
        graph: GraphClient,
        cluster: str,
        window_seconds: float = 120.0,
        max_events: int = 1000,
    ):
        self.graph = graph
        self.cluster = cluster
        self.window = window_seconds
        self._changes: deque[StateChange] = deque(maxlen=max_events)
        self._lock = threading.Lock()
        self._edges_created = 0

    def record(
        self,
        change_type: str,
        namespace: str,
        resource_name: str,
        timestamp: float | None = None,
        label: str = "",
    ) -> None:
        """Record a state change in the rolling window."""
        ts = timestamp or time.time()
        change = StateChange(
            change_type=change_type,
            namespace=namespace,
            resource_name=resource_name,
            timestamp=ts,
            label=label,
        )
        with self._lock:
            self._changes.append(change)
            # Prune old entries
            cutoff = time.time() - self.window
            while self._changes and self._changes[0].timestamp < cutoff:
                self._changes.popleft()

    def correlate(
        self,
        namespace: str,
        pod: str,
        template_hash: str,
        event_ts: float,
    ) -> int:
        """Check if any recent state changes correlate with this LogEvent.

        Returns the number of CAUSED_BY edges created.
        """
        cutoff_lo = event_ts - self.window
        cutoff_hi = event_ts + 10  # small grace for clock skew

        candidates: list[StateChange] = []
        with self._lock:
            for change in self._changes:
                if change.timestamp < cutoff_lo:
                    continue
                if change.timestamp > cutoff_hi:
                    continue
                # Must be same namespace (cross-namespace correlation is too noisy)
                if change.namespace and change.namespace != namespace:
                    continue
                candidates.append(change)

        if not candidates:
            return 0

        n = 0
        for change in candidates:
            delta = round(event_ts - change.timestamp, 1)
            try:
                # Create CAUSED_BY edge from LogEvent to the resource that changed
                if change.label:
                    self.graph.query(
                        f"MATCH (e:LogEvent {{cluster: $cluster, namespace: $ns, "
                        f"pod: $pod, template_hash: $th}}) "
                        f"MATCH (x:{change.label} {{name: $rname, namespace: $ns, "
                        f"cluster: $cluster}}) "
                        f"MERGE (e)-[r:CAUSED_BY]->(x) "
                        f"SET r.confidence = 'candidate', r.delta_seconds = $delta, "
                        f"r.change_type = $ctype",
                        {
                            "cluster": self.cluster,
                            "ns": namespace,
                            "pod": pod,
                            "th": template_hash,
                            "rname": change.resource_name,
                            "delta": delta,
                            "ctype": change.change_type,
                        },
                    )
                    n += 1
            except Exception as e:
                logger.debug("correlate CAUSED_BY error: %s", e)

        self._edges_created += n
        return n

    @property
    def edges_created(self) -> int:
        return self._edges_created

    @property
    def changes_in_window(self) -> int:
        with self._lock:
            return len(self._changes)
