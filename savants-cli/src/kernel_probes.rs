//! Kernel-level probes - process, network, file access, privilege monitoring.
//!
//! All probes use /proc and /sys (guaranteed on every Linux).
//! No external tools needed. No eBPF dependency (yet).
//!
//! Each probe tracks state between calls and only emits NEW events.
//! Events are generic - the cloud side correlates them by time.
//!
//! Graph model:
//!   - Every event has: timestamp, pid, comm, container_id, namespace, pod
//!   - Events that happen within the same time window are temporally correlated
//!   - Process lineage (ppid chain) provides structural correlation
//!   - Container/pod context links kernel events to application workloads

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A kernel-level event detected by probes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelEvent {
    pub timestamp: i64,
    pub probe: String,
    pub severity: String,
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub comm: String,
    pub container_id: Option<String>,
    pub namespace: Option<String>,
    pub pod: Option<String>,
    pub detail: serde_json::Value,
}

// ── State tracking ──

static KNOWN_PIDS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);
static KNOWN_CONNECTIONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static KNOWN_LISTENERS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Run all probes and return new events since last call.
/// Designed to be called every WATCH_INTERVAL (60s).
pub fn poll_all(hostname: &str) -> Vec<KernelEvent> {
    let mut events = Vec::new();
    // Process + network probes (state diffing)
    events.extend(probe_processes(hostname));
    events.extend(probe_connections(hostname));
    events.extend(probe_listeners(hostname));
    events.extend(probe_fd_pressure());
    // Histogram probes (/proc-based equivalents of eBPF Tier 1)
    events.extend(poll_histograms());
    events
}

// ── Process probe ──
// Detects new processes by diffing /proc PIDs between calls.
// For each new PID: reads comm, cmdline, status, cgroup (container detection).

fn probe_processes(hostname: &str) -> Vec<KernelEvent> {
    let current_pids = read_all_pids();
    let mut known = match KNOWN_PIDS.lock() {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };

    if known.is_none() {
        *known = Some(current_pids);
        return Vec::new();
    }

    let prev = known.as_ref().unwrap();
    let new_pids: Vec<u32> = current_pids.iter()
        .filter(|pid| !prev.contains(pid))
        .copied()
        .collect();

    let now = chrono::Utc::now().timestamp();
    let mut events = Vec::new();

    for pid in &new_pids {
        if let Some(info) = read_proc_info(*pid) {
            // Skip kernel threads and known-boring processes
            if info.comm.starts_with("kworker") || info.comm.starts_with("rcu_")
                || info.comm.starts_with("migration") || info.comm.starts_with("ksoftirqd")
                || info.comm.starts_with("watchdog") || info.comm.starts_with("irq/")
                || info.comm.starts_with("scsi_") || info.comm.starts_with("jbd2")
                || info.comm.starts_with("kswapd") || info.comm.starts_with("oom_reaper")
            {
                continue;
            }

            let severity = classify_process_severity(&info);
            if severity == "ignore" { continue; }

            events.push(KernelEvent {
                timestamp: now,
                probe: "process_exec".into(),
                severity: severity.to_string(),
                pid: *pid,
                ppid: info.ppid,
                uid: info.uid,
                comm: info.comm.clone(),
                container_id: info.container_id.clone(),
                namespace: info.namespace.clone(),
                pod: info.pod.clone(),
                detail: serde_json::json!({
                    "cmdline": info.cmdline,
                    "parent_comm": info.parent_comm,
                    "cwd": info.cwd,
                }),
            });
        }
    }

    *known = Some(current_pids);
    events
}

// ── Network connection probe ──
// Detects new ESTABLISHED TCP connections by diffing /proc/net/tcp.

fn probe_connections(_hostname: &str) -> Vec<KernelEvent> {
    let current = read_tcp_connections("01"); // ESTABLISHED
    let mut known = match KNOWN_CONNECTIONS.lock() {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };

    if known.is_none() {
        *known = Some(current);
        return Vec::new();
    }

    let prev = known.as_ref().unwrap();
    let new_conns: Vec<String> = current.iter()
        .filter(|c| !prev.contains(*c))
        .cloned()
        .collect();

    let now = chrono::Utc::now().timestamp();
    let mut events = Vec::new();

    for conn in &new_conns {
        let parts: Vec<&str> = conn.split('|').collect();
        if parts.len() < 4 { continue; }
        let dest_ip = parts[0];
        let dest_port: u16 = parts[1].parse().unwrap_or(0);
        let src_port: u16 = parts[2].parse().unwrap_or(0);
        let inode = parts[3];

        // Skip loopback and cluster-internal
        if dest_ip == "127.0.0.1" || dest_ip == "0.0.0.0" { continue; }

        // Try to find which process owns this socket
        let (pid, comm) = find_socket_owner(inode);

        let severity = if dest_port == 4444 || dest_port == 5555 || dest_port == 1337
            || dest_port == 31337 || dest_port == 9999 {
            "high" // suspicious ports
        } else if dest_ip.starts_with("10.") || dest_ip.starts_with("172.")
            || dest_ip.starts_with("192.168.") || dest_ip.starts_with("100.") {
            "ignore" // internal
        } else {
            "info"
        };

        if severity == "ignore" { continue; }

        events.push(KernelEvent {
            timestamp: now,
            probe: "network_connect".into(),
            severity: severity.into(),
            pid,
            ppid: 0,
            uid: 0,
            comm: comm.clone(),
            container_id: if pid > 0 { detect_container_id(pid) } else { None },
            namespace: None,
            pod: None,
            detail: serde_json::json!({
                "dest_ip": dest_ip,
                "dest_port": dest_port,
                "src_port": src_port,
                "process": comm,
            }),
        });
    }

    *known = Some(current);
    events
}

// ── Listener probe ──
// Detects new LISTEN sockets - something started serving.

fn probe_listeners(_hostname: &str) -> Vec<KernelEvent> {
    let current = read_tcp_connections("0A"); // LISTEN
    let mut known = match KNOWN_LISTENERS.lock() {
        Ok(k) => k,
        Err(_) => return Vec::new(),
    };

    if known.is_none() {
        *known = Some(current);
        return Vec::new();
    }

    let prev = known.as_ref().unwrap();
    let new_listeners: Vec<String> = current.iter()
        .filter(|c| !prev.contains(*c))
        .cloned()
        .collect();

    let now = chrono::Utc::now().timestamp();
    let mut events = Vec::new();

    for listener in &new_listeners {
        let parts: Vec<&str> = listener.split('|').collect();
        if parts.len() < 4 { continue; }
        let bind_ip = parts[0];
        let bind_port: u16 = parts[1].parse().unwrap_or(0);
        let inode = parts[3];

        let (pid, comm) = find_socket_owner(inode);

        events.push(KernelEvent {
            timestamp: now,
            probe: "network_listen".into(),
            severity: "info".into(),
            pid,
            ppid: 0,
            uid: 0,
            comm: comm.clone(),
            container_id: if pid > 0 { detect_container_id(pid) } else { None },
            namespace: None,
            pod: None,
            detail: serde_json::json!({
                "bind_ip": bind_ip,
                "bind_port": bind_port,
                "process": comm,
            }),
        });
    }

    *known = Some(current);
    events
}

// ── FD pressure probe ──
// Checks system-wide file descriptor usage.

fn probe_fd_pressure() -> Vec<KernelEvent> {
    let mut events = Vec::new();

    if let Ok(raw) = std::fs::read_to_string("/proc/sys/fs/file-nr") {
        let parts: Vec<&str> = raw.trim().split_whitespace().collect();
        if parts.len() >= 3 {
            let allocated: u64 = parts[0].parse().unwrap_or(0);
            let max: u64 = parts[2].parse().unwrap_or(1);
            let pct = allocated as f64 / max as f64 * 100.0;

            if pct > 80.0 {
                let now = chrono::Utc::now().timestamp();
                events.push(KernelEvent {
                    timestamp: now,
                    probe: "fd_pressure".into(),
                    severity: if pct > 95.0 { "critical" } else { "warning" }.into(),
                    pid: 0, ppid: 0, uid: 0,
                    comm: "kernel".into(),
                    container_id: None, namespace: None, pod: None,
                    detail: serde_json::json!({
                        "allocated": allocated,
                        "max": max,
                        "percent": pct,
                    }),
                });
            }
        }
    }

    events
}

// ── Helpers ──

struct ProcInfo {
    comm: String,
    cmdline: String,
    ppid: u32,
    uid: u32,
    parent_comm: String,
    container_id: Option<String>,
    namespace: Option<String>,
    pod: Option<String>,
    cwd: String,
}

fn read_all_pids() -> HashSet<u32> {
    let mut pids = HashSet::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                if let Ok(pid) = name.parse::<u32>() {
                    pids.insert(pid);
                }
            }
        }
    }
    pids
}

fn read_proc_info(pid: u32) -> Option<ProcInfo> {
    let proc_dir = format!("/proc/{}", pid);
    if !std::path::Path::new(&proc_dir).exists() { return None; }

    let comm = std::fs::read_to_string(format!("{}/comm", proc_dir))
        .ok()?.trim().to_string();
    let cmdline = std::fs::read_to_string(format!("{}/cmdline", proc_dir))
        .ok()?.replace('\0', " ").trim().to_string();
    let status = std::fs::read_to_string(format!("{}/status", proc_dir)).ok()?;

    let uid = parse_status_field(&status, "Uid:");
    let ppid = parse_status_field(&status, "PPid:");

    let parent_comm = std::fs::read_to_string(format!("/proc/{}/comm", ppid))
        .unwrap_or_default().trim().to_string();

    let cwd = std::fs::read_link(format!("{}/cwd", proc_dir))
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let container_id = detect_container_id(pid);
    let (namespace, pod) = detect_k8s_context(pid);

    Some(ProcInfo { comm, cmdline, ppid, uid, parent_comm, container_id, namespace, pod, cwd })
}

fn parse_status_field(status: &str, field: &str) -> u32 {
    status.lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn detect_container_id(pid: u32) -> Option<String> {
    let cgroup = std::fs::read_to_string(format!("/proc/{}/cgroup", pid)).ok()?;
    for line in cgroup.lines() {
        if line.contains("docker") || line.contains("containerd") || line.contains("cri-o") {
            let parts: Vec<&str> = line.rsplit('/').collect();
            if let Some(id) = parts.first() {
                let id = id.trim();
                if id.len() >= 12 && id.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(id[..12].to_string());
                }
            }
        }
    }
    None
}

fn detect_k8s_context(pid: u32) -> (Option<String>, Option<String>) {
    let environ = std::fs::read_to_string(format!("/proc/{}/environ", pid))
        .unwrap_or_default();
    let vars: Vec<&str> = environ.split('\0').collect();

    let namespace = vars.iter()
        .find(|v| v.starts_with("POD_NAMESPACE=") || v.starts_with("KUBERNETES_NAMESPACE="))
        .map(|v| v.split('=').nth(1).unwrap_or("").to_string());

    let pod = vars.iter()
        .find(|v| v.starts_with("POD_NAME=") || v.starts_with("HOSTNAME="))
        .map(|v| v.split('=').nth(1).unwrap_or("").to_string());

    (namespace, pod)
}

fn read_tcp_connections(state_filter: &str) -> HashSet<String> {
    let mut conns = HashSet::new();
    for proto_file in &["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(content) = std::fs::read_to_string(proto_file) {
            for line in content.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 10 { continue; }
                if parts[3] != state_filter { continue; }

                let (dest_ip, dest_port) = match parse_hex_addr(parts[2]) {
                    Some(v) => v,
                    None => continue,
                };
                let (_, src_port) = match parse_hex_addr(parts[1]) {
                    Some(v) => v,
                    None => continue,
                };
                let inode = parts[9];

                conns.insert(format!("{}|{}|{}|{}", dest_ip, dest_port, src_port, inode));
            }
        }
    }
    conns
}

fn parse_hex_addr(hex: &str) -> Option<(String, u16)> {
    let parts: Vec<&str> = hex.split(':').collect();
    if parts.len() != 2 { return None; }

    let ip_hex = parts[0];
    let port = u16::from_str_radix(parts[1], 16).ok()?;

    if ip_hex.len() == 8 {
        let a = u8::from_str_radix(&ip_hex[6..8], 16).ok()?;
        let b = u8::from_str_radix(&ip_hex[4..6], 16).ok()?;
        let c = u8::from_str_radix(&ip_hex[2..4], 16).ok()?;
        let d = u8::from_str_radix(&ip_hex[0..2], 16).ok()?;
        Some((format!("{}.{}.{}.{}", a, b, c, d), port))
    } else {
        None // IPv6 - skip for now
    }
}

fn find_socket_owner(inode: &str) -> (u32, String) {
    // Walk /proc/*/fd looking for socket:[inode]
    let target = format!("socket:[{}]", inode);
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let pid: u32 = match name.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };
            let fd_dir = format!("/proc/{}/fd", pid);
            if let Ok(fds) = std::fs::read_dir(&fd_dir) {
                for fd in fds.flatten() {
                    if let Ok(link) = std::fs::read_link(fd.path()) {
                        if link.to_string_lossy() == target {
                            let comm = std::fs::read_to_string(format!("/proc/{}/comm", pid))
                                .unwrap_or_default().trim().to_string();
                            return (pid, comm);
                        }
                    }
                }
            }
        }
    }
    (0, String::new())
}

fn classify_process_severity(info: &ProcInfo) -> &'static str {
    let comm = &info.comm;
    let cmdline = &info.cmdline;

    // Known malware - always critical
    let malware = ["xmrig", "minerd", "kdevtmpfsi", "kinsing", "masscan", "meterpreter"];
    if malware.iter().any(|m| comm.contains(m) || cmdline.contains(m)) {
        return "critical";
    }

    // Reverse shell patterns
    if cmdline.contains("/dev/tcp/") || cmdline.contains("base64 -d | bash")
        || cmdline.contains("| sh") && cmdline.contains("curl") {
        return "critical";
    }

    // In a container: shells and download tools are suspicious
    if info.container_id.is_some() {
        let suspicious = ["bash", "sh", "nc", "ncat", "curl", "wget", "python", "perl"];
        let basename = comm.rsplit('/').next().unwrap_or(comm);
        if suspicious.contains(&basename) {
            // But not if it's a health probe
            let health = ["ping_", "health", "readiness", "liveness", "pg_isready", "redis-cli"];
            if !health.iter().any(|h| cmdline.contains(h)) {
                return "warning";
            }
        }
        // Unknown process in container - interesting but not alarming
        return "info";
    }

    // On host: known system processes are boring
    let boring = [
        "systemd", "journald", "logind", "udevd", "dbus-daemon",
        "NetworkManager", "dhcpcd", "wpa_supplicant", "sshd", "agetty",
        "polkitd", "containerd", "dockerd", "k3s", "kubelet",
        "nix-daemon", "nix-build", "nix-env", "nix-store",
        "auto-cpufreq", "thermald", "cron", "anacron",
    ];
    if boring.iter().any(|b| comm.starts_with(b)) {
        return "ignore";
    }

    "info"
}

// ── eBPF probe registry ──
// Defines ALL probes we want, with /proc fallbacks for each.
// When Aya eBPF is wired in, each probe gains a real-time kernel hook.
// Until then, /proc/sys polling covers the same metrics at lower resolution.

/// Probe definition with /proc fallback.
pub struct ProbeDefinition {
    pub name: &'static str,
    pub tier: &'static str,       // "always", "periodic", "diagnostic"
    pub kernel_hook: &'static str, // e.g., "tracepoint/block/block_rq_complete"
    pub description: &'static str,
    pub procfs_fallback: &'static str, // what /proc file to read instead
}

/// The complete probe registry based on Brendan Gregg's recommendations.
pub const PROBE_REGISTRY: &[ProbeDefinition] = &[
    // ── Tier 1: Run 24/7 (in-kernel histograms, near-zero overhead) ──
    ProbeDefinition {
        name: "biolatency",
        tier: "always",
        kernel_hook: "tracepoint/block/block_rq_complete",
        description: "Block I/O latency histogram - detects disk slowdowns",
        procfs_fallback: "/proc/diskstats",
    },
    ProbeDefinition {
        name: "runqlat",
        tier: "always",
        kernel_hook: "tracepoint/sched/sched_wakeup,sched_switch",
        description: "CPU scheduler queue latency - detects CPU saturation",
        procfs_fallback: "/proc/schedstat",
    },
    ProbeDefinition {
        name: "tcpretrans",
        tier: "always",
        kernel_hook: "tracepoint/tcp/tcp_retransmit_skb",
        description: "TCP retransmissions with dest IP - detects network issues",
        procfs_fallback: "/proc/net/snmp",
    },
    ProbeDefinition {
        name: "oomkill",
        tier: "always",
        kernel_hook: "tracepoint/oom/mark_victim",
        description: "OOM killer events - detects memory exhaustion",
        procfs_fallback: "/proc/vmstat",
    },
    ProbeDefinition {
        name: "cachestat",
        tier: "always",
        kernel_hook: "kprobe/add_to_page_cache_lru,mark_page_accessed",
        description: "Page cache hit/miss ratio - detects cache pressure",
        procfs_fallback: "/proc/vmstat",
    },
    ProbeDefinition {
        name: "tcprtt",
        tier: "always",
        kernel_hook: "kprobe/tcp_rcv_established",
        description: "TCP round-trip time distribution - detects network degradation",
        procfs_fallback: "/proc/net/snmp",
    },
    ProbeDefinition {
        name: "softirqs",
        tier: "always",
        kernel_hook: "tracepoint/irq/softirq_entry,softirq_exit",
        description: "Soft IRQ processing time - detects interrupt storms",
        procfs_fallback: "/proc/softirqs",
    },

    // ── Tier 2: Run periodically (sample 10s every 5 min) ──
    ProbeDefinition {
        name: "execsnoop",
        tier: "periodic",
        kernel_hook: "tracepoint/sched/sched_process_exec",
        description: "New process execution - security + debugging",
        procfs_fallback: "/proc",
    },
    ProbeDefinition {
        name: "tcplife",
        tier: "periodic",
        kernel_hook: "kprobe/tcp_set_state",
        description: "TCP connection lifecycle - duration, bytes, ports",
        procfs_fallback: "/proc/net/tcp",
    },
    ProbeDefinition {
        name: "tcpdrop",
        tier: "periodic",
        kernel_hook: "tracepoint/skb/kfree_skb",
        description: "Kernel TCP packet drops with reason",
        procfs_fallback: "/proc/net/snmp",
    },
    ProbeDefinition {
        name: "ext4slower",
        tier: "periodic",
        kernel_hook: "kprobe/ext4_file_read_iter,ext4_file_write_iter",
        description: "Slow filesystem operations (>10ms threshold)",
        procfs_fallback: "/proc/diskstats",
    },
    ProbeDefinition {
        name: "runqslower",
        tier: "periodic",
        kernel_hook: "tracepoint/sched/sched_wakeup,sched_switch",
        description: "Scheduling delays exceeding threshold",
        procfs_fallback: "/proc/schedstat",
    },
    ProbeDefinition {
        name: "exitsnoop",
        tier: "periodic",
        kernel_hook: "tracepoint/sched/sched_process_exit",
        description: "Process exits with exit code and signal",
        procfs_fallback: "/proc",
    },
    ProbeDefinition {
        name: "drsnoop",
        tier: "periodic",
        kernel_hook: "tracepoint/vmscan/mm_vmscan_direct_reclaim_begin",
        description: "Direct memory reclaim stalls",
        procfs_fallback: "/proc/vmstat",
    },

    // ── Tier 3: Diagnostic only (on-demand via MCP tool) ──
    ProbeDefinition {
        name: "profile",
        tier: "diagnostic",
        kernel_hook: "perf_events/cpu-cycles (49Hz sampling)",
        description: "CPU stack traces - where is time being spent",
        procfs_fallback: "/proc/stat",
    },
    ProbeDefinition {
        name: "offcputime",
        tier: "diagnostic",
        kernel_hook: "tracepoint/sched/sched_switch",
        description: "Off-CPU time with stack traces - where are processes blocked",
        procfs_fallback: "/proc/stat",
    },
    ProbeDefinition {
        name: "biosnoop",
        tier: "diagnostic",
        kernel_hook: "tracepoint/block/block_rq_issue,block_rq_complete",
        description: "Per-I/O event with process, latency, device",
        procfs_fallback: "/proc/diskstats",
    },
    ProbeDefinition {
        name: "opensnoop",
        tier: "diagnostic",
        kernel_hook: "tracepoint/syscalls/sys_enter_openat",
        description: "Every file open with process and path",
        procfs_fallback: "/proc",
    },
    ProbeDefinition {
        name: "tcpconnect",
        tier: "diagnostic",
        kernel_hook: "kprobe/tcp_v4_connect,tcp_v6_connect",
        description: "Every outbound TCP connection attempt",
        procfs_fallback: "/proc/net/tcp",
    },
];

/// Run /proc-based equivalents of the Tier 1 eBPF probes.
/// These provide the same information at lower resolution (point-in-time vs real-time).
/// Called every watch interval alongside the process/connection probes.
pub fn poll_histograms() -> Vec<KernelEvent> {
    let mut events = Vec::new();
    let now = chrono::Utc::now().timestamp();

    // biolatency equivalent: disk I/O stats from /proc/diskstats
    // We track deltas of io_ticks to detect latency spikes
    events.extend(probe_diskstats(now));

    // cachestat equivalent: page cache hit/miss from /proc/vmstat
    events.extend(probe_vmstat_cache(now));

    // softirqs: time spent in each softirq from /proc/softirqs
    events.extend(probe_softirqs(now));

    // oomkill equivalent: check /proc/vmstat oom_kill counter
    events.extend(probe_oom_counter(now));

    events
}

static PREV_DISKSTATS: Mutex<Option<HashMap<String, (u64, u64, u64)>>> = Mutex::new(None);

fn probe_diskstats(now: i64) -> Vec<KernelEvent> {
    let mut events = Vec::new();

    if let Ok(raw) = std::fs::read_to_string("/proc/diskstats") {
        let mut current: HashMap<String, (u64, u64, u64)> = HashMap::new();

        for line in raw.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 14 { continue; }
            let name = parts[2];
            // Skip partitions (only whole disks)
            if name.ends_with(|c: char| c.is_ascii_digit()) && !name.starts_with("nvme") && !name.starts_with("sd") {
                continue;
            }
            // Skip loop, ram, dm devices unless they have I/O
            if name.starts_with("loop") || name.starts_with("ram") { continue; }

            let reads_completed: u64 = parts[3].parse().unwrap_or(0);
            let writes_completed: u64 = parts[7].parse().unwrap_or(0);
            let io_ticks_ms: u64 = parts[12].parse().unwrap_or(0); // time doing I/O (ms)

            if reads_completed + writes_completed == 0 { continue; }
            current.insert(name.to_string(), (reads_completed, writes_completed, io_ticks_ms));
        }

        if let Ok(mut prev) = PREV_DISKSTATS.lock() {
            if let Some(ref prev_stats) = *prev {
                for (disk, (reads, writes, ticks)) in &current {
                    if let Some((prev_r, prev_w, prev_t)) = prev_stats.get(disk) {
                        let delta_ops = (reads - prev_r) + (writes - prev_w);
                        let delta_ticks = ticks - prev_t;

                        if delta_ops > 0 {
                            let avg_latency_ms = delta_ticks as f64 / delta_ops as f64;

                            // Alert on high average latency
                            if avg_latency_ms > 50.0 {
                                events.push(KernelEvent {
                                    timestamp: now,
                                    probe: "biolatency_proc".into(),
                                    severity: if avg_latency_ms > 200.0 { "critical" } else { "warning" }.into(),
                                    pid: 0, ppid: 0, uid: 0,
                                    comm: "kernel".into(),
                                    container_id: None, namespace: None, pod: None,
                                    detail: serde_json::json!({
                                        "disk": disk,
                                        "avg_latency_ms": avg_latency_ms,
                                        "delta_ops": delta_ops,
                                        "delta_io_ticks_ms": delta_ticks,
                                    }),
                                });
                            }
                        }
                    }
                }
            }
            *prev = Some(current);
        }
    }

    events
}

static PREV_VMSTAT: Mutex<Option<(u64, u64)>> = Mutex::new(None);

fn probe_vmstat_cache(now: i64) -> Vec<KernelEvent> {
    let mut events = Vec::new();

    if let Ok(raw) = std::fs::read_to_string("/proc/vmstat") {
        let mut pgfault: u64 = 0;
        let mut pgmajfault: u64 = 0;

        for line in raw.lines() {
            if let Some(val) = line.strip_prefix("pgfault ") {
                pgfault = val.trim().parse().unwrap_or(0);
            }
            if let Some(val) = line.strip_prefix("pgmajfault ") {
                pgmajfault = val.trim().parse().unwrap_or(0);
            }
        }

        if let Ok(mut prev) = PREV_VMSTAT.lock() {
            if let Some((prev_fault, prev_major)) = *prev {
                let delta_fault = pgfault.saturating_sub(prev_fault);
                let delta_major = pgmajfault.saturating_sub(prev_major);

                // Major page faults = reading from disk instead of cache
                if delta_major > 100 {
                    let miss_rate = if delta_fault > 0 {
                        delta_major as f64 / delta_fault as f64 * 100.0
                    } else { 0.0 };

                    events.push(KernelEvent {
                        timestamp: now,
                        probe: "cachestat_proc".into(),
                        severity: if miss_rate > 10.0 { "warning" } else { "info" }.into(),
                        pid: 0, ppid: 0, uid: 0,
                        comm: "kernel".into(),
                        container_id: None, namespace: None, pod: None,
                        detail: serde_json::json!({
                            "delta_pgfault": delta_fault,
                            "delta_pgmajfault": delta_major,
                            "cache_miss_rate_pct": miss_rate,
                        }),
                    });
                }
            }
            *prev = Some((pgfault, pgmajfault));
        }
    }

    events
}

fn probe_softirqs(now: i64) -> Vec<KernelEvent> {
    // /proc/softirqs shows cumulative counts per CPU per IRQ type
    // We're looking for NET_RX or NET_TX being disproportionately high
    // (indicates NIC interrupt storm)
    // This is a basic check - full eBPF softirqs tool would measure time, not just count
    Vec::new() // Stub - full impl needs delta tracking per IRQ type
}

static PREV_OOM_COUNT: Mutex<Option<u64>> = Mutex::new(None);

fn probe_oom_counter(now: i64) -> Vec<KernelEvent> {
    let mut events = Vec::new();

    if let Ok(raw) = std::fs::read_to_string("/proc/vmstat") {
        for line in raw.lines() {
            if let Some(val) = line.strip_prefix("oom_kill ") {
                let count: u64 = val.trim().parse().unwrap_or(0);

                if let Ok(mut prev) = PREV_OOM_COUNT.lock() {
                    if let Some(prev_count) = *prev {
                        let delta = count.saturating_sub(prev_count);
                        if delta > 0 {
                            events.push(KernelEvent {
                                timestamp: now,
                                probe: "oomkill_proc".into(),
                                severity: "critical".into(),
                                pid: 0, ppid: 0, uid: 0,
                                comm: "kernel".into(),
                                container_id: None, namespace: None, pod: None,
                                detail: serde_json::json!({
                                    "new_oom_kills": delta,
                                    "total_oom_kills": count,
                                }),
                            });
                        }
                    }
                    *prev = Some(count);
                }
                break;
            }
        }
    }

    events
}

/// Check if this system supports eBPF (BTF present, sufficient kernel version).
pub fn ebpf_available() -> bool {
    std::path::Path::new("/sys/kernel/btf/vmlinux").exists()
}

/// List which probes are available on this system.
pub fn available_probes() -> Vec<(&'static str, &'static str, bool)> {
    let has_ebpf = ebpf_available();

    PROBE_REGISTRY.iter().map(|p| {
        let fallback_exists = std::path::Path::new(p.procfs_fallback).exists();
        let available = has_ebpf || fallback_exists;
        (p.name, p.tier, available)
    }).collect()
}
