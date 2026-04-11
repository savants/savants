//! Process execution probe — detects new processes.
//!
//! In eBPF mode: attaches to tracepoint/syscalls/sys_enter_execve
//! In fallback mode: polls /proc for new PIDs and checks their cmdline

use crate::events::{KernelSecurityEvent, EventDetail, Severity};
use chrono::Utc;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

static KNOWN_PIDS: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// Poll /proc for new processes (fallback when eBPF isn't available).
pub fn poll_proc(hostname: &str) -> Option<Vec<KernelSecurityEvent>> {
    let mut known = KNOWN_PIDS.lock().ok()?;
    let current_pids = read_all_pids();

    if known.is_none() {
        // First run — initialize the set, don't emit events for existing processes
        *known = Some(current_pids);
        return None;
    }

    let prev = known.as_ref().unwrap();
    let new_pids: Vec<u32> = current_pids.iter()
        .filter(|pid| !prev.contains(pid))
        .copied()
        .collect();

    let mut events = Vec::new();
    for pid in &new_pids {
        if let Some(event) = read_process_event(*pid, hostname) {
            // Only emit events for interesting processes
            if is_interesting(&event) {
                events.push(event);
            }
        }
    }

    *known = Some(current_pids);

    if events.is_empty() { None } else { Some(events) }
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

fn read_process_event(pid: u32, hostname: &str) -> Option<KernelSecurityEvent> {
    let proc_dir = format!("/proc/{}", pid);
    let path = Path::new(&proc_dir);
    if !path.exists() { return None; }

    // Read comm (process name)
    let comm = std::fs::read_to_string(format!("{}/comm", proc_dir))
        .ok()?.trim().to_string();

    // Read cmdline
    let cmdline = std::fs::read_to_string(format!("{}/cmdline", proc_dir))
        .ok()?
        .replace('\0', " ")
        .trim()
        .to_string();

    // Read status for UIDs and PPIDs
    let status = std::fs::read_to_string(format!("{}/status", proc_dir)).ok()?;
    let uid = parse_status_field(&status, "Uid:");
    let ppid = parse_status_field(&status, "PPid:");

    // Read parent comm
    let parent_comm = std::fs::read_to_string(format!("/proc/{}/comm", ppid))
        .unwrap_or_default().trim().to_string();

    // Check if in a container (cgroup contains "docker" or "containerd")
    let container_id = detect_container_id(pid);

    // Check if in a K8s pod
    let (namespace, pod) = detect_k8s_context(pid);

    let argv: Vec<String> = cmdline.split_whitespace()
        .take(5)
        .map(|s| s.to_string())
        .collect();

    let filename = argv.first().cloned().unwrap_or_else(|| comm.clone());

    Some(KernelSecurityEvent {
        timestamp: Utc::now(),
        hostname: hostname.to_string(),
        probe: "execve".to_string(),
        severity: Severity::Info, // classified later
        pid,
        ppid,
        uid,
        comm,
        container_id,
        namespace,
        pod,
        detail: EventDetail::ProcessExec {
            filename,
            argv,
            parent_comm,
        },
    })
}

fn is_interesting(event: &KernelSecurityEvent) -> bool {
    // Skip system noise — only report processes that are potentially suspicious
    let boring = ["kworker", "migration", "rcu_", "ksoftirqd", "watchdog",
                  "irq/", "scsi_", "loop", "jbd2", "ext4"];
    if boring.iter().any(|b| event.comm.starts_with(b)) {
        return false;
    }

    // In containers, almost everything is interesting
    if event.container_id.is_some() {
        return true;
    }

    // On the host, only report unusual processes
    match &event.detail {
        EventDetail::ProcessExec { filename, .. } => {
            let basename = filename.rsplit('/').next().unwrap_or(filename);
            let suspicious = ["bash", "sh", "nc", "ncat", "curl", "wget",
                            "python", "perl", "ruby", "xmrig", "nmap"];
            // Only interesting if it's a suspicious binary OR parent is unusual
            suspicious.iter().any(|s| basename == *s) || event.ppid == 1
        }
        _ => false,
    }
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
            // Extract container ID from the cgroup path
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
    // K8s pods have specific cgroup patterns and environment variables
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
