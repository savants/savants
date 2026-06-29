//! Single source of truth for all graph property names.
//!
//! Both the ingestor (write path) and the MCP server (read path)
//! import from here. If a field name changes, it changes everywhere.
//! If the ingestor uses a constant the MCP server doesn't know about,
//! it's a compile error — not a silent runtime mismatch.
//!
//! This module eliminates the #1 bug category in Savants: field name
//! mismatches between write and read paths.

// ── Host ──

pub mod host {
    pub const LABEL: &str = "Host";
    pub const HOSTNAME: &str = "hostname";
    pub const OS: &str = "os";
    pub const KERNEL: &str = "kernel";
    pub const UPTIME_SECONDS: &str = "uptime_seconds";
    pub const CPU_COUNT: &str = "cpu_count";
    pub const CPU_PERCENT: &str = "cpu_percent";
    pub const MEMORY_TOTAL_MB: &str = "memory_total_mb";
    pub const MEMORY_USED_MB: &str = "memory_used_mb";
    pub const MEMORY_PERCENT: &str = "memory_percent";
    pub const SWAP_TOTAL_MB: &str = "swap_total_mb";
    pub const SWAP_USED_MB: &str = "swap_used_mb";
    pub const LOAD_1M: &str = "load_1m";
    pub const LOAD_5M: &str = "load_5m";
    pub const LOAD_15M: &str = "load_15m";
}

pub mod disk {
    pub const LABEL: &str = "HostDisk";
    pub const HOSTNAME: &str = "hostname";
    pub const MOUNTPOINT: &str = "mountpoint";
    pub const DEVICE: &str = "device";
    pub const FSTYPE: &str = "fstype";
    pub const TOTAL_GB: &str = "total_gb";
    pub const USED_GB: &str = "used_gb";
    pub const FREE_GB: &str = "free_gb";
    pub const PERCENT: &str = "percent";
}

pub mod net_iface {
    pub const LABEL: &str = "HostNetIface";
    pub const HOSTNAME: &str = "hostname";
    pub const NAME: &str = "name";
    pub const IPV4: &str = "ipv4";
    pub const IPV6: &str = "ipv6";
    pub const MAC: &str = "mac";
    pub const STATE: &str = "state";
    pub const MTU: &str = "mtu";
}

pub mod process {
    pub const LABEL: &str = "HostProcess";
    pub const HOSTNAME: &str = "hostname";
    pub const PID: &str = "pid";
    pub const NAME: &str = "name";
    pub const CMDLINE: &str = "cmdline";
    pub const CPU_PERCENT: &str = "cpu_percent";
    pub const MEMORY_MB: &str = "memory_mb";
    pub const USER: &str = "user";
    pub const STATUS: &str = "status";
}

pub mod systemd_unit {
    pub const LABEL: &str = "SystemdUnit";
    pub const HOSTNAME: &str = "hostname";
    pub const NAME: &str = "name";
    pub const TYPE: &str = "type";
    pub const ACTIVE_STATE: &str = "active_state";
    pub const SUB_STATE: &str = "sub_state";
    pub const DESCRIPTION: &str = "description";
}

pub mod docker_container {
    pub const LABEL: &str = "DockerContainer";
    pub const HOSTNAME: &str = "hostname";
    pub const CONTAINER_ID: &str = "container_id";
    pub const NAME: &str = "name";
    pub const IMAGE: &str = "image";
    pub const STATUS: &str = "status";
    pub const STATE: &str = "state";
    pub const PORTS: &str = "ports";
    pub const CREATED_AT: &str = "created_at";
    pub const RESTART_COUNT: &str = "restart_count";
}

pub mod host_log_event {
    pub const LABEL: &str = "HostLogEvent";
    pub const HOSTNAME: &str = "hostname";
    pub const TEMPLATE_HASH: &str = "template_hash";
    pub const SOURCE: &str = "source";
    pub const UNIT: &str = "unit";
    pub const SEVERITY: &str = "severity";
    pub const TEMPLATE_TEXT: &str = "template_text";
    pub const FIRST_SEEN: &str = "first_seen";
    pub const LAST_SEEN: &str = "last_seen";
    pub const COUNT: &str = "count";
    pub const EXAMPLE_LINES: &str = "example_lines";
}

pub mod kernel_event {
    pub const LABEL: &str = "KernelEvent";
    pub const HOSTNAME: &str = "hostname";
    pub const TEMPLATE_HASH: &str = "template_hash";
    pub const CATEGORY: &str = "category";
    pub const SEVERITY: &str = "severity";
    pub const TEMPLATE_TEXT: &str = "template_text";
    pub const FIRST_SEEN: &str = "first_seen";
    pub const LAST_SEEN: &str = "last_seen";
    pub const COUNT: &str = "count";
    pub const EXAMPLE_LINES: &str = "example_lines";
}

// ── K8s ──

pub mod k8s_cluster {
    pub const LABEL: &str = "K8sCluster";
    pub const NAME: &str = "name";
    pub const VERSION: &str = "version";
    pub const CONTEXT: &str = "context";
}

pub mod k8s_pod {
    pub const LABEL: &str = "K8sPod";
    pub const NAME: &str = "name";
    pub const NAMESPACE: &str = "namespace";
    pub const CLUSTER: &str = "cluster";
    pub const STATUS: &str = "status";
    pub const NODE_NAME: &str = "node_name";
    pub const RESTART_COUNT: &str = "restart_count";
    pub const READY: &str = "ready";
    pub const IMAGE: &str = "image";
    pub const OWNER_KIND: &str = "owner_kind";
    pub const OWNER_NAME: &str = "owner_name";
    pub const RESOURCE_VERSION: &str = "resource_version";
}

pub mod k8s_deployment {
    pub const LABEL: &str = "K8sDeployment";
    pub const NAME: &str = "name";
    pub const NAMESPACE: &str = "namespace";
    pub const CLUSTER: &str = "cluster";
    pub const KIND: &str = "kind";
    pub const REPLICAS_DESIRED: &str = "replicas_desired";
    pub const REPLICAS_READY: &str = "replicas_ready";
    pub const REPLICAS_AVAILABLE: &str = "replicas_available";
    pub const IMAGE: &str = "image";
}

pub mod log_event {
    pub const LABEL: &str = "LogEvent";
    pub const CLUSTER: &str = "cluster";
    pub const NAMESPACE: &str = "namespace";
    pub const POD: &str = "pod";
    pub const TEMPLATE_HASH: &str = "template_hash";
    pub const SEVERITY: &str = "severity";
    pub const TEMPLATE_TEXT: &str = "template_text";
    pub const FIRST_SEEN: &str = "first_seen";
    pub const LAST_SEEN: &str = "last_seen";
    pub const COUNT: &str = "count";
    pub const EXAMPLE_LINES: &str = "example_lines";
}

// ── Edges ──

pub mod edges {
    pub const HAS_DISK: &str = "HAS_DISK";
    pub const HAS_IFACE: &str = "HAS_IFACE";
    pub const HAS_UNIT: &str = "HAS_UNIT";
    pub const RUNS: &str = "RUNS";
    pub const RUNS_CONTAINER: &str = "RUNS_CONTAINER";
    pub const EMITTED: &str = "EMITTED";
    pub const CONTAINS: &str = "CONTAINS";
    pub const READS: &str = "READS";
    pub const MENTIONS: &str = "MENTIONS";
    pub const CAUSED_BY: &str = "CAUSED_BY";
    pub const CALLS: &str = "CALLS";
    pub const IMPORTS: &str = "IMPORTS";
    pub const DECORATED_BY: &str = "DECORATED_BY";
    pub const CHANGES: &str = "CHANGES";
}

// ── Platform detection ──

/// Detect the runtime environment for adaptive host ingestion.
#[derive(Debug, Clone, PartialEq)]
pub enum Platform {
    /// Full Linux with /proc, systemd, etc.
    Linux,
    /// NixOS — Linux but with nix-store paths and different binary locations
    NixOS,
    /// Inside a Docker/OCI container — limited /proc, no systemd
    Container,
    /// Inside a K8s pod — container + K8s service account
    KubernetesPod,
    /// FreeBSD/OpenBSD — different /proc, no systemd, uses sysctl
    Bsd,
    /// macOS — sysctl, launchctl, different everything
    MacOS,
    /// Unknown — graceful degradation
    Unknown,
}

impl Platform {
    pub fn detect() -> Self {
        // Check container first (most restrictive environment)
        if std::path::Path::new("/.dockerenv").exists()
            || std::fs::read_to_string("/proc/1/cgroup")
                .map(|s| s.contains("docker") || s.contains("containerd") || s.contains("kubepods"))
                .unwrap_or(false)
        {
            if std::path::Path::new("/var/run/secrets/kubernetes.io").exists() {
                return Platform::KubernetesPod;
            }
            return Platform::Container;
        }

        // Check OS
        if std::path::Path::new("/etc/NIXOS").exists()
            || std::path::Path::new("/run/current-system/sw").exists()
        {
            return Platform::NixOS;
        }

        if cfg!(target_os = "linux") && std::path::Path::new("/proc").exists() {
            return Platform::Linux;
        }

        if cfg!(target_os = "freebsd") || cfg!(target_os = "openbsd") || cfg!(target_os = "netbsd") {
            return Platform::Bsd;
        }

        if cfg!(target_os = "macos") {
            return Platform::MacOS;
        }

        Platform::Unknown
    }

    /// Does this platform have /proc/meminfo, /proc/stat, etc.?
    pub fn has_procfs(&self) -> bool {
        matches!(self, Platform::Linux | Platform::NixOS | Platform::Container | Platform::KubernetesPod)
    }

    /// Does this platform have systemd (systemctl)?
    pub fn has_systemd(&self) -> bool {
        matches!(self, Platform::Linux | Platform::NixOS)
    }

    /// Does this platform support dmesg?
    pub fn has_dmesg(&self) -> bool {
        matches!(self, Platform::Linux | Platform::NixOS | Platform::Bsd)
    }

    /// Does this platform have journalctl?
    pub fn has_journald(&self) -> bool {
        matches!(self, Platform::Linux | Platform::NixOS)
    }

    /// Does this platform have `ip` command for network info?
    pub fn has_ip_command(&self) -> bool {
        matches!(self, Platform::Linux | Platform::NixOS | Platform::Container | Platform::KubernetesPod)
    }

    /// Can we read full /proc/{pid}/stat for all processes?
    /// Containers often have a restricted /proc view.
    pub fn has_full_procfs(&self) -> bool {
        matches!(self, Platform::Linux | Platform::NixOS)
    }

    /// BSD-style: use sysctl instead of /proc
    pub fn use_sysctl(&self) -> bool {
        matches!(self, Platform::Bsd | Platform::MacOS)
    }

    /// macOS: use launchctl instead of systemctl
    pub fn use_launchctl(&self) -> bool {
        matches!(self, Platform::MacOS)
    }
}
