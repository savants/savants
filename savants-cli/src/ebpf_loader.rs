//! eBPF probe loader - 9 Brendan Gregg essential probes via libbpf-rs.
//!
//! Tier 1 (24/7):
//!   tcp_retransmit - per-dest retransmit counts (link vs server diagnosis)
//!   biolatency    - block I/O latency histogram (disk health)
//!   runqlat       - CPU scheduler queue latency (NO /proc fallback)
//!   tcpdrop       - kernel packet drops by reason code
//!   oomkill       - OOM killer with victim details
//!   execsnoop     - real-time process execution (catches short-lived procs)
//!
//! Tier 2 (differentiators):
//!   tcplife       - TCP connection lifecycle (duration, port, short-lived count)
//!   tcpconnlat    - TCP resets by destination (connection refused tracking)
//!   capable       - Linux capability checks (container security)

use std::net::Ipv4Addr;

// Embedded BPF bytecode - all compiled from C, zero runtime dependencies
const BPF_TCP_RETRANSMIT: &[u8] = include_bytes!("../ebpf/tcp_retransmit.bpf.o");
const BPF_BIOLATENCY: &[u8] = include_bytes!("../ebpf/biolatency.bpf.o");
const BPF_RUNQLAT: &[u8] = include_bytes!("../ebpf/runqlat.bpf.o");
const BPF_TCPDROP: &[u8] = include_bytes!("../ebpf/tcpdrop.bpf.o");
const BPF_OOMKILL: &[u8] = include_bytes!("../ebpf/oomkill.bpf.o");
const BPF_EXECSNOOP: &[u8] = include_bytes!("../ebpf/execsnoop.bpf.o");
const BPF_TCPLIFE: &[u8] = include_bytes!("../ebpf/tcplife.bpf.o");
const BPF_TCPCONNLAT: &[u8] = include_bytes!("../ebpf/tcpconnlat.bpf.o");
const BPF_CAPABLE: &[u8] = include_bytes!("../ebpf/capable.bpf.o");

// Drop reason names (from kernel skb_drop_reason enum)
const DROP_REASONS: &[&str] = &[
    "UNUSED", "UNUSED", "NOT_SPECIFIED", "NO_SOCKET", "SOCKET_CLOSE",
    "SOCKET_FILTER", "SOCKET_RCVBUFF", "UNIX_DISCONNECT", "UNIX_SKIP_OOB",
    "PKT_TOO_SMALL", "TCP_CSUM", "UDP_CSUM", "NETFILTER_DROP", "OTHERHOST",
    "IP_CSUM", "IP_INHDR", "IP_RPFILTER", "UNICAST_IN_L2_MULTICAST",
    "XFRM_POLICY", "IP_NOPROTO", "PROTO_MEM", "TCP_AUTH_HDR",
    "TCP_MD5NOTFOUND", "TCP_MD5UNEXPECTED", "TCP_MD5FAILURE",
    "TCP_AONOTFOUND", "TCP_AOUNEXPECTED", "TCP_AOKEYNOTFOUND", "TCP_AOFAILURE",
    "SOCKET_BACKLOG", "TCP_FLAGS", "TCP_ABORT_ON_DATA", "TCP_ZEROWINDOW",
    "TCP_OLD_DATA", "TCP_OVERWINDOW", "TCP_OFOMERGE", "TCP_RFC7323_PAWS",
    "TCP_RFC7323_PAWS_ACK", "TCP_RFC7323_TW_PAWS", "TCP_RFC7323_TSECR",
    "TCP_LISTEN_OVERFLOW", "TCP_OLD_SEQUENCE", "TCP_INVALID_SEQUENCE",
    "TCP_INVALID_END_SEQUENCE", "TCP_INVALID_ACK_SEQUENCE", "TCP_RESET",
    "TCP_INVALID_SYN", "TCP_CLOSE", "TCP_FASTOPEN", "TCP_OLD_ACK",
    "TCP_TOO_OLD_ACK", "TCP_ACK_UNSENT_DATA", "TCP_OFO_QUEUE_PRUNE",
    "TCP_OFO_DROP", "IP_OUTNOROUTES", "BPF_CGROUP_EGRESS", "IPV6DISABLED",
    "NEIGH_CREATEFAIL", "NEIGH_FAILED", "NEIGH_QUEUEFULL", "NEIGH_DEAD",
];

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProbeSnapshot {
    pub retransmit: Option<RetransmitData>,
    pub biolatency: Option<HistogramData>,
    pub runqlat: Option<HistogramData>,
    pub tcpdrop: Option<DropData>,
    pub oomkill: Option<CounterData>,
    pub execsnoop: Option<CounterData>,
    pub tcplife: Option<TcpLifeData>,
    pub tcpconnlat: Option<RetransmitData>, // reuse: per-dest reset counts
    pub capable: Option<CounterData>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CounterData {
    pub total: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TcpLifeData {
    pub total_conns: u64,
    pub short_lived: u64,
    pub by_port: Vec<(u16, u64)>,
    pub duration_hist: Option<HistogramData>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RetransmitData {
    pub total: u64,
    pub by_dest: Vec<(String, u64)>,
    pub dest_count: usize,
    pub is_link_issue: bool,
    pub diagnosis: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HistogramData {
    pub buckets: Vec<(String, u64)>, // (range_label, count)
    pub total: u64,
    pub p50_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DropData {
    pub total: u64,
    pub by_reason: Vec<(String, u64)>,
    pub top_reason: String,
}

pub fn can_load() -> bool {
    std::path::Path::new("/sys/kernel/btf/vmlinux").exists()
        && (nix::unistd::geteuid().is_root() || has_cap_bpf())
}

fn has_cap_bpf() -> bool {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("CapEff:") {
                let hex = line.split_whitespace().nth(1).unwrap_or("0");
                if let Ok(caps) = u64::from_str_radix(hex.trim_start_matches("0x").trim(), 16) {
                    return caps & (1u64 << 39) != 0;
                }
            }
        }
    }
    false
}

#[cfg(feature = "ebpf")]
pub fn load_and_attach() -> Result<BpfHandle, String> {
    use libbpf_rs::ObjectBuilder;

    let mut probes_loaded = Vec::new();
    let mut objects = Vec::new();
    let mut links = Vec::new();

    // Load each probe - continue if one fails
    for (name, bytecode, tracepoints) in &[
        // Tier 1: 24/7 probes
        ("tcp_retransmit", BPF_TCP_RETRANSMIT, vec![("trace_retransmit", "tcp", "tcp_retransmit_skb")]),
        ("biolatency", BPF_BIOLATENCY, vec![("trace_rq_issue", "block", "block_rq_issue"), ("trace_rq_complete", "block", "block_rq_complete")]),
        ("runqlat", BPF_RUNQLAT, vec![("trace_wakeup", "sched", "sched_wakeup"), ("trace_switch", "sched", "sched_switch")]),
        ("tcpdrop", BPF_TCPDROP, vec![("trace_kfree_skb", "skb", "kfree_skb")]),
        ("oomkill", BPF_OOMKILL, vec![("trace_oom", "oom", "mark_victim")]),
        ("execsnoop", BPF_EXECSNOOP, vec![("trace_exec", "sched", "sched_process_exec")]),
        // Tier 2: differentiators
        ("tcplife", BPF_TCPLIFE, vec![("trace_destroy", "tcp", "tcp_destroy_sock"), ("trace_synack", "tcp", "tcp_retransmit_synack")]),
        ("tcpconnlat", BPF_TCPCONNLAT, vec![("trace_tcp_reset", "tcp", "tcp_receive_reset")]),
        ("capable", BPF_CAPABLE, vec![("trace_cap_syscall", "raw_syscalls", "sys_enter")]),
    ] {
        match load_one(name, bytecode, tracepoints) {
            Ok((obj, obj_links)) => {
                probes_loaded.push(name.to_string());
                objects.push(obj);
                links.extend(obj_links);
            }
            Err(e) => {
                eprintln!("  eBPF: {} failed: {}", name, e);
            }
        }
    }

    if probes_loaded.is_empty() {
        return Err("No probes loaded".into());
    }

    Ok(BpfHandle { objects, _links: links, probes: probes_loaded })
}

#[cfg(feature = "ebpf")]
fn load_one(name: &str, bytecode: &[u8], tracepoints: &[(&str, &str, &str)]) -> Result<(libbpf_rs::Object, Vec<libbpf_rs::Link>), String> {
    use libbpf_rs::ObjectBuilder;

    let mut builder = ObjectBuilder::default();
    let open_obj = builder.open_memory(bytecode)
        .map_err(|e| format!("open: {}", e))?;
    let mut obj = open_obj.load()
        .map_err(|e| format!("load: {}", e))?;

    let mut links = Vec::new();
    for (prog_name, category, event) in tracepoints {
        let mut found = false;
        for mut prog in obj.progs_mut() {
            if prog.name() == std::ffi::OsStr::new(prog_name) {
                let link = prog.attach_tracepoint(category, event)
                    .map_err(|e| format!("attach {}: {}", prog_name, e))?;
                links.push(link);
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!("program '{}' not found", prog_name));
        }
    }

    Ok((obj, links))
}

#[cfg(feature = "ebpf")]
pub struct BpfHandle {
    objects: Vec<libbpf_rs::Object>,
    _links: Vec<libbpf_rs::Link>,
    pub probes: Vec<String>,
}

#[cfg(feature = "ebpf")]
impl BpfHandle {
    pub fn read_snapshot(&self) -> ProbeSnapshot {
        ProbeSnapshot {
            retransmit: self.read_retransmit(),
            biolatency: self.read_histogram("io_hist", 1),   // obj index 1
            runqlat: self.read_histogram("runq_hist", 2),     // obj index 2
            tcpdrop: self.read_drops(),                       // obj index 3
            oomkill: self.read_counter("oom_total", 4),       // obj index 4
            execsnoop: self.read_counter("exec_total", 5),    // obj index 5
            tcplife: self.read_tcplife(),                     // obj index 6
            tcpconnlat: self.read_reset_map(),                // obj index 7
            capable: self.read_counter("cap_total", 8),       // obj index 8
        }
    }

    fn read_retransmit(&self) -> Option<RetransmitData> {
        use libbpf_rs::MapCore;
        let obj = self.objects.get(0)?; // tcp_retransmit is first
        let map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new("retransmits"))?;

        let mut by_dest = Vec::new();
        let mut total: u64 = 0;

        for key in map.keys() {
            if let Ok(Some(val)) = map.lookup(&key, libbpf_rs::MapFlags::ANY) {
                if key.len() == 4 && val.len() == 8 {
                    let ip_raw = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]);
                    let count = u64::from_ne_bytes(val[..8].try_into().unwrap_or([0;8]));
                    total += count;
                    by_dest.push((Ipv4Addr::from(ip_raw).to_string(), count));
                }
            }
        }

        by_dest.sort_by(|a, b| b.1.cmp(&a.1));
        let dest_count = by_dest.len();
        let is_link_issue = dest_count >= 3;
        let diagnosis = if is_link_issue {
            format!("LOCAL LINK: {} destinations with retransmits - ISP/WiFi issue", dest_count)
        } else if dest_count == 1 {
            format!("REMOTE: retransmits only to {}", by_dest[0].0)
        } else {
            "Healthy".into()
        };

        Some(RetransmitData { total, by_dest, dest_count, is_link_issue, diagnosis })
    }

    fn read_histogram(&self, map_name: &str, obj_idx: usize) -> Option<HistogramData> {
        use libbpf_rs::MapCore;
        let obj = self.objects.get(obj_idx + 1)?; // +1 because tcp_retransmit is index 0
        let map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new(map_name))?;

        let mut buckets = Vec::new();
        let mut total: u64 = 0;
        let mut cumulative: u64 = 0;
        let mut p50: u64 = 0;
        let mut p99: u64 = 0;
        let mut max_us: u64 = 0;

        for i in 0u32..32 {
            let key = i.to_ne_bytes();
            if let Ok(Some(val)) = map.lookup(&key, libbpf_rs::MapFlags::ANY) {
                let count = u64::from_ne_bytes(val[..8].try_into().unwrap_or([0;8]));
                if count > 0 {
                    let lo = 1u64 << i;
                    let hi = 1u64 << (i + 1);
                    let label = if i == 0 { "<1us".into() }
                        else if lo < 1000 { format!("{}-{}us", lo, hi) }
                        else if lo < 1_000_000 { format!("{}-{}ms", lo/1000, hi/1000) }
                        else { format!("{}-{}s", lo/1_000_000, hi/1_000_000) };
                    buckets.push((label, count));
                    total += count;
                    max_us = hi;
                }
            }
        }

        // Compute percentiles
        for (i, (_, count)) in buckets.iter().enumerate() {
            cumulative += count;
            if p50 == 0 && cumulative >= total / 2 {
                p50 = 1u64 << (i + 1);
            }
            if p99 == 0 && cumulative >= total * 99 / 100 {
                p99 = 1u64 << (i + 1);
            }
        }

        Some(HistogramData { buckets, total, p50_us: p50, p99_us: p99, max_us })
    }

    fn read_drops(&self) -> Option<DropData> {
        use libbpf_rs::MapCore;
        let obj = self.objects.get(3)?; // tcpdrop is index 3
        let map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new("drop_reasons"))?;

        let mut by_reason = Vec::new();
        let mut total: u64 = 0;

        for i in 2u32..128 {
            let key = i.to_ne_bytes();
            if let Ok(Some(val)) = map.lookup(&key, libbpf_rs::MapFlags::ANY) {
                let count = u64::from_ne_bytes(val[..8].try_into().unwrap_or([0;8]));
                if count > 0 {
                    let reason_name = DROP_REASONS.get(i as usize).unwrap_or(&"UNKNOWN");
                    by_reason.push((reason_name.to_string(), count));
                    total += count;
                }
            }
        }

        by_reason.sort_by(|a, b| b.1.cmp(&a.1));
        let top_reason = by_reason.first().map(|(r, _)| r.clone()).unwrap_or_default();

        Some(DropData { total, by_reason, top_reason })
    }

    fn read_counter(&self, map_name: &str, obj_idx: usize) -> Option<CounterData> {
        use libbpf_rs::MapCore;
        let obj = self.objects.get(obj_idx)?;
        let map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new(map_name))?;
        let key = 0u32.to_ne_bytes();
        let val = map.lookup(&key, libbpf_rs::MapFlags::ANY).ok()??;
        let total = u64::from_ne_bytes(val[..8].try_into().unwrap_or([0;8]));
        Some(CounterData { total })
    }

    fn read_tcplife(&self) -> Option<TcpLifeData> {
        use libbpf_rs::MapCore;
        let obj = self.objects.get(6)?; // tcplife is index 6

        // Total connections
        let total_map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new("conn_total"))?;
        let key = 0u32.to_ne_bytes();
        let total_conns = total_map.lookup(&key, libbpf_rs::MapFlags::ANY).ok()
            .flatten()
            .map(|v| u64::from_ne_bytes(v[..8].try_into().unwrap_or([0;8])))
            .unwrap_or(0);

        // Short-lived
        let short_map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new("short_conns"))?;
        let short_lived = short_map.lookup(&key, libbpf_rs::MapFlags::ANY).ok()
            .flatten()
            .map(|v| u64::from_ne_bytes(v[..8].try_into().unwrap_or([0;8])))
            .unwrap_or(0);

        // By port
        let port_map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new("conn_by_port"))?;
        let mut by_port = Vec::new();
        for k in port_map.keys() {
            if let Ok(Some(v)) = port_map.lookup(&k, libbpf_rs::MapFlags::ANY) {
                if k.len() == 2 && v.len() == 8 {
                    let port = u16::from_ne_bytes([k[0], k[1]]);
                    let count = u64::from_ne_bytes(v[..8].try_into().unwrap_or([0;8]));
                    by_port.push((port, count));
                }
            }
        }
        by_port.sort_by(|a, b| b.1.cmp(&a.1));

        Some(TcpLifeData { total_conns, short_lived, by_port, duration_hist: self.read_histogram("conn_duration", 6) })
    }

    fn read_reset_map(&self) -> Option<RetransmitData> {
        use libbpf_rs::MapCore;
        let obj = self.objects.get(7)?; // tcpconnlat is index 7
        let map = obj.maps().find(|m| m.name() == std::ffi::OsStr::new("reset_by_dest"))?;

        let mut by_dest = Vec::new();
        let mut total: u64 = 0;

        for key in map.keys() {
            if let Ok(Some(val)) = map.lookup(&key, libbpf_rs::MapFlags::ANY) {
                if key.len() == 4 && val.len() == 8 {
                    let ip_raw = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]);
                    let count = u64::from_ne_bytes(val[..8].try_into().unwrap_or([0;8]));
                    total += count;
                    by_dest.push((Ipv4Addr::from(ip_raw).to_string(), count));
                }
            }
        }

        by_dest.sort_by(|a, b| b.1.cmp(&a.1));
        let dest_count = by_dest.len();
        let is_link_issue = dest_count >= 3;
        let diagnosis = if total == 0 { "No resets".into() }
            else if is_link_issue { format!("RESETS to {} destinations", dest_count) }
            else { format!("Resets to {}", by_dest.first().map(|(ip,_)| ip.as_str()).unwrap_or("?")) };

        Some(RetransmitData { total, by_dest, dest_count, is_link_issue, diagnosis })
    }
}

#[cfg(not(feature = "ebpf"))]
pub fn load_and_attach() -> Result<(), String> {
    Err("eBPF not compiled in (--features ebpf)".into())
}

pub fn bytecode_size() -> usize {
    BPF_TCP_RETRANSMIT.len() + BPF_BIOLATENCY.len() + BPF_RUNQLAT.len() + BPF_TCPDROP.len()
    + BPF_OOMKILL.len() + BPF_EXECSNOOP.len() + BPF_TCPLIFE.len() + BPF_TCPCONNLAT.len() + BPF_CAPABLE.len()
}
