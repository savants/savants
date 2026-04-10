"""Code Property Graph schema definitions for FalkorDB.

Layer 1 — current state (the live code property graph):

  Node types:
    - File: source file in the repository
    - Function: function/method definition
    - Class: class definition
    - Module: logical module or package
    - Variable: global/module-level variable or constant

  Edge types:
    - CONTAINS: File -> Function/Class/Variable
    - CALLS: Function -> Function
    - INHERITS_FROM: Class -> Class
    - IMPORTS: File -> File/Module
    - DEPENDS_ON: Module -> Module
    - DEFINES: Class -> Function (methods)
    - REFERENCES: Function -> Variable

Layer 2 — history (the time-travel overlay, see docs/architecture-layered-graphs.md):

  Node types:
    - Episode: a discrete event (git commit, chat message, agent action)

  Edge types:
    - CHANGES: Episode -> File/Function/Class
        properties: op ('add'|'remove'|'modify'|'rename'),
                    before_props, after_props
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime

# Cypher schema creation queries — both layers
SCHEMA_INDICES = [
    # Layer 1
    "CREATE INDEX FOR (f:File) ON (f.path)",
    "CREATE INDEX FOR (fn:Function) ON (fn.name)",
    "CREATE INDEX FOR (c:Class) ON (c.name)",
    "CREATE INDEX FOR (m:Module) ON (m.name)",
    # Layer 2 — history
    "CREATE INDEX FOR (e:Episode) ON (e.sha)",
    "CREATE INDEX FOR (e:Episode) ON (e.timestamp)",
    "CREATE INDEX FOR (e:Episode) ON (e.author)",
    "CREATE INDEX FOR (e:Episode) ON (e.branch)",
    "CREATE INDEX FOR (e:Episode) ON (e.source_type)",
    "CREATE INDEX FOR (fn:Function) ON (fn.decorators)",
    "CREATE INDEX FOR (k:ConfigKey) ON (k.name)",
    "CREATE INDEX FOR (k:ConfigKey) ON (k.file_path)",
    "CREATE INDEX FOR (fn:Function) ON (fn.class_name)",
    "CREATE INDEX FOR (e:EnvVar) ON (e.name)",
    "CREATE INDEX FOR (d:Decorator) ON (d.name)",
    # K8s runtime layer indices
    "CREATE INDEX FOR (c:K8sCluster) ON (c.name)",
    "CREATE INDEX FOR (n:K8sNamespace) ON (n.name)",
    "CREATE INDEX FOR (n:K8sNamespace) ON (n.cluster)",
    "CREATE INDEX FOR (d:K8sDeployment) ON (d.name)",
    "CREATE INDEX FOR (d:K8sDeployment) ON (d.namespace)",
    "CREATE INDEX FOR (p:K8sPod) ON (p.name)",
    "CREATE INDEX FOR (p:K8sPod) ON (p.namespace)",
    "CREATE INDEX FOR (p:K8sPod) ON (p.status)",
    "CREATE INDEX FOR (s:K8sService) ON (s.name)",
    "CREATE INDEX FOR (cm:K8sConfigMap) ON (cm.name)",
    "CREATE INDEX FOR (sec:K8sSecret) ON (sec.name)",
    # Log intelligence layer
    "CREATE INDEX FOR (e:LogEvent) ON (e.template_hash)",
    "CREATE INDEX FOR (e:LogEvent) ON (e.pod)",
    "CREATE INDEX FOR (e:LogEvent) ON (e.namespace)",
    "CREATE INDEX FOR (e:LogEvent) ON (e.cluster)",
    "CREATE INDEX FOR (e:LogEvent) ON (e.severity)",
    "CREATE INDEX FOR (e:LogEvent) ON (e.last_seen)",
    # Host agent layer
    "CREATE INDEX FOR (h:Host) ON (h.hostname)",
    "CREATE INDEX FOR (p:HostProcess) ON (p.pid)",
    "CREATE INDEX FOR (p:HostProcess) ON (p.hostname)",
    "CREATE INDEX FOR (u:SystemdUnit) ON (u.name)",
    "CREATE INDEX FOR (u:SystemdUnit) ON (u.hostname)",
    "CREATE INDEX FOR (d:HostDisk) ON (d.mountpoint)",
    "CREATE INDEX FOR (n:HostNetIface) ON (n.name)",
    "CREATE INDEX FOR (he:HostLogEvent) ON (he.hostname)",
    "CREATE INDEX FOR (he:HostLogEvent) ON (he.template_hash)",
    "CREATE INDEX FOR (he:HostLogEvent) ON (he.severity)",
    "CREATE INDEX FOR (he:HostLogEvent) ON (he.last_seen)",
    "CREATE INDEX FOR (dc:DockerContainer) ON (dc.hostname)",
    "CREATE INDEX FOR (dc:DockerContainer) ON (dc.container_id)",
    "CREATE INDEX FOR (ke:KernelEvent) ON (ke.hostname)",
    "CREATE INDEX FOR (ke:KernelEvent) ON (ke.category)",
    "CREATE INDEX FOR (ke:KernelEvent) ON (ke.severity)",
]


@dataclass
class FileNode:
    path: str
    language: str
    line_count: int
    sha256: str  # provenance hash
    last_commit: str = ""


@dataclass
class FunctionNode:
    name: str
    file_path: str
    start_line: int
    end_line: int
    parameters: list[str] = field(default_factory=list)
    return_type: str = ""
    decorators: list[str] = field(default_factory=list)
    docstring: str = ""
    class_name: str = ""  # populated for methods; "" for top-level functions


@dataclass
class ClassNode:
    name: str
    file_path: str
    start_line: int
    end_line: int
    bases: list[str] = field(default_factory=list)
    docstring: str = ""
    decorators: list[str] = field(default_factory=list)


@dataclass
class EnvVarNode:
    """Environment variable referenced from application code.

    Captured from os.getenv(...), os.environ[...], and process.env.X.
    Lets us answer "what env vars does this service read?" which
    ConfigKey indexing can't answer alone (env vars live in Secrets,
    Helm values, .env files — often not in the repo at all).
    """

    name: str  # the env var name (e.g. "FALKORDB_HOST")
    file_path: str
    default_value: str = ""  # if os.getenv("X", "default"), capture the default


@dataclass
class K8sClusterNode:
    """A Kubernetes cluster (top-level scope for the runtime layer).

    Stable ID format: K8sCluster:{cluster_name}
    The cluster_name matches the kubeconfig context (e.g. 'astra-k3s').
    """

    name: str
    version: str = ""  # k8s server version
    context: str = ""  # kubeconfig context


@dataclass
class K8sNamespaceNode:
    """A Kubernetes namespace within a cluster.

    Stable ID format: K8sNamespace:{cluster}/{namespace}
    """

    name: str
    cluster: str
    status: str = "Active"  # Active, Terminating
    age_seconds: int = 0
    resource_version: str = ""  # etcd ModRevision, used for diff-based ingest


@dataclass
class K8sDeploymentNode:
    """A Kubernetes Deployment (or StatefulSet / DaemonSet — same node type
    with a `kind` field to distinguish).

    Stable ID format: K8sDeployment:{cluster}/{namespace}/{name}
    """

    name: str
    namespace: str
    cluster: str
    kind: str = "Deployment"  # Deployment | StatefulSet | DaemonSet
    replicas_desired: int = 0
    replicas_ready: int = 0
    replicas_available: int = 0
    image: str = ""  # primary container image
    labels: list[str] = field(default_factory=list)  # flattened key=value
    resource_version: str = ""


@dataclass
class K8sPodNode:
    """A running Kubernetes Pod instance.

    Stable ID format: K8sPod:{cluster}/{namespace}/{name}
    """

    name: str
    namespace: str
    cluster: str
    status: str = "Running"  # Running | Pending | Failed | Succeeded | CrashLoopBackOff
    node_name: str = ""  # which k8s node is running this pod
    restart_count: int = 0
    ready: bool = False
    image: str = ""
    owner_kind: str = ""  # ReplicaSet, StatefulSet, DaemonSet, etc.
    owner_name: str = ""
    resource_version: str = ""


@dataclass
class K8sServiceNode:
    """A Kubernetes Service.

    Stable ID format: K8sService:{cluster}/{namespace}/{name}
    """

    name: str
    namespace: str
    cluster: str
    type: str = "ClusterIP"  # ClusterIP | NodePort | LoadBalancer | ExternalName
    cluster_ip: str = ""
    ports: list[str] = field(default_factory=list)  # "80/TCP", "443/TCP"
    selector: list[str] = field(default_factory=list)  # flattened key=value
    resource_version: str = ""


@dataclass
class K8sConfigMapNode:
    """A ConfigMap in a namespace. Stores only the key names, NOT the values
    (values could contain secrets or customer data).

    Stable ID format: K8sConfigMap:{cluster}/{namespace}/{name}
    """

    name: str
    namespace: str
    cluster: str
    key_names: list[str] = field(default_factory=list)  # just the keys, no values
    resource_version: str = ""


@dataclass
class K8sSecretNode:
    """A Secret in a namespace. Stores ONLY the secret name and key names —
    never the secret values. The whole point of the secret scrubber applies
    doubly here.

    Stable ID format: K8sSecret:{cluster}/{namespace}/{name}
    """

    name: str
    namespace: str
    cluster: str
    type: str = "Opaque"  # Opaque | kubernetes.io/tls | kubernetes.io/dockerconfigjson
    key_names: list[str] = field(default_factory=list)  # just the keys
    resource_version: str = ""


@dataclass
class LogEventNode:
    """A deduplicated, significant log event derived from a pod's stdout/stderr.

    One LogEventNode represents *one template* (via drain3) emitted by one pod,
    aggregated across all occurrences. Repeated occurrences update `count`,
    `last_seen`, and rotate into `example_lines` (capped). This keeps the graph
    small (~1 node per distinct pattern per pod) while preserving enough raw
    text for LLM narration and human inspection.

    Stable ID format: LogEvent:{cluster}/{namespace}/{pod}/{template_hash}
    """

    template_hash: str  # drain3 cluster id, stable within a pod's lifetime
    pod: str
    namespace: str
    cluster: str
    severity: str = "INFO"  # INFO | WARN | ERROR | FATAL
    template_text: str = ""  # drain3 parameterized template, e.g. "user <*> failed <*>"
    first_seen: float = 0.0  # unix ts
    last_seen: float = 0.0  # unix ts
    count: int = 0  # total occurrences seen
    example_lines: list[str] = field(default_factory=list)  # capped sample, last N


@dataclass
class HostNode:
    """The machine itself — physical server, VM, or dev laptop.

    Stable ID format: Host:{hostname}
    """

    hostname: str
    os: str = ""  # e.g. "Linux 6.19.7"
    kernel: str = ""
    uptime_seconds: int = 0
    cpu_count: int = 0
    cpu_percent: float = 0.0
    memory_total_mb: int = 0
    memory_used_mb: int = 0
    memory_percent: float = 0.0
    swap_total_mb: int = 0
    swap_used_mb: int = 0
    load_1m: float = 0.0
    load_5m: float = 0.0
    load_15m: float = 0.0


@dataclass
class HostDiskNode:
    """A mounted filesystem on a host.

    Stable ID format: HostDisk:{hostname}:{mountpoint}
    """

    hostname: str
    mountpoint: str
    device: str = ""
    fstype: str = ""
    total_gb: float = 0.0
    used_gb: float = 0.0
    free_gb: float = 0.0
    percent: float = 0.0


@dataclass
class HostNetIfaceNode:
    """A network interface on a host.

    Stable ID format: HostNetIface:{hostname}:{name}
    """

    hostname: str
    name: str
    ipv4: str = ""
    ipv6: str = ""
    mac: str = ""
    state: str = ""  # up / down
    mtu: int = 0


@dataclass
class HostProcessNode:
    """A running process, captured as a snapshot of top-N by resource usage.

    Stable ID format: HostProcess:{hostname}:{pid}
    """

    hostname: str
    pid: int
    name: str = ""
    cmdline: str = ""
    cpu_percent: float = 0.0
    memory_mb: float = 0.0
    user: str = ""
    status: str = ""  # running / sleeping / zombie / etc.


@dataclass
class SystemdUnitNode:
    """A systemd unit (service, timer, mount, etc.).

    Stable ID format: SystemdUnit:{hostname}:{name}
    """

    hostname: str
    name: str
    type: str = "service"  # service / timer / mount / socket
    active_state: str = ""  # active / inactive / failed / activating
    sub_state: str = ""  # running / dead / exited / waiting
    description: str = ""


@dataclass
class DockerContainerNode:
    """A Docker container running on the host (non-K8s workloads).

    For hosts that run Docker directly (not via K8s), this captures
    standalone containers. K8s pods are handled by the K8s ingestor.

    Stable ID format: DockerContainer:{hostname}:{container_id[:12]}
    """

    hostname: str
    container_id: str  # short 12-char id
    name: str = ""
    image: str = ""
    status: str = ""  # running / exited / created / paused
    state: str = ""  # health status if available
    ports: list[str] = field(default_factory=list)  # "8080/tcp->80"
    created_at: str = ""
    restart_count: int = 0


@dataclass
class KernelEventNode:
    """A significant kernel-level event from dmesg.

    Captures OOM kills, hardware errors, NFS stalls, disk I/O errors,
    segfaults, kernel panics — the things that explain why userspace
    misbehaves but are invisible to application-level monitoring.

    Stable ID format: KernelEvent:{hostname}:{template_hash}
    """

    hostname: str
    template_hash: str
    category: str = ""  # oom / io_error / segfault / hardware / nfs / panic
    severity: str = "WARN"
    template_text: str = ""
    first_seen: float = 0.0
    last_seen: float = 0.0
    count: int = 0
    example_lines: list[str] = field(default_factory=list)


@dataclass
class HostLogEventNode:
    """A significant log event from the host (journald / dmesg / syslog).

    Same pattern as LogEventNode for K8s pods — deduplicated by drain3
    template, scoped to the hostname instead of a pod.

    Stable ID format: HostLogEvent:{hostname}:{source}:{template_hash}
    """

    template_hash: str
    hostname: str
    source: str = ""  # dmesg / journald / syslog
    unit: str = ""  # systemd unit name if from journald
    severity: str = "INFO"
    template_text: str = ""
    first_seen: float = 0.0
    last_seen: float = 0.0
    count: int = 0
    example_lines: list[str] = field(default_factory=list)


@dataclass
class DecoratorNode:
    """A decorator name interned as a graph node.

    Functions decorated with the same decorator share the same
    Decorator node (MERGE on name). The DECORATED_BY edge from
    Function to Decorator is what `decorated_with` queries — using
    the indexed `name` property turns the lookup from a full Function
    scan (~173ms on zora_backend) into an O(matches) indexed query
    (~5ms).
    """

    name: str  # the decorator's callable name, e.g. "workflow.defn"


@dataclass
class ConfigKeyNode:
    """A leaf key in a config file (YAML/TOML/JSON).

    `name` is the dotted key path (e.g. 'operationProfiling.mode') so that
    `search_code` — which matches on `name CONTAINS pattern` — finds it.
    """

    name: str  # dotted key path, used as the searchable identifier
    file_path: str
    value: str  # stringified leaf value (truncated)
    format: str  # 'yaml' | 'toml' | 'json'
    line: int = 0


@dataclass
class ProvenanceStamp:
    """SHA-256 provenance attached to every graph entry."""

    source_commit: str
    author: str
    timestamp: str
    content_hash: str


def create_file_query(node: FileNode) -> tuple[str, dict]:
    return (
        "MERGE (f:File {path: $path}) "
        "SET f.language = $language, f.line_count = $line_count, "
        "f.sha256 = $sha256, f.last_commit = $last_commit",
        {
            "path": node.path,
            "language": node.language,
            "line_count": node.line_count,
            "sha256": node.sha256,
            "last_commit": node.last_commit,
        },
    )


def create_function_query(node: FunctionNode) -> tuple[str, dict]:
    return (
        "MERGE (fn:Function {name: $name, file_path: $file_path}) "
        "SET fn.start_line = $start_line, fn.end_line = $end_line, "
        "fn.parameters = $parameters, fn.return_type = $return_type, "
        "fn.decorators = $decorators, fn.docstring = $docstring, "
        "fn.class_name = $class_name",
        {
            "name": node.name,
            "file_path": node.file_path,
            "start_line": node.start_line,
            "end_line": node.end_line,
            "parameters": node.parameters,
            "return_type": node.return_type,
            "decorators": node.decorators,
            "docstring": node.docstring,
            "class_name": node.class_name,
        },
    )


def create_class_query(node: ClassNode) -> tuple[str, dict]:
    return (
        "MERGE (c:Class {name: $name, file_path: $file_path}) "
        "SET c.start_line = $start_line, c.end_line = $end_line, "
        "c.bases = $bases, c.docstring = $docstring, "
        "c.decorators = $decorators",
        {
            "name": node.name,
            "file_path": node.file_path,
            "start_line": node.start_line,
            "end_line": node.end_line,
            "bases": node.bases,
            "docstring": node.docstring,
            "decorators": node.decorators,
        },
    )


def create_env_var_query(node: EnvVarNode) -> tuple[str, dict]:
    return (
        "MERGE (e:EnvVar {name: $name}) "
        "SET e.default_value = COALESCE(e.default_value, $default_value)",
        {"name": node.name, "default_value": node.default_value},
    )


def create_decorator_query(name: str) -> tuple[str, dict]:
    """MERGE a Decorator node by name. The name is the canonical key —
    every function decorated with `@workflow.defn` shares the same
    Decorator{name:'workflow.defn'} node.
    """
    return ("MERGE (d:Decorator {name: $name})", {"name": name})


# ---------------------------------------------------------------------------
# K8s runtime layer query helpers
# ---------------------------------------------------------------------------


def create_k8s_cluster_query(node: K8sClusterNode) -> tuple[str, dict]:
    return (
        "MERGE (c:K8sCluster {name: $name}) "
        "SET c.version = $version, c.context = $context",
        {"name": node.name, "version": node.version, "context": node.context},
    )


def create_k8s_namespace_query(node: K8sNamespaceNode) -> tuple[str, dict]:
    return (
        "MERGE (n:K8sNamespace {name: $name, cluster: $cluster}) "
        "SET n.status = $status, n.age_seconds = $age_seconds, "
        "n.resource_version = $rv",
        {
            "name": node.name,
            "cluster": node.cluster,
            "status": node.status,
            "age_seconds": node.age_seconds,
            "rv": node.resource_version,
        },
    )


def create_k8s_deployment_query(node: K8sDeploymentNode) -> tuple[str, dict]:
    return (
        "MERGE (d:K8sDeployment {name: $name, namespace: $namespace, cluster: $cluster}) "
        "SET d.kind = $kind, d.replicas_desired = $rd, d.replicas_ready = $rr, "
        "d.replicas_available = $ra, d.image = $image, d.labels = $labels, "
        "d.resource_version = $rv",
        {
            "name": node.name,
            "namespace": node.namespace,
            "cluster": node.cluster,
            "kind": node.kind,
            "rd": node.replicas_desired,
            "rr": node.replicas_ready,
            "ra": node.replicas_available,
            "image": node.image,
            "labels": node.labels,
            "rv": node.resource_version,
        },
    )


def create_k8s_pod_query(node: K8sPodNode) -> tuple[str, dict]:
    return (
        "MERGE (p:K8sPod {name: $name, namespace: $namespace, cluster: $cluster}) "
        "SET p.status = $status, p.node_name = $node_name, "
        "p.restart_count = $rc, p.ready = $ready, p.image = $image, "
        "p.owner_kind = $owner_kind, p.owner_name = $owner_name, "
        "p.resource_version = $rv",
        {
            "name": node.name,
            "namespace": node.namespace,
            "cluster": node.cluster,
            "status": node.status,
            "node_name": node.node_name,
            "rc": node.restart_count,
            "ready": node.ready,
            "image": node.image,
            "owner_kind": node.owner_kind,
            "owner_name": node.owner_name,
            "rv": node.resource_version,
        },
    )


def create_k8s_service_query(node: K8sServiceNode) -> tuple[str, dict]:
    return (
        "MERGE (s:K8sService {name: $name, namespace: $namespace, cluster: $cluster}) "
        "SET s.type = $type, s.cluster_ip = $cluster_ip, "
        "s.ports = $ports, s.selector = $selector, s.resource_version = $rv",
        {
            "name": node.name,
            "namespace": node.namespace,
            "cluster": node.cluster,
            "type": node.type,
            "cluster_ip": node.cluster_ip,
            "ports": node.ports,
            "selector": node.selector,
            "rv": node.resource_version,
        },
    )


def create_k8s_configmap_query(node: K8sConfigMapNode) -> tuple[str, dict]:
    return (
        "MERGE (cm:K8sConfigMap {name: $name, namespace: $namespace, cluster: $cluster}) "
        "SET cm.key_names = $keys, cm.resource_version = $rv",
        {
            "name": node.name,
            "namespace": node.namespace,
            "cluster": node.cluster,
            "keys": node.key_names,
            "rv": node.resource_version,
        },
    )


def create_k8s_secret_query(node: K8sSecretNode) -> tuple[str, dict]:
    return (
        "MERGE (sec:K8sSecret {name: $name, namespace: $namespace, cluster: $cluster}) "
        "SET sec.type = $type, sec.key_names = $keys, sec.resource_version = $rv",
        {
            "name": node.name,
            "namespace": node.namespace,
            "cluster": node.cluster,
            "type": node.type,
            "keys": node.key_names,
            "rv": node.resource_version,
        },
    )


def create_log_event_query(node: LogEventNode) -> tuple[str, dict]:
    """MERGE a LogEvent by (cluster, namespace, pod, template_hash).

    Repeated occurrences update `count`, `last_seen`, and `example_lines`
    without creating duplicate nodes. The caller is expected to pass the
    *cumulative* count and the *refreshed* example_lines list (drain3
    mining state lives in the ingestor, not the graph).
    """
    return (
        "MERGE (e:LogEvent {cluster: $cluster, namespace: $namespace, "
        "pod: $pod, template_hash: $th}) "
        "SET e.severity = $severity, e.template_text = $template_text, "
        "e.last_seen = $last_seen, e.count = $count, "
        "e.example_lines = $example_lines, "
        "e.first_seen = CASE WHEN e.first_seen IS NULL OR e.first_seen = 0 "
        "                    THEN $first_seen ELSE e.first_seen END",
        {
            "cluster": node.cluster,
            "namespace": node.namespace,
            "pod": node.pod,
            "th": node.template_hash,
            "severity": node.severity,
            "template_text": node.template_text,
            "first_seen": node.first_seen,
            "last_seen": node.last_seen,
            "count": node.count,
            "example_lines": node.example_lines,
        },
    )


def create_host_query(node: HostNode) -> tuple[str, dict]:
    return (
        "MERGE (h:Host {hostname: $hostname}) "
        "SET h.os = $os, h.kernel = $kernel, h.uptime_seconds = $uptime, "
        "h.cpu_count = $cpus, h.cpu_percent = $cpu_pct, "
        "h.memory_total_mb = $mem_total, h.memory_used_mb = $mem_used, "
        "h.memory_percent = $mem_pct, h.swap_total_mb = $swap_total, "
        "h.swap_used_mb = $swap_used, h.load_1m = $l1, h.load_5m = $l5, "
        "h.load_15m = $l15",
        {
            "hostname": node.hostname, "os": node.os, "kernel": node.kernel,
            "uptime": node.uptime_seconds, "cpus": node.cpu_count,
            "cpu_pct": node.cpu_percent, "mem_total": node.memory_total_mb,
            "mem_used": node.memory_used_mb, "mem_pct": node.memory_percent,
            "swap_total": node.swap_total_mb, "swap_used": node.swap_used_mb,
            "l1": node.load_1m, "l5": node.load_5m, "l15": node.load_15m,
        },
    )


def create_host_disk_query(node: HostDiskNode) -> tuple[str, dict]:
    return (
        "MERGE (d:HostDisk {hostname: $hostname, mountpoint: $mp}) "
        "SET d.device = $dev, d.fstype = $fs, d.total_gb = $total, "
        "d.used_gb = $used, d.free_gb = $free, d.percent = $pct",
        {
            "hostname": node.hostname, "mp": node.mountpoint,
            "dev": node.device, "fs": node.fstype,
            "total": node.total_gb, "used": node.used_gb,
            "free": node.free_gb, "pct": node.percent,
        },
    )


def create_host_net_iface_query(node: HostNetIfaceNode) -> tuple[str, dict]:
    return (
        "MERGE (n:HostNetIface {hostname: $hostname, name: $name}) "
        "SET n.ipv4 = $ipv4, n.ipv6 = $ipv6, n.mac = $mac, "
        "n.state = $state, n.mtu = $mtu",
        {
            "hostname": node.hostname, "name": node.name,
            "ipv4": node.ipv4, "ipv6": node.ipv6, "mac": node.mac,
            "state": node.state, "mtu": node.mtu,
        },
    )


def create_host_process_query(node: HostProcessNode) -> tuple[str, dict]:
    return (
        "MERGE (p:HostProcess {hostname: $hostname, pid: $pid}) "
        "SET p.name = $name, p.cmdline = $cmd, p.cpu_percent = $cpu, "
        "p.memory_mb = $mem, p.user = $user, p.status = $status",
        {
            "hostname": node.hostname, "pid": node.pid,
            "name": node.name, "cmd": node.cmdline,
            "cpu": node.cpu_percent, "mem": node.memory_mb,
            "user": node.user, "status": node.status,
        },
    )


def create_systemd_unit_query(node: SystemdUnitNode) -> tuple[str, dict]:
    return (
        "MERGE (u:SystemdUnit {hostname: $hostname, name: $name}) "
        "SET u.type = $type, u.active_state = $active, "
        "u.sub_state = $sub, u.description = $desc",
        {
            "hostname": node.hostname, "name": node.name,
            "type": node.type, "active": node.active_state,
            "sub": node.sub_state, "desc": node.description,
        },
    )


def create_docker_container_query(node: DockerContainerNode) -> tuple[str, dict]:
    return (
        "MERGE (c:DockerContainer {hostname: $hostname, container_id: $cid}) "
        "SET c.name = $name, c.image = $image, c.status = $status, "
        "c.state = $state, c.ports = $ports, c.created_at = $created, "
        "c.restart_count = $rc",
        {
            "hostname": node.hostname, "cid": node.container_id,
            "name": node.name, "image": node.image,
            "status": node.status, "state": node.state,
            "ports": node.ports, "created": node.created_at,
            "rc": node.restart_count,
        },
    )


def create_kernel_event_query(node: KernelEventNode) -> tuple[str, dict]:
    return (
        "MERGE (e:KernelEvent {hostname: $hostname, template_hash: $th}) "
        "SET e.category = $cat, e.severity = $severity, "
        "e.template_text = $tmpl, e.last_seen = $last_seen, "
        "e.count = $count, e.example_lines = $examples, "
        "e.first_seen = CASE WHEN e.first_seen IS NULL OR e.first_seen = 0 "
        "                    THEN $first_seen ELSE e.first_seen END",
        {
            "hostname": node.hostname, "th": node.template_hash,
            "cat": node.category, "severity": node.severity,
            "tmpl": node.template_text, "first_seen": node.first_seen,
            "last_seen": node.last_seen, "count": node.count,
            "examples": node.example_lines,
        },
    )


def create_host_log_event_query(node: HostLogEventNode) -> tuple[str, dict]:
    return (
        "MERGE (e:HostLogEvent {hostname: $hostname, source: $source, "
        "template_hash: $th}) "
        "SET e.unit = $unit, e.severity = $severity, "
        "e.template_text = $tmpl, e.last_seen = $last_seen, "
        "e.count = $count, e.example_lines = $examples, "
        "e.first_seen = CASE WHEN e.first_seen IS NULL OR e.first_seen = 0 "
        "                    THEN $first_seen ELSE e.first_seen END",
        {
            "hostname": node.hostname, "source": node.source,
            "th": node.template_hash, "unit": node.unit,
            "severity": node.severity, "tmpl": node.template_text,
            "first_seen": node.first_seen, "last_seen": node.last_seen,
            "count": node.count, "examples": node.example_lines,
        },
    )


def create_config_key_query(node: ConfigKeyNode) -> tuple[str, dict]:
    """MERGE a ConfigKey by (file_path, name). Multiple files can declare the
    same key path — they're separate nodes.
    """
    return (
        "MERGE (k:ConfigKey {file_path: $file_path, name: $name}) "
        "SET k.value = $value, k.format = $format, k.line = $line",
        {
            "name": node.name,
            "file_path": node.file_path,
            "value": node.value,
            "format": node.format,
            "line": node.line,
        },
    )


def create_edge_query(
    from_label: str, from_key: str, from_val: str,
    to_label: str, to_key: str, to_val: str,
    edge_type: str,
) -> tuple[str, dict]:
    return (
        f"MATCH (a:{from_label} {{{from_key}: $from_val}}) "
        f"MATCH (b:{to_label} {{{to_key}: $to_val}}) "
        f"MERGE (a)-[:{edge_type}]->(b)",
        {"from_val": from_val, "to_val": to_val},
    )


# --- Layer 2: History (Episode + CHANGES) ------------------------------------


@dataclass
class EpisodeNode:
    """A discrete event in the history layer (commit, chat, agent action)."""

    sha: str  # commit SHA, message ID, or other unique identifier
    source_type: str = "git_commit"  # "git_commit" | "chat" | "agent_action" | ...
    timestamp: str = ""  # ISO8601 datetime string
    author: str = ""
    message: str = ""
    branch: str = "main"


@dataclass
class ChangeProps:
    """Properties on a CHANGES edge from an Episode to a Layer 1 node."""

    op: str  # "add" | "remove" | "modify" | "rename"
    before_props: dict | None = None  # state before this commit
    after_props: dict | None = None  # state after this commit


def create_episode_query(node: EpisodeNode) -> tuple[str, dict]:
    """MERGE an Episode node by SHA. SHA is the natural key."""
    return (
        "MERGE (e:Episode {sha: $sha}) "
        "SET e.source_type = $source_type, "
        "    e.timestamp = $timestamp, "
        "    e.author = $author, "
        "    e.message = $message, "
        "    e.branch = $branch",
        {
            "sha": node.sha,
            "source_type": node.source_type,
            "timestamp": node.timestamp,
            "author": node.author,
            "message": node.message,
            "branch": node.branch,
        },
    )


def create_changes_edge_query(
    episode_sha: str,
    target_label: str,
    target_key: str,
    target_val: str,
    op: str,
    before_props: dict | None = None,
    after_props: dict | None = None,
    file_path: str | None = None,
) -> tuple[str, dict]:
    """Create a CHANGES edge from an Episode to a Layer 1 node.

    For Function/Class targets, file_path is needed because their canonical
    identity is (name, file_path), not name alone.
    """
    import json

    if file_path and target_label in ("Function", "Class"):
        match_target = (
            f"MATCH (b:{target_label} {{{target_key}: $target_val, "
            f"file_path: $file_path}}) "
        )
    else:
        match_target = f"MATCH (b:{target_label} {{{target_key}: $target_val}}) "

    cypher = (
        "MATCH (e:Episode {sha: $episode_sha}) "
        + match_target
        + "MERGE (e)-[c:CHANGES]->(b) "
        "SET c.op = $op, "
        "    c.before_props = $before_props_json, "
        "    c.after_props = $after_props_json"
    )
    return (
        cypher,
        {
            "episode_sha": episode_sha,
            "target_val": target_val,
            "file_path": file_path or "",
            "op": op,
            "before_props_json": json.dumps(before_props or {}),
            "after_props_json": json.dumps(after_props or {}),
        },
    )
