//! eBPF probe loader - tcp_retransmit tracepoint via libbpf-rs.

use std::net::Ipv4Addr;

const TCP_RETRANSMIT_BPF: &[u8] = include_bytes!("../ebpf/tcp_retransmit.bpf.o");

#[derive(Debug, Clone, serde::Serialize)]
pub struct RetransmitSnapshot {
    pub total: u64,
    pub by_dest: Vec<(String, u64)>,
    pub dest_count: usize,
    pub is_link_issue: bool,
    pub link_confidence: f64,
    pub diagnosis: String,
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

    let mut obj_builder = ObjectBuilder::default();
    let open_obj = obj_builder.open_memory(TCP_RETRANSMIT_BPF)
        .map_err(|e| format!("open: {}", e))?;

    let mut obj = open_obj.load()
        .map_err(|e| format!("load: {}", e))?;

    // Find the program by iterating (libbpf-rs 0.24 uses iterators, not name lookup)
    let link = {
        let mut found = None;
        for mut prog in obj.progs_mut() {
            if prog.name() == "trace_retransmit" {
                found = Some(prog.attach_tracepoint("tcp", "tcp_retransmit_skb")
                    .map_err(|e| format!("attach: {}", e))?);
                break;
            }
        }
        found.ok_or("Program 'trace_retransmit' not found in BPF object")?
    };

    Ok(BpfHandle { obj, _link: link })
}

#[cfg(feature = "ebpf")]
pub struct BpfHandle {
    obj: libbpf_rs::Object,
    _link: libbpf_rs::Link,
}

#[cfg(feature = "ebpf")]
impl BpfHandle {
    pub fn read_snapshot(&self) -> Result<RetransmitSnapshot, String> {
        use libbpf_rs::MapCore;
        use std::ffi::OsStr;

        let map = self.obj.maps()
            .find(|m| m.name() == OsStr::new("retransmits"))
            .ok_or("Map 'retransmits' not found")?;

        let mut by_dest = Vec::new();
        let mut total: u64 = 0;

        for key in map.keys() {
            if let Ok(Some(val)) = map.lookup(&key, libbpf_rs::MapFlags::ANY) {
                if key.len() == 4 && val.len() == 8 {
                    let ip_raw = u32::from_ne_bytes([key[0], key[1], key[2], key[3]]);
                    let count = u64::from_ne_bytes([val[0], val[1], val[2], val[3], val[4], val[5], val[6], val[7]]);
                    let ip = Ipv4Addr::from(ip_raw);
                    total += count;
                    by_dest.push((ip.to_string(), count));
                }
            }
        }

        by_dest.sort_by(|a, b| b.1.cmp(&a.1));
        let dest_count = by_dest.len();
        let is_link_issue = dest_count >= 3;
        let link_confidence = match dest_count {
            0 => 0.0, 1 => 0.1, 2 => 0.4, 3..=4 => 0.8, _ => 0.95,
        };
        let diagnosis = if is_link_issue {
            format!("LOCAL LINK: {} destinations with retransmits - ISP/WAN issue", dest_count)
        } else if dest_count == 1 {
            format!("REMOTE: retransmits only to {}", by_dest[0].0)
        } else {
            "Healthy".into()
        };

        Ok(RetransmitSnapshot { total, by_dest, dest_count, is_link_issue, link_confidence, diagnosis })
    }
}

#[cfg(not(feature = "ebpf"))]
pub fn load_and_attach() -> Result<(), String> {
    Err("eBPF not compiled in (--features ebpf)".into())
}

pub fn bytecode_size() -> usize { TCP_RETRANSMIT_BPF.len() }
