"""Live log intelligence layer for the Mazkir runtime.

Streams stdout/stderr from every tracked pod, runs a three-tier
significance pipeline (classifier → drain3 template dedupe → graph
write), and produces `LogEvent` nodes that are deduplicated per
(pod, template_hash). The graph stays small; the raw log firehose is
discarded.

Architecture::

    kubelet (follow=true) ──► Tier 1: severity / token classifier
                              ├── drop: INFO, DEBUG, healthchecks, dupes
                              └── keep: errors, panics, high-signal tokens
                                        │
                                        ▼
                             Tier 2: drain3 template extractor
                              ├── buckets by (pod, template_hash)
                              └── accumulates count / first_seen / last_seen
                                        │
                                        ▼
                             Tier 3: graph write
                              └── MERGE LogEvent via create_log_event_query
                                  + EMITTED edge from K8sPod

No LLM in the ingest path. LLMs only run on query-time narration over
the resulting subgraph (see `pod_story` MCP tool, later phase).

Reads logs directly from the kube-apiserver via the kubelet proxy
endpoint (`/api/v1/namespaces/{ns}/pods/{pod}/log?follow=true`). No
external log collector required. Latency from pod `print()` to graph
write is typically 200–500ms in steady state.
"""

from __future__ import annotations

import datetime as _dt
import logging
import re
import threading
import time
from dataclasses import dataclass, field
from typing import Any

from savants.graph.client import GraphClient
from savants.graph.schema import LogEventNode, create_log_event_query

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Log timestamp extraction
# ---------------------------------------------------------------------------

# Order matters: most specific first. Each regex must capture a group we can
# feed to the parser paired with it.
_TS_ISO = re.compile(
    r"(\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?)"
)
# klog: `E0407 12:10:33.317687` (level letter + MMDD + time, no year — assume current)
_TS_KLOG = re.compile(r"\b[EIWF](\d{2})(\d{2})\s+(\d{2}:\d{2}:\d{2}(?:\.\d+)?)")


def extract_log_timestamp(line: str) -> float | None:
    """Best-effort parse of an emission timestamp from a log line.

    Returns a unix timestamp if one of the supported formats is found,
    else None (caller should fall back to ingestion time). Keeps latency
    low: single regex scan per format, no datetime parse in the hot path
    unless a match hits.
    """
    m = _TS_ISO.search(line)
    if m:
        raw = m.group(1).replace(",", ".").replace(" ", "T")
        # Handle the trailing Z and missing colon in offset like "+0000"
        if raw.endswith("Z"):
            raw = raw[:-1] + "+00:00"
        # Normalize "+0000" → "+00:00"
        if len(raw) >= 5 and raw[-5] in "+-" and raw[-3] != ":":
            raw = raw[:-2] + ":" + raw[-2:]
        try:
            return _dt.datetime.fromisoformat(raw).timestamp()
        except Exception:
            pass

    m = _TS_KLOG.search(line)
    if m:
        try:
            month, day, hms = m.group(1), m.group(2), m.group(3)
            year = _dt.datetime.now().year
            raw = f"{year}-{month}-{day}T{hms}+00:00"
            return _dt.datetime.fromisoformat(raw).timestamp()
        except Exception:
            pass

    return None


# ---------------------------------------------------------------------------
# Tier 1: classifier
# ---------------------------------------------------------------------------

SEVERITY_PATTERNS: list[tuple[re.Pattern, str]] = [
    (re.compile(r"\b(FATAL|PANIC|panic:)\b"), "FATAL"),
    (re.compile(r"\b(ERROR|ERR|Exception|Traceback|error:)\b", re.I), "ERROR"),
    (re.compile(r"\b(WARN|WARNING)\b", re.I), "WARN"),
]

# Tokens that should always be kept even without an explicit severity word.
HIGH_SIGNAL_TOKENS = re.compile(
    r"\b("
    r"OOMKilled|CrashLoopBackOff|ImagePullBackOff|"
    r"connection\s+refused|dial\s+tcp|"
    r"permission\s+denied|ENOENT|EACCES|"
    r"timeout|timed\s+out|"
    r"segmentation\s+fault|segfault|"
    r"5\d\d\s|"  # 5xx status code
    r"KeyError|ValueError|RuntimeError|NullPointerException|"
    r"no\s+such\s+file"
    r")",
    re.I,
)

# Drop unconditionally — healthchecks, debug, access log noise.
DROP_PATTERNS = re.compile(
    r"\b(DEBUG|TRACE|healthz|readyz|livez|/metrics\s)",
    re.I,
)


def classify_line(line: str) -> str | None:
    """Return a severity label if the line is significant, else None.

    Cheap, side-effect free, order: drop → severity pattern → token signal.
    """
    if not line or len(line) > 8192:  # ignore binary spam / huge lines
        return None
    if DROP_PATTERNS.search(line):
        return None
    for pat, label in SEVERITY_PATTERNS:
        if pat.search(line):
            return label
    if HIGH_SIGNAL_TOKENS.search(line):
        return "WARN"
    return None


# ---------------------------------------------------------------------------
# Tier 2: drain3 template extraction + per-pod accumulator
# ---------------------------------------------------------------------------


@dataclass
class _Bucket:
    """In-memory aggregator for one (pod, template_hash) pair."""

    template_hash: str
    template_text: str
    severity: str
    first_seen: float
    last_seen: float
    count: int = 0
    example_lines: list[str] = field(default_factory=list)
    dirty: bool = False  # needs graph flush

    EXAMPLE_CAP = 5

    def add(self, line: str, severity: str, ts: float) -> None:
        self.count += 1
        self.last_seen = ts
        # Keep the highest severity seen for this template
        if _severity_rank(severity) > _severity_rank(self.severity):
            self.severity = severity
        if len(self.example_lines) < self.EXAMPLE_CAP:
            self.example_lines.append(line)
        self.dirty = True


def _severity_rank(sev: str) -> int:
    return {"INFO": 0, "WARN": 1, "ERROR": 2, "FATAL": 3}.get(sev, 0)


class PodTemplateMiner:
    """Per-pod drain3 state plus the bucket accumulator.

    Each pod gets its own miner so template IDs are stable within a pod
    but can't collide across pods with very different log shapes. This
    also makes it trivial to evict state when a pod is deleted.
    """

    def __init__(self, pod: str, namespace: str, max_clusters: int = 500):
        from drain3 import TemplateMiner
        from drain3.template_miner_config import TemplateMinerConfig

        cfg = TemplateMinerConfig()
        cfg.drain_max_clusters = max_clusters
        cfg.drain_sim_th = 0.5
        # drain3 reads a config file by default; skip it.
        self._miner = TemplateMiner(config=cfg)
        self.pod = pod
        self.namespace = namespace
        self.buckets: dict[str, _Bucket] = {}

    def ingest(self, line: str, severity: str, ts: float) -> _Bucket | None:
        """Feed a line through drain3; return the bucket if it's new or dirty."""
        line = line.rstrip("\n").strip()
        if not line:
            return None
        result = self._miner.add_log_message(line)
        if not result:
            return None
        cluster_id = str(result["cluster_id"])
        template = result.get("template_mined", "")
        change = result.get("change_type", "none")

        bucket = self.buckets.get(cluster_id)
        if bucket is None:
            bucket = _Bucket(
                template_hash=cluster_id,
                template_text=template,
                severity=severity,
                first_seen=ts,
                last_seen=ts,
            )
            self.buckets[cluster_id] = bucket
        elif change == "cluster_template_changed":
            bucket.template_text = template

        bucket.add(line, severity, ts)
        return bucket


# ---------------------------------------------------------------------------
# Tier 3: graph writer
# ---------------------------------------------------------------------------


class EntityIndex:
    """Per-cluster index of K8s entity names for MENTIONS edge extraction.

    Maintains a per-namespace regex of every ConfigMap / Secret / Service
    / Deployment name, so each flushed LogEvent can be scanned for
    mentions and linked to the actual graph node it refers to. The index
    is namespace-scoped: a log line from `prod/api-gateway` that says
    "api-config" matches the `api-config` ConfigMap in `prod` but not
    in `dev`. This avoids cross-namespace false positives.

    Refresh cadence is caller-driven (called from the flusher loop).
    """

    def __init__(self, graph: GraphClient, cluster: str):
        self.graph = graph
        self.cluster = cluster
        # {namespace: {name: label}}
        self._by_ns: dict[str, dict[str, str]] = {}
        # {namespace: compiled regex}
        self._re_by_ns: dict[str, re.Pattern] = {}
        self._last_refresh = 0.0

    def refresh(self) -> None:
        by_ns: dict[str, dict[str, str]] = {}
        for label in ("K8sConfigMap", "K8sSecret", "K8sService", "K8sDeployment"):
            r = self.graph.query(
                f"MATCH (x:{label} {{cluster: $cluster}}) "
                "RETURN x.name, x.namespace",
                {"cluster": self.cluster},
            )
            for row in r.result_set or []:
                name, ns = row[0], row[1] or ""
                if not name or len(name) < 4:
                    # Skip very short names — too many false positives
                    # (e.g. a ConfigMap literally named "db" would match
                    # every log line containing the word "db").
                    continue
                by_ns.setdefault(ns, {})[name] = label

        re_by_ns: dict[str, re.Pattern] = {}
        for ns, names in by_ns.items():
            if not names:
                continue
            # Longest-first so "authentik-postgresql" wins over "authentik"
            sorted_names = sorted(names.keys(), key=len, reverse=True)
            alt = "|".join(re.escape(n) for n in sorted_names)
            re_by_ns[ns] = re.compile(rf"\b({alt})\b")

        self._by_ns = by_ns
        self._re_by_ns = re_by_ns
        self._last_refresh = time.time()

    def scan(self, namespace: str, *texts: str) -> list[tuple[str, str]]:
        """Return [(name, label)] for every entity mentioned in any text.

        Deduplicated within a single call. Namespace-scoped: only
        entities in the given namespace are considered.
        """
        regex = self._re_by_ns.get(namespace)
        if regex is None:
            return []
        name_to_label = self._by_ns.get(namespace, {})
        hits: dict[str, str] = {}
        for t in texts:
            if not t:
                continue
            for m in regex.finditer(t):
                n = m.group(1)
                label = name_to_label.get(n)
                if label:
                    hits[n] = label
        return list(hits.items())


class LogEventWriter:
    """Flushes dirty buckets to FalkorDB as LogEvent nodes + EMITTED edges."""

    def __init__(self, graph: GraphClient, cluster: str, entity_index: EntityIndex | None = None):
        self.graph = graph
        self.cluster = cluster
        self.entity_index = entity_index

    def flush(self, pod: str, namespace: str, buckets: dict[str, _Bucket]) -> int:
        n = 0
        for b in buckets.values():
            if not b.dirty:
                continue
            node = LogEventNode(
                template_hash=b.template_hash,
                pod=pod,
                namespace=namespace,
                cluster=self.cluster,
                severity=b.severity,
                template_text=b.template_text,
                first_seen=b.first_seen,
                last_seen=b.last_seen,
                count=b.count,
                example_lines=list(b.example_lines),
            )
            cy, params = create_log_event_query(node)
            self.graph.query(cy, params)
            # Edge: Pod EMITTED LogEvent
            self.graph.query(
                "MATCH (p:K8sPod {name: $pod, namespace: $ns, cluster: $cluster}) "
                "MATCH (e:LogEvent {cluster: $cluster, namespace: $ns, "
                "pod: $pod, template_hash: $th}) "
                "MERGE (p)-[:EMITTED]->(e)",
                {
                    "pod": pod,
                    "ns": namespace,
                    "cluster": self.cluster,
                    "th": b.template_hash,
                },
            )
            # Edges: LogEvent MENTIONS (ConfigMap|Secret|Service|Deployment)
            # Scan the template text + first example line; that's enough
            # to catch named references without paying for every line.
            if self.entity_index is not None:
                scan_target = b.template_text
                if b.example_lines:
                    scan_target = f"{scan_target} {b.example_lines[0]}"
                for ent_name, ent_label in self.entity_index.scan(namespace, scan_target):
                    self.graph.query(
                        f"MATCH (e:LogEvent {{cluster: $cluster, namespace: $ns, "
                        f"pod: $pod, template_hash: $th}}) "
                        f"MATCH (x:{ent_label} {{name: $ent, namespace: $ns, "
                        f"cluster: $cluster}}) "
                        f"MERGE (e)-[:MENTIONS]->(x)",
                        {
                            "cluster": self.cluster,
                            "ns": namespace,
                            "pod": pod,
                            "th": b.template_hash,
                            "ent": ent_name,
                        },
                    )
            b.dirty = False
            n += 1
        return n


# ---------------------------------------------------------------------------
# Pod log reader thread
# ---------------------------------------------------------------------------


@dataclass
class LogReaderStats:
    lines_seen: int = 0
    lines_kept: int = 0
    lines_rate_limited: int = 0
    templates: int = 0
    events_flushed: int = 0
    errors: int = 0
    last_line_ts: float = 0.0


class _PodLogReader:
    """One thread that tails a single pod's logs through the kubelet proxy.

    Uses `CoreV1Api.read_namespaced_pod_log(..., follow=True,
    _preload_content=False)`, which returns a urllib3 HTTPResponse we
    can iterate line-by-line. On disconnect we back off and reconnect.
    """

    def __init__(
        self,
        core_api: Any,
        pod: str,
        namespace: str,
        miner: PodTemplateMiner,
        writer: LogEventWriter,
        stop_event: threading.Event,
        rate_limit_per_sec: int = 1000,
        flush_interval_seconds: float = 5.0,
        tail_lines: int = 0,
    ):
        self.core = core_api
        self.pod = pod
        self.namespace = namespace
        self.miner = miner
        self.writer = writer
        self._stop = stop_event
        self.rate_limit = rate_limit_per_sec
        self.flush_interval = flush_interval_seconds
        self.tail_lines = tail_lines
        self.stats = LogReaderStats()
        self._thread: threading.Thread | None = None

    def start(self) -> None:
        self._thread = threading.Thread(
            target=self._run,
            name=f"mazkir-logs-{self.namespace}-{self.pod}",
            daemon=True,
        )
        self._thread.start()

    def _run(self) -> None:
        backoff = 1.0
        while not self._stop.is_set():
            try:
                self._stream_once()
                backoff = 1.0
            except Exception as e:
                self.stats.errors += 1
                logger.debug("log reader %s/%s error: %s", self.namespace, self.pod, e)
                if self._stop.wait(backoff):
                    return
                backoff = min(backoff * 2, 30.0)

    def _stream_once(self) -> None:
        # Rate-limit window state.
        window_start = time.time()
        window_count = 0

        last_flush = time.time()

        resp = self.core.read_namespaced_pod_log(
            name=self.pod,
            namespace=self.namespace,
            follow=True,
            _preload_content=False,
            tail_lines=self.tail_lines,
            timestamps=False,
        )

        try:
            # urllib3 HTTPResponse supports .stream() for chunk iteration,
            # but read_chunked is cleaner for line-based consumption.
            buf = b""
            for chunk in resp.stream(amt=4096, decode_content=True):
                if self._stop.is_set():
                    break
                buf += chunk
                while b"\n" in buf:
                    raw_line, buf = buf.split(b"\n", 1)
                    self._handle_line(raw_line)
                    # Rate limit
                    now = time.time()
                    if now - window_start >= 1.0:
                        window_start = now
                        window_count = 0
                    window_count += 1
                    if window_count > self.rate_limit:
                        self.stats.lines_rate_limited += 1
                        continue

                # Periodic flush
                now = time.time()
                if now - last_flush >= self.flush_interval:
                    self._flush()
                    last_flush = now
        finally:
            try:
                resp.release_conn()
            except Exception:
                pass
            self._flush()

    def _handle_line(self, raw: bytes) -> None:
        self.stats.lines_seen += 1
        try:
            line = raw.decode("utf-8", errors="replace")
        except Exception:
            return
        severity = classify_line(line)
        if severity is None:
            return
        self.stats.lines_kept += 1
        now = time.time()
        # Prefer the timestamp embedded in the log line; this is what
        # matters for MTTR queries (`since_minutes` must filter on the
        # moment the event actually happened, not when we read it).
        # Fall back to ingestion time when the line has no recognizable
        # timestamp. Guard against skew: if the parsed ts is in the
        # future, treat it as bogus and use `now`.
        parsed = extract_log_timestamp(line)
        if parsed is not None and parsed <= now + 300:
            event_ts = parsed
        else:
            event_ts = now
        self.stats.last_line_ts = now
        bucket = self.miner.ingest(line, severity, event_ts)
        if bucket is not None:
            self.stats.templates = len(self.miner.buckets)

    def _flush(self) -> None:
        n = self.writer.flush(self.pod, self.namespace, self.miner.buckets)
        self.stats.events_flushed += n


# ---------------------------------------------------------------------------
# Log watcher — manages readers for a set of pods
# ---------------------------------------------------------------------------


class LogWatcher:
    """Top-level log-intelligence manager.

    Owns per-pod readers and a shared writer. You drive it by calling
    `add_pod(name, namespace)` / `remove_pod(name, namespace)`. Later
    this will be wired into `K8sWatcher`'s pod lifecycle events so it
    automatically follows every pod in every tracked namespace.
    """

    def __init__(
        self,
        graph: GraphClient,
        cluster: str,
        core_api: Any,
        rate_limit_per_sec: int = 1000,
        flush_interval_seconds: float = 5.0,
        tail_lines: int = 0,
        retention_seconds: int = 24 * 60 * 60,
        prune_interval_seconds: float = 300.0,
    ):
        self.graph = graph
        self.cluster = cluster
        self.core = core_api
        self.rate_limit = rate_limit_per_sec
        self.flush_interval = flush_interval_seconds
        self.tail_lines = tail_lines
        # Retention: any LogEvent whose `last_seen` is older than this
        # gets deleted during the periodic prune pass. Default 24h.
        # Set to 0 to disable pruning entirely.
        self.retention_seconds = retention_seconds
        self.prune_interval = prune_interval_seconds
        self._last_prune = 0.0
        # Entity index for MENTIONS edges (configmap/secret/service names
        # referenced in log text). Refreshed from the graph on the flusher
        # cadence; see `_flush_loop`.
        self.entity_index = EntityIndex(graph, cluster)
        self.entity_refresh_interval = 60.0
        self._last_entity_refresh = 0.0
        self._stop = threading.Event()
        self._readers: dict[tuple[str, str], _PodLogReader] = {}
        self._miners: dict[tuple[str, str], PodTemplateMiner] = {}
        self.writer = LogEventWriter(graph, cluster, entity_index=self.entity_index)
        self._lock = threading.Lock()
        # Background flusher: per-reader flush cadence depends on inbound
        # chunk arrival, which is unreliable for quiet pods. A global
        # ticker guarantees dirty buckets land in the graph on schedule
        # regardless of stream activity.
        self._flusher = threading.Thread(
            target=self._flush_loop, name="mazkir-log-flusher", daemon=True
        )
        self._flusher.start()

    def _flush_loop(self) -> None:
        # Prime the entity index immediately so the first flush has
        # something to scan against.
        try:
            self.entity_index.refresh()
            self._last_entity_refresh = time.time()
        except Exception as e:
            logger.debug("initial entity index refresh failed: %s", e)

        while not self._stop.is_set():
            if self._stop.wait(self.flush_interval):
                break
            self.flush_all()
            now = time.time()
            # Entity index refresh on its own cadence
            if now - self._last_entity_refresh >= self.entity_refresh_interval:
                try:
                    self.entity_index.refresh()
                except Exception as e:
                    logger.debug("entity index refresh failed: %s", e)
                self._last_entity_refresh = now
            # Run prune at its own cadence, not every flush
            if (
                self.retention_seconds > 0
                and now - self._last_prune >= self.prune_interval
            ):
                try:
                    self.prune_stale()
                except Exception as e:
                    logger.warning("prune_stale error: %s", e)
                self._last_prune = now

    def prune_stale(self) -> int:
        """Delete LogEvent nodes older than the retention window.

        Returns the number of nodes deleted. Cheap: single Cypher
        statement with an indexed `last_seen` filter.
        """
        cutoff = time.time() - self.retention_seconds
        r = self.graph.query(
            "MATCH (e:LogEvent {cluster: $cluster}) "
            "WHERE e.last_seen < $cutoff "
            "WITH e, count(e) AS _ "
            "DETACH DELETE e "
            "RETURN count(*)",
            {"cluster": self.cluster, "cutoff": cutoff},
        )
        try:
            n = int(r.result_set[0][0]) if r.result_set else 0
        except Exception:
            n = 0
        if n > 0:
            logger.info("prune_stale: deleted %d expired LogEvent nodes", n)
        return n

    def flush_all(self) -> int:
        """Force a flush of every reader's dirty buckets."""
        total = 0
        with self._lock:
            readers = list(self._readers.values())
        for r in readers:
            try:
                r._flush()
                total += 1
            except Exception as e:
                logger.debug("flush_all error: %s", e)
        return total

    def add_pod(self, pod: str, namespace: str) -> None:
        key = (namespace, pod)
        with self._lock:
            if key in self._readers:
                return
            miner = PodTemplateMiner(pod=pod, namespace=namespace)
            reader = _PodLogReader(
                core_api=self.core,
                pod=pod,
                namespace=namespace,
                miner=miner,
                writer=self.writer,
                stop_event=self._stop,
                rate_limit_per_sec=self.rate_limit,
                flush_interval_seconds=self.flush_interval,
                tail_lines=self.tail_lines,
            )
            self._miners[key] = miner
            self._readers[key] = reader
        reader.start()
        logger.debug("log reader started for %s/%s", namespace, pod)

    def mark_pod_deleted(self, pod: str, namespace: str) -> None:
        """Called when K8s reports a pod DELETED.

        Stops the tail thread (no more logs will come) and stamps
        existing LogEvent nodes with `pod_deleted_at` so downstream
        tooling can distinguish "pod is still emitting this" from
        "pod is gone but the crash story is still within retention."

        Events are *not* deleted — retention GC handles that. This
        preserves post-mortem evidence for the window that matters.
        """
        self.remove_pod(pod, namespace, purge_events=False)
        try:
            self.graph.query(
                "MATCH (e:LogEvent {cluster: $cluster, namespace: $ns, pod: $pod}) "
                "SET e.pod_deleted_at = $ts",
                {
                    "cluster": self.cluster,
                    "ns": namespace,
                    "pod": pod,
                    "ts": time.time(),
                },
            )
        except Exception as e:
            logger.debug("mark_pod_deleted query failed: %s", e)

    def remove_pod(self, pod: str, namespace: str, purge_events: bool = False) -> None:
        """Stop tailing a pod. Optionally delete its stored LogEvents.

        `purge_events=True` is appropriate when the pod has been deleted
        from the cluster and its story is no longer relevant. For temporary
        unsubscribes (e.g. rate-limit lockout) leave events in place so
        historical queries still resolve.
        """
        key = (namespace, pod)
        with self._lock:
            reader = self._readers.pop(key, None)
            self._miners.pop(key, None)
        if reader is not None:
            reader._flush()
        if purge_events:
            try:
                self.graph.query(
                    "MATCH (e:LogEvent {cluster: $cluster, namespace: $ns, pod: $pod}) "
                    "DETACH DELETE e",
                    {"cluster": self.cluster, "ns": namespace, "pod": pod},
                )
            except Exception as e:
                logger.debug("purge_events error for %s/%s: %s", namespace, pod, e)

    def stop(self) -> None:
        self._stop.set()
        # Threads are daemon + blocked on HTTP stream; they die with the process.

    def stats(self) -> dict[tuple[str, str], LogReaderStats]:
        with self._lock:
            return {k: r.stats for k, r in self._readers.items()}
