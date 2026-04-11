//! Kernel security events emitted by eBPF probes.

use chrono::{DateTime, Utc};
use colored::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSecurityEvent {
    pub timestamp: DateTime<Utc>,
    pub hostname: String,
    pub probe: String,           // "execve", "connect", "openat", "setuid"
    pub severity: Severity,
    pub pid: u32,
    pub ppid: u32,               // parent PID — traces the process lineage
    pub uid: u32,
    pub comm: String,            // process name (16 char max from kernel)
    pub container_id: Option<String>,  // cgroup-derived container ID
    pub namespace: Option<String>,     // K8s namespace if in a pod
    pub pod: Option<String>,           // K8s pod name if in a pod

    // Probe-specific data
    pub detail: EventDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventDetail {
    /// New process created (execve)
    ProcessExec {
        filename: String,        // binary path
        argv: Vec<String>,       // first 4 args
        parent_comm: String,     // parent process name
    },
    /// Outbound network connection (connect)
    Connect {
        dest_ip: String,
        dest_port: u16,
        protocol: String,        // "tcp", "udp"
    },
    /// File access on a sensitive path (openat)
    FileAccess {
        path: String,
        flags: String,           // "read", "write", "read+write"
    },
    /// Privilege escalation (setuid/setgid)
    PrivEscalation {
        old_uid: u32,
        new_uid: u32,
        old_gid: u32,
        new_gid: u32,
    },
    /// DNS query (detected via connect to port 53)
    DnsQuery {
        domain: String,
        query_type: String,      // "A", "AAAA", "TXT", "MX"
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, PartialOrd)]
pub enum Severity {
    Info,
    Warning,
    High,
    Critical,
}

impl KernelSecurityEvent {
    pub fn format(&self) -> String {
        let sev_icon = match self.severity {
            Severity::Critical => "🔴",
            Severity::High => "🟠",
            Severity::Warning => "🟡",
            Severity::Info => "🔵",
        };

        let container = self.container_id.as_deref()
            .map(|c| format!(" [{}]", &c[..12.min(c.len())]))
            .unwrap_or_default();

        let pod_info = match (&self.namespace, &self.pod) {
            (Some(ns), Some(pod)) => format!(" {}/{}", ns, pod),
            _ => String::new(),
        };

        let detail = match &self.detail {
            EventDetail::ProcessExec { filename, argv, parent_comm } => {
                let args = if argv.is_empty() {
                    String::new()
                } else {
                    format!(" {}", argv.join(" "))
                };
                format!("exec: {}{} (parent: {})", filename, args, parent_comm)
            }
            EventDetail::Connect { dest_ip, dest_port, protocol } => {
                format!("connect: {}://{}:{}", protocol, dest_ip, dest_port)
            }
            EventDetail::FileAccess { path, flags } => {
                format!("open: {} ({})", path, flags)
            }
            EventDetail::PrivEscalation { old_uid, new_uid, .. } => {
                format!("setuid: {} → {}", old_uid, new_uid)
            }
            EventDetail::DnsQuery { domain, query_type } => {
                format!("dns: {} {}", query_type, domain)
            }
        };

        format!(
            "{} [{}] pid={} uid={}{}{} {} {}",
            sev_icon,
            self.probe,
            self.pid,
            self.uid,
            container,
            pod_info,
            self.comm,
            detail,
        )
    }
}

/// Classify the severity of a kernel event based on heuristics.
pub fn classify_severity(event: &KernelSecurityEvent) -> Severity {
    match &event.detail {
        EventDetail::PrivEscalation { old_uid, new_uid, .. } => {
            if *new_uid == 0 && *old_uid != 0 {
                Severity::Critical // non-root → root
            } else {
                Severity::Warning
            }
        }
        EventDetail::ProcessExec { filename, .. } => {
            let suspicious = [
                "bash", "sh", "dash", "zsh",  // shells (inside containers = suspicious)
                "nc", "ncat", "netcat",        // reverse shell tools
                "curl", "wget",                // download tools (in containers)
                "python", "perl", "ruby",      // scripting (unexpected in production)
                "xmrig", "minerd",             // known cryptominers
                "kdevtmpfsi", "kinsing",       // known malware
                "nmap", "masscan",             // scanning tools
            ];
            let basename = filename.rsplit('/').next().unwrap_or(filename);
            if suspicious.iter().any(|s| basename == *s) {
                if event.container_id.is_some() {
                    Severity::High  // suspicious process inside container
                } else {
                    Severity::Warning  // on host, might be legitimate
                }
            } else {
                Severity::Info
            }
        }
        EventDetail::Connect { dest_port, dest_ip, .. } => {
            // Connections to unusual ports from containers
            let suspicious_ports = [4444, 5555, 6666, 7777, 8888, 9999,
                                    1337, 31337, 12345, 54321];
            if suspicious_ports.contains(dest_port) {
                Severity::High
            } else if *dest_port == 53 {
                Severity::Info  // DNS is normal
            } else if dest_ip.starts_with("10.") || dest_ip.starts_with("172.") || dest_ip.starts_with("192.168.") {
                Severity::Info  // internal traffic
            } else if event.container_id.is_some() {
                Severity::Warning  // external connection from container
            } else {
                Severity::Info
            }
        }
        EventDetail::FileAccess { path, flags } => {
            let sensitive_paths = [
                "/etc/shadow", "/etc/passwd", "/etc/sudoers",
                "/root/.ssh", "/home/", "/.ssh/",
                "/var/run/secrets/kubernetes.io",  // K8s service account tokens
                "/proc/self/environ",              // environment variables
                "/etc/kubernetes/",                // K8s configs
            ];
            if sensitive_paths.iter().any(|p| path.contains(p)) {
                if flags.contains("write") {
                    Severity::Critical
                } else {
                    Severity::High
                }
            } else {
                Severity::Info
            }
        }
        EventDetail::DnsQuery { domain, .. } => {
            // Very long DNS names or high entropy = possible exfiltration
            if domain.len() > 60 {
                Severity::Warning
            } else {
                Severity::Info
            }
        }
    }
}
