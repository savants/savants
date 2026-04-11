//! Network connection probe — detects outbound connections.
//!
//! In eBPF mode: attaches to tracepoint/syscalls/sys_enter_connect
//! In fallback mode: polls /proc/net/tcp for new connections

use crate::events::{KernelSecurityEvent, EventDetail, Severity};
use chrono::Utc;
use std::collections::HashSet;
use std::sync::Mutex;

static KNOWN_CONNECTIONS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Poll /proc/net/tcp for new outbound connections.
pub fn poll_proc(hostname: &str) -> Option<Vec<KernelSecurityEvent>> {
    let current = read_established_connections();
    let mut known = KNOWN_CONNECTIONS.lock().ok()?;

    if known.is_none() {
        *known = Some(current);
        return None;
    }

    let prev = known.as_ref().unwrap();
    let new_conns: Vec<String> = current.iter()
        .filter(|c| !prev.contains(*c))
        .cloned()
        .collect();

    let mut events = Vec::new();
    for conn in &new_conns {
        if let Some(event) = parse_connection(conn, hostname) {
            events.push(event);
        }
    }

    *known = Some(current);

    if events.is_empty() { None } else { Some(events) }
}

fn read_established_connections() -> HashSet<String> {
    let mut conns = HashSet::new();

    for proto_file in &["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(content) = std::fs::read_to_string(proto_file) {
            for line in content.lines().skip(1) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 4 { continue; }

                // State 01 = ESTABLISHED
                if parts[3] == "01" {
                    // local:port → remote:port
                    conns.insert(format!("{}→{}", parts[1], parts[2]));
                }
            }
        }
    }
    conns
}

fn parse_connection(conn: &str, hostname: &str) -> Option<KernelSecurityEvent> {
    let parts: Vec<&str> = conn.split('→').collect();
    if parts.len() != 2 { return None; }

    let (dest_ip, dest_port) = parse_hex_addr(parts[1])?;

    // Skip internal cluster traffic and loopback
    if dest_ip == "127.0.0.1" || dest_ip == "0.0.0.0" {
        return None;
    }

    Some(KernelSecurityEvent {
        timestamp: Utc::now(),
        hostname: hostname.to_string(),
        probe: "connect".to_string(),
        severity: Severity::Info,
        pid: 0,  // /proc/net/tcp doesn't give us the PID directly
        ppid: 0,
        uid: 0,
        comm: String::new(),
        container_id: None,
        namespace: None,
        pod: None,
        detail: EventDetail::Connect {
            dest_ip,
            dest_port,
            protocol: "tcp".to_string(),
        },
    })
}

fn parse_hex_addr(hex: &str) -> Option<(String, u16)> {
    let parts: Vec<&str> = hex.split(':').collect();
    if parts.len() != 2 { return None; }

    let ip_hex = parts[0];
    let port = u16::from_str_radix(parts[1], 16).ok()?;

    if ip_hex.len() == 8 {
        // IPv4: stored as little-endian hex
        let a = u8::from_str_radix(&ip_hex[6..8], 16).ok()?;
        let b = u8::from_str_radix(&ip_hex[4..6], 16).ok()?;
        let c = u8::from_str_radix(&ip_hex[2..4], 16).ok()?;
        let d = u8::from_str_radix(&ip_hex[0..2], 16).ok()?;
        Some((format!("{}.{}.{}.{}", a, b, c, d), port))
    } else {
        // IPv6 or other — skip for now
        None
    }
}
