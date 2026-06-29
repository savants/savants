//! Host ingestor — reads local machine state from /proc, systemd, dmesg,
//! Docker and writes it to Savants memory. Rust port of the Python
//! `savants.host.ingestor` module.

pub mod template;

use crate::graph::{GraphClient, ParamValue};
use colored::*;
use nix::sys::statvfs::statvfs;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use template::extract_template;

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub struct HostIngestStats {
    pub hostname: String,
    pub elapsed_seconds: f64,
    pub disks: usize,
    pub interfaces: usize,
    pub processes: usize,
    pub systemd_units: usize,
    pub failed_units: usize,
    pub docker_containers: usize,
    pub kernel_events: usize,
    pub journal_events: usize,
    pub disk_io_devices: usize,
    pub dns_checks: usize,
    pub tls_certs: usize,
    pub ntp_status: usize,
    pub ssh_logins: usize,
    pub pending_updates: usize,
    pub hardware_health: usize,
    pub errors: Vec<String>,
}

impl HostIngestStats {
    fn new(hostname: &str) -> Self {
        Self {
            hostname: hostname.to_string(),
            elapsed_seconds: 0.0,
            disks: 0,
            interfaces: 0,
            processes: 0,
            systemd_units: 0,
            failed_units: 0,
            docker_containers: 0,
            kernel_events: 0,
            journal_events: 0,
            disk_io_devices: 0,
            dns_checks: 0,
            tls_certs: 0,
            ntp_status: 0,
            ssh_logins: 0,
            pending_updates: 0,
            hardware_health: 0,
            errors: Vec::new(),
        }
    }

    pub fn summary(&self) -> String {
        let mut lines = vec![
            format!(
                "Host ingest for '{}' in {:.1}s",
                self.hostname, self.elapsed_seconds
            ),
            format!("  Disks:        {}", self.disks),
            format!("  Interfaces:   {}", self.interfaces),
            format!("  Processes:    {} (top by CPU/mem)", self.processes),
            format!(
                "  Systemd:      {} units ({} failed)",
                self.systemd_units, self.failed_units
            ),
            format!("  Docker:       {} containers", self.docker_containers),
            format!("  Kernel:       {} events", self.kernel_events),
            format!("  Journal:      {} events", self.journal_events),
            format!("  Disk I/O:     {} devices", self.disk_io_devices),
            format!("  DNS checks:   {}", self.dns_checks),
            format!("  TLS certs:    {}", self.tls_certs),
            format!("  NTP status:   {}", self.ntp_status),
            format!("  SSH logins:   {}", self.ssh_logins),
            format!("  Pending updates: {}", self.pending_updates),
            format!("  HW health:    {}", self.hardware_health),
        ];
        if !self.errors.is_empty() {
            lines.push(format!("  Errors:       {}", self.errors.len()));
        }
        lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Ingestor
// ---------------------------------------------------------------------------

pub struct HostIngestor {
    client: GraphClient,
    hostname: String,
    top_n: usize,
    dmesg_lines: usize,
    journal_lines: usize,
}

impl HostIngestor {
    pub fn new(client: GraphClient, hostname: Option<String>, top_n: usize) -> Self {
        let hostname =
            hostname.unwrap_or_else(|| gethostname::gethostname().to_string_lossy().to_string());
        Self {
            client,
            hostname,
            top_n,
            dmesg_lines: 500,
            journal_lines: 500,
        }
    }

    pub fn snapshot(&self) -> HostIngestStats {
        let t0 = Instant::now();
        let mut stats = HostIngestStats::new(&self.hostname);

        // 1. Host node
        if let Err(e) = self.ingest_host() {
            stats.errors.push(format!("host: {}", e));
        }

        // 2. Disks
        match self.ingest_disks() {
            Ok(n) => stats.disks = n,
            Err(e) => stats.errors.push(format!("disks: {}", e)),
        }

        // 3. Network interfaces
        match self.ingest_interfaces() {
            Ok(n) => stats.interfaces = n,
            Err(e) => stats.errors.push(format!("interfaces: {}", e)),
        }

        // 3b. WiFi quality (reads /proc/net/wireless)
        if let Err(e) = self.ingest_wifi_quality() {
            // WiFi not present is not an error — skip silently
            let _ = e;
        }

        // 4. Top processes
        match self.ingest_processes() {
            Ok(n) => stats.processes = n,
            Err(e) => stats.errors.push(format!("processes: {}", e)),
        }

        // 5. Systemd units
        match self.ingest_systemd() {
            Ok((total, failed)) => {
                stats.systemd_units = total;
                stats.failed_units = failed;
            }
            Err(e) => stats.errors.push(format!("systemd: {}", e)),
        }

        // 6. Docker containers
        match self.ingest_docker() {
            Ok(n) => stats.docker_containers = n,
            Err(e) => stats.errors.push(format!("docker: {}", e)),
        }

        // 7. Kernel events (dmesg)
        match self.ingest_dmesg() {
            Ok(n) => stats.kernel_events = n,
            Err(e) => stats.errors.push(format!("dmesg: {}", e)),
        }

        // 8. Journal errors
        match self.ingest_journal() {
            Ok(n) => stats.journal_events = n,
            Err(e) => stats.errors.push(format!("journal: {}", e)),
        }

        // 9. Disk I/O stats
        match self.ingest_disk_io() {
            Ok(n) => stats.disk_io_devices = n,
            Err(e) => stats.errors.push(format!("disk_io: {}", e)),
        }

        // 10. DNS resolution checks
        match self.ingest_dns_check() {
            Ok(n) => stats.dns_checks = n,
            Err(e) => stats.errors.push(format!("dns: {}", e)),
        }

        // 11. TLS certificate expiry
        match self.ingest_tls_certs() {
            Ok(n) => stats.tls_certs = n,
            Err(e) => stats.errors.push(format!("tls_certs: {}", e)),
        }

        // 12. NTP synchronization status
        match self.ingest_ntp_status() {
            Ok(n) => stats.ntp_status = n,
            Err(e) => stats.errors.push(format!("ntp: {}", e)),
        }

        // 13. SSH login activity
        match self.ingest_ssh_logins() {
            Ok(n) => stats.ssh_logins = n,
            Err(e) => stats.errors.push(format!("ssh_logins: {}", e)),
        }

        // 14. Pending security updates
        match self.ingest_pending_updates() {
            Ok(n) => stats.pending_updates = n,
            Err(e) => stats.errors.push(format!("pending_updates: {}", e)),
        }

        // 15. Hardware health (temps, SMART)
        match self.ingest_hardware_health() {
            Ok(n) => stats.hardware_health = n,
            Err(e) => stats.errors.push(format!("hardware_health: {}", e)),
        }

        stats.elapsed_seconds = t0.elapsed().as_secs_f64();
        stats
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn merge(&self, cypher: &str, params: &[(&str, ParamValue)]) -> Result<(), String> {
        self.client
            .query_typed(cypher, params)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // 1. Host node
    // ------------------------------------------------------------------

    fn ingest_host(&self) -> Result<(), String> {
        let uname = uname_info();
        let uptime = read_uptime().unwrap_or(0);
        let cpu_count = num_cpus();
        let cpu_percent = read_cpu_percent().unwrap_or(0.0);
        let mem = read_meminfo();
        let load = read_loadavg().unwrap_or((0.0, 0.0, 0.0));

        let os_str = format!("{} {}", uname.0, uname.1);

        self.merge(
            "MERGE (h:Host {hostname: $hostname}) \
             SET h.os = $os, h.kernel = $kernel, h.uptime_seconds = $uptime, \
             h.cpu_count = $cpus, h.cpu_percent = $cpu_pct, \
             h.memory_total_mb = $mem_total, h.memory_used_mb = $mem_used, \
             h.memory_percent = $mem_pct, h.swap_total_mb = $swap_total, \
             h.swap_used_mb = $swap_used, h.load_1m = $l1, h.load_5m = $l5, \
             h.load_15m = $l15",
            &[
                ("hostname", ParamValue::Str(self.hostname.clone())),
                ("os", ParamValue::Str(os_str)),
                ("kernel", ParamValue::Str(uname.1.clone())),
                ("uptime", ParamValue::Int(uptime)),
                ("cpus", ParamValue::Int(cpu_count as i64)),
                ("cpu_pct", ParamValue::Float(cpu_percent)),
                ("mem_total", ParamValue::Int(mem.total_mb)),
                ("mem_used", ParamValue::Int(mem.used_mb)),
                ("mem_pct", ParamValue::Float(mem.percent)),
                ("swap_total", ParamValue::Int(mem.swap_total_mb)),
                ("swap_used", ParamValue::Int(mem.swap_used_mb)),
                ("l1", ParamValue::Float(load.0)),
                ("l5", ParamValue::Float(load.1)),
                ("l15", ParamValue::Float(load.2)),
            ],
        )
    }

    // ------------------------------------------------------------------
    // 2. Disks
    // ------------------------------------------------------------------

    fn ingest_disks(&self) -> Result<usize, String> {
        let partitions = get_disk_partitions();
        let mut n = 0usize;
        for part in &partitions {
            let usage = match disk_usage(&part.mountpoint) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let total_gb = usage.total as f64 / 1e9;
            let used_gb = usage.used as f64 / 1e9;
            let free_gb = usage.free as f64 / 1e9;
            let pct = if usage.total > 0 {
                100.0 * usage.used as f64 / usage.total as f64
            } else {
                0.0
            };

            self.merge(
                "MERGE (d:HostDisk {hostname: $hostname, mountpoint: $mp}) \
                 SET d.device = $dev, d.fstype = $fs, d.total_gb = $total, \
                 d.used_gb = $used, d.free_gb = $free, d.percent = $pct",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("mp", ParamValue::Str(part.mountpoint.clone())),
                    ("dev", ParamValue::Str(part.device.clone())),
                    ("fs", ParamValue::Str(part.fstype.clone())),
                    ("total", ParamValue::Float(round2(total_gb))),
                    ("used", ParamValue::Float(round2(used_gb))),
                    ("free", ParamValue::Float(round2(free_gb))),
                    ("pct", ParamValue::Float(round1(pct))),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (d:HostDisk {hostname: $hn, mountpoint: $mp}) \
                 MERGE (h)-[:HAS_DISK]->(d)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("mp", ParamValue::Str(part.mountpoint.clone())),
                ],
            )?;
            n += 1;
        }
        Ok(n)
    }

    // ------------------------------------------------------------------
    // 3. Network interfaces
    // ------------------------------------------------------------------

    fn ingest_interfaces(&self) -> Result<usize, String> {
        let output = Command::new("ip")
            .args(["-j", "addr", "show"])
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Ok(0);
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let ifaces: Vec<serde_json::Value> =
            serde_json::from_str(&stdout).map_err(|e| e.to_string())?;

        let mut n = 0usize;
        for iface in &ifaces {
            let name = iface["ifname"].as_str().unwrap_or("");
            if name.is_empty() || name == "lo" {
                continue;
            }
            let state = iface["operstate"]
                .as_str()
                .unwrap_or("UNKNOWN")
                .to_lowercase();
            let mac = iface["address"].as_str().unwrap_or("").to_string();
            let mtu = iface["mtu"].as_i64().unwrap_or(0);
            let mut ipv4 = String::new();
            let mut ipv6 = String::new();
            if let Some(addrs) = iface["addr_info"].as_array() {
                for addr in addrs {
                    let family = addr["family"].as_str().unwrap_or("");
                    let local = addr["local"].as_str().unwrap_or("");
                    if family == "inet" && ipv4.is_empty() {
                        ipv4 = local.to_string();
                    } else if family == "inet6" && ipv6.is_empty() {
                        ipv6 = local.to_string();
                    }
                }
            }

            self.merge(
                "MERGE (n:HostNetIface {hostname: $hostname, name: $name}) \
                 SET n.ipv4 = $ipv4, n.ipv6 = $ipv6, n.mac = $mac, \
                 n.state = $state, n.mtu = $mtu",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("name", ParamValue::Str(name.to_string())),
                    ("ipv4", ParamValue::Str(ipv4)),
                    ("ipv6", ParamValue::Str(ipv6)),
                    ("mac", ParamValue::Str(mac)),
                    ("state", ParamValue::Str(state)),
                    ("mtu", ParamValue::Int(mtu)),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (n:HostNetIface {hostname: $hn, name: $name}) \
                 MERGE (h)-[:HAS_IFACE]->(n)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("name", ParamValue::Str(name.to_string())),
                ],
            )?;
            n += 1;
        }
        Ok(n)
    }

    // ------------------------------------------------------------------
    // 3b. WiFi quality from /proc/net/wireless
    // ------------------------------------------------------------------

    fn ingest_wifi_quality(&self) -> Result<(), String> {
        let content = fs::read_to_string("/proc/net/wireless")
            .map_err(|e| format!("no wifi: {}", e))?;

        for line in content.lines().skip(2) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 11 { continue; }

            let iface = parts[0].trim_end_matches(':');
            let quality = parts[2].trim_end_matches('.').parse::<i32>().unwrap_or(0);
            let signal_dbm = parts[3].trim_end_matches('.').parse::<i32>().unwrap_or(0);
            let noise_dbm = parts[4].trim_end_matches('.').parse::<i32>().unwrap_or(0);
            let discard_nwid = parts[5].parse::<i64>().unwrap_or(0);
            let discard_crypt = parts[6].parse::<i64>().unwrap_or(0);
            let discard_frag = parts[7].parse::<i64>().unwrap_or(0);
            let retry = parts[8].parse::<i64>().unwrap_or(0);
            let discard_misc = parts[9].parse::<i64>().unwrap_or(0);
            let missed_beacon = parts[10].parse::<i64>().unwrap_or(0);

            let total_discarded = discard_nwid + discard_crypt + discard_frag + discard_misc;

            // Get WiFi band/channel from NetworkManager
            let (band, channel, ssid) = get_wifi_info(iface);

            // Write to graph
            let q = format!(
                "MERGE (w:WifiStatus {{hostname: '{}', interface: '{}'}}) \
                 SET w.quality = {}, w.signal_dbm = {}, w.noise_dbm = {}, \
                 w.retry = {}, w.discarded = {}, w.missed_beacon = {}, \
                 w.band = '{}', w.channel = '{}', w.ssid = '{}'",
                self.hostname, iface,
                quality, signal_dbm, noise_dbm,
                retry, total_discarded, missed_beacon,
                band, channel, ssid,
            );
            let _ = self.client.query(&q, &[]);

            // Edge: Host HAS_WIFI WifiStatus
            let edge_q = format!(
                "MATCH (h:Host {{hostname: '{}'}}) \
                 MATCH (w:WifiStatus {{hostname: '{}', interface: '{}'}}) \
                 MERGE (h)-[:HAS_WIFI]->(w)",
                self.hostname, self.hostname, iface,
            );
            let _ = self.client.query(&edge_q, &[]);

            // Auto-diagnose: flag high discard counts
            if total_discarded > 1000 {
                let diag_q = format!(
                    "MERGE (e:HostLogEvent {{hostname: '{}', source: 'wifi', template_hash: 'wifi-discard-high'}}) \
                     SET e.severity = 'WARNING', e.template_text = 'WiFi {} discarding {} packets (signal {}dBm, band {}, ch {}). Check channel congestion and power management.', \
                     e.count = {}, e.last_seen = {}",
                    self.hostname,
                    iface, total_discarded, signal_dbm, band, channel,
                    total_discarded,
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64(),
                );
                let _ = self.client.query(&diag_q, &[]);
            }

            // Also flag if signal is weak
            if signal_dbm < -75 && signal_dbm > -256 {
                let diag_q = format!(
                    "MERGE (e:HostLogEvent {{hostname: '{}', source: 'wifi', template_hash: 'wifi-signal-weak'}}) \
                     SET e.severity = 'WARNING', e.template_text = 'WiFi {} signal weak at {}dBm (band {}, ch {}). May cause packet loss and connectivity drops.', \
                     e.count = 1, e.last_seen = {}",
                    self.hostname,
                    iface, signal_dbm, band, channel,
                    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64(),
                );
                let _ = self.client.query(&diag_q, &[]);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // 4. Top processes
    // ------------------------------------------------------------------

    fn ingest_processes(&self) -> Result<usize, String> {
        let mut procs = Vec::new();
        let proc_dir = match fs::read_dir("/proc") {
            Ok(d) => d,
            Err(e) => return Err(e.to_string()),
        };

        for entry in proc_dir.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let pid: i64 = match name_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            let pid_path = entry.path();
            let stat_path = pid_path.join("stat");
            let stat_content = match fs::read_to_string(&stat_path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Parse /proc/{pid}/stat — name is in parens, may contain spaces
            let open = match stat_content.find('(') {
                Some(i) => i,
                None => continue,
            };
            let close = match stat_content.rfind(')') {
                Some(i) => i,
                None => continue,
            };
            let proc_name = stat_content[open + 1..close].to_string();
            let rest: Vec<&str> = stat_content[close + 2..].split_whitespace().collect();
            if rest.len() < 22 {
                continue;
            }

            let status_char = rest[0];
            let status = match status_char {
                "R" => "running",
                "S" => "sleeping",
                "D" => "disk-sleep",
                "Z" => "zombie",
                "T" => "stopped",
                "I" => "idle",
                other => other,
            }
            .to_string();

            let utime: i64 = rest[11].parse().unwrap_or(0);
            let stime: i64 = rest[12].parse().unwrap_or(0);
            let cpu_ticks = utime + stime;
            let rss_pages: i64 = rest[21].parse().unwrap_or(0);
            let mem_mb = (rss_pages as f64 * 4096.0) / 1e6;

            // Read cmdline
            let cmdline = fs::read_to_string(pid_path.join("cmdline"))
                .unwrap_or_default()
                .replace('\0', " ")
                .trim()
                .chars()
                .take(200)
                .collect::<String>();
            let cmdline = if cmdline.is_empty() {
                proc_name.clone()
            } else {
                cmdline
            };

            // Read user from /proc/{pid}/status
            let user = read_proc_user(&pid_path);

            procs.push(ProcInfo {
                pid,
                name: proc_name,
                cmdline,
                cpu_ticks,
                mem_mb: round1(mem_mb),
                user,
                status,
            });
        }

        // Top N by memory + top N by CPU, deduplicated
        procs.sort_by(|a, b| b.mem_mb.partial_cmp(&a.mem_mb).unwrap_or(std::cmp::Ordering::Equal));
        let by_mem: Vec<&ProcInfo> = procs.iter().take(self.top_n).collect();

        let mut by_cpu = procs.clone();
        by_cpu.sort_by(|a, b| b.cpu_ticks.cmp(&a.cpu_ticks));
        let by_cpu: Vec<&ProcInfo> = by_cpu.iter().take(self.top_n).collect();

        let mut seen = HashSet::new();
        let mut top = Vec::new();
        for p in by_mem.iter().chain(by_cpu.iter()) {
            if seen.insert(p.pid) {
                top.push(*p);
            }
        }

        // Clear old process nodes
        self.merge(
            "MATCH (p:HostProcess {hostname: $hn}) DETACH DELETE p",
            &[("hn", ParamValue::Str(self.hostname.clone()))],
        )?;

        for p in &top {
            self.merge(
                "MERGE (p:HostProcess {hostname: $hostname, pid: $pid}) \
                 SET p.name = $name, p.cmdline = $cmd, p.cpu_percent = $cpu, \
                 p.memory_mb = $mem, p.user = $user, p.status = $status",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("pid", ParamValue::Int(p.pid)),
                    ("name", ParamValue::Str(p.name.clone())),
                    ("cmd", ParamValue::Str(p.cmdline.clone())),
                    ("cpu", ParamValue::Float(0.0)),
                    ("mem", ParamValue::Float(p.mem_mb)),
                    ("user", ParamValue::Str(p.user.clone())),
                    ("status", ParamValue::Str(p.status.clone())),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (p:HostProcess {hostname: $hn, pid: $pid}) \
                 MERGE (h)-[:RUNS]->(p)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("pid", ParamValue::Int(p.pid)),
                ],
            )?;
        }

        Ok(top.len())
    }

    // ------------------------------------------------------------------
    // 5. Systemd units
    // ------------------------------------------------------------------

    fn ingest_systemd(&self) -> Result<(usize, usize), String> {
        let output = Command::new("systemctl")
            .args([
                "list-units",
                "--all",
                "--no-pager",
                "--plain",
                "--no-legend",
            ])
            .output()
            .map_err(|e| e.to_string())?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut total = 0usize;
        let mut failed = 0usize;

        for line in stdout.lines() {
            let parts: Vec<&str> = line.splitn(5, char::is_whitespace).collect();
            if parts.len() < 4 {
                continue;
            }
            let name = parts[0];
            let active = parts[2];
            let sub = parts[3];

            let unit_type = name.rsplit('.').next().unwrap_or("");
            if !["service", "timer", "mount", "socket"].contains(&unit_type) {
                continue;
            }

            let desc = if parts.len() > 4 {
                parts[4].trim()
            } else {
                ""
            };

            self.merge(
                "MERGE (u:SystemdUnit {hostname: $hostname, name: $name}) \
                 SET u.type = $type, u.active_state = $active, \
                 u.sub_state = $sub, u.description = $desc",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("name", ParamValue::Str(name.to_string())),
                    ("type", ParamValue::Str(unit_type.to_string())),
                    ("active", ParamValue::Str(active.to_string())),
                    ("sub", ParamValue::Str(sub.to_string())),
                    ("desc", ParamValue::Str(desc.to_string())),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (u:SystemdUnit {hostname: $hn, name: $name}) \
                 MERGE (h)-[:HAS_UNIT]->(u)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("name", ParamValue::Str(name.to_string())),
                ],
            )?;

            total += 1;
            if active == "failed" {
                failed += 1;
            }
        }

        Ok((total, failed))
    }

    // ------------------------------------------------------------------
    // 6. Docker containers
    // ------------------------------------------------------------------

    fn ingest_docker(&self) -> Result<usize, String> {
        // Check if docker binary exists
        if which("docker").is_none() {
            return Ok(0);
        }

        let output = Command::new("docker")
            .args([
                "ps",
                "-a",
                "--format",
                "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}\t{{.CreatedAt}}",
            ])
            .output()
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let re_restart = Regex::new(r"Restarting \((\d+)\)").unwrap();
        let mut n = 0usize;

        for line in stdout.trim().lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 {
                continue;
            }
            let cid = &parts[0][..parts[0].len().min(12)];
            let name = parts.get(1).copied().unwrap_or("");
            let image = parts.get(2).copied().unwrap_or("");
            let status = parts.get(3).copied().unwrap_or("");
            let ports_str = parts.get(4).copied().unwrap_or("");
            let created = parts.get(5).copied().unwrap_or("");

            let ports: Vec<String> = if ports_str.is_empty() {
                Vec::new()
            } else {
                ports_str.split(", ").map(|s| s.to_string()).collect()
            };

            let mut state = if status.contains("Up") {
                "running"
            } else {
                "exited"
            }
            .to_string();

            let mut rc = 0i64;
            if let Some(caps) = re_restart.captures(status) {
                rc = caps[1].parse().unwrap_or(0);
                state = "restarting".to_string();
            }

            self.merge(
                "MERGE (c:DockerContainer {hostname: $hostname, container_id: $cid}) \
                 SET c.name = $name, c.image = $image, c.status = $status, \
                 c.state = $state, c.ports = $ports, c.created_at = $created, \
                 c.restart_count = $rc",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("cid", ParamValue::Str(cid.to_string())),
                    ("name", ParamValue::Str(name.to_string())),
                    ("image", ParamValue::Str(image.to_string())),
                    ("status", ParamValue::Str(status.to_string())),
                    ("state", ParamValue::Str(state)),
                    ("ports", ParamValue::StrList(ports)),
                    ("created", ParamValue::Str(created.to_string())),
                    ("rc", ParamValue::Int(rc)),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (c:DockerContainer {hostname: $hn, container_id: $cid}) \
                 MERGE (h)-[:RUNS_CONTAINER]->(c)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("cid", ParamValue::Str(cid.to_string())),
                ],
            )?;
            n += 1;
        }
        Ok(n)
    }

    // ------------------------------------------------------------------
    // 7. Kernel events (dmesg)
    // ------------------------------------------------------------------

    fn ingest_dmesg(&self) -> Result<usize, String> {
        let output = Command::new("dmesg")
            .args(["-l", "err,warn,crit,alert,emerg"])
            .output()
            .map_err(|e| e.to_string())?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        let start = if lines.len() > self.dmesg_lines {
            lines.len() - self.dmesg_lines
        } else {
            0
        };
        let lines = &lines[start..];

        self.ingest_kernel_lines(lines)
    }

    fn ingest_kernel_lines(&self, lines: &[&str]) -> Result<usize, String> {
        if lines.is_empty() {
            return Ok(0);
        }

        let cat_patterns: Vec<(Regex, &str)> = vec![
            (Regex::new(r"(?i)Out of memory|oom-kill|oom_reaper|invoked oom-killer").unwrap(), "oom"),
            (Regex::new(r"(?i)I/O error|Buffer I/O error|blk_update_request").unwrap(), "io_error"),
            (Regex::new(r"(?i)segfault|general protection fault").unwrap(), "segfault"),
            (Regex::new(r"(?i)Hardware Error|MCE|mce:|GHES").unwrap(), "hardware"),
            (Regex::new(r"(?i)nfs|NFSD|rpc_task").unwrap(), "nfs"),
            (Regex::new(r"(?i)Kernel panic|BUG:").unwrap(), "panic"),
        ];

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut buckets: HashMap<String, KernelBucket> = HashMap::new();

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let (tmpl, hash) = extract_template(line);

            // Detect category
            let mut cat = "other";
            for (pat, label) in cat_patterns.iter() {
                if pat.is_match(line) {
                    cat = label;
                    break;
                }
            }

            // Detect severity
            let lower = line.to_lowercase();
            let sev = if ["emerg", "panic", "bug:", "oops"]
                .iter()
                .any(|kw| lower.contains(kw))
            {
                "FATAL"
            } else if ["err", "crit", "alert", "error"]
                .iter()
                .any(|kw| lower.contains(kw))
            {
                "ERROR"
            } else {
                "WARN"
            };

            let bucket = buckets.entry(hash.clone()).or_insert_with(|| KernelBucket {
                template_hash: hash,
                template_text: tmpl,
                category: cat.to_string(),
                severity: sev.to_string(),
                first_seen: now,
                last_seen: now,
                count: 0,
                examples: Vec::new(),
            });
            bucket.count += 1;
            bucket.last_seen = now;
            if bucket.examples.len() < 5 {
                bucket
                    .examples
                    .push(line.chars().take(300).collect());
            }
        }

        for b in buckets.values() {
            self.merge(
                "MERGE (e:KernelEvent {hostname: $hostname, template_hash: $th}) \
                 SET e.category = $cat, e.severity = $severity, \
                 e.template_text = $tmpl, e.last_seen = $last_seen, \
                 e.count = $count, e.example_lines = $examples, \
                 e.first_seen = CASE WHEN e.first_seen IS NULL OR e.first_seen = 0 \
                                     THEN $first_seen ELSE e.first_seen END",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("th", ParamValue::Str(b.template_hash.clone())),
                    ("cat", ParamValue::Str(b.category.clone())),
                    ("severity", ParamValue::Str(b.severity.clone())),
                    ("tmpl", ParamValue::Str(b.template_text.clone())),
                    ("first_seen", ParamValue::Float(b.first_seen)),
                    ("last_seen", ParamValue::Float(b.last_seen)),
                    ("count", ParamValue::Int(b.count)),
                    ("examples", ParamValue::StrList(b.examples.clone())),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (e:KernelEvent {hostname: $hn, template_hash: $th}) \
                 MERGE (h)-[:EMITTED]->(e)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("th", ParamValue::Str(b.template_hash.clone())),
                ],
            )?;
        }

        Ok(buckets.len())
    }

    // ------------------------------------------------------------------
    // 8. Journal errors
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // 9. Disk I/O from /proc/diskstats
    // ------------------------------------------------------------------

    fn ingest_disk_io(&self) -> Result<usize, String> {
        let content = fs::read_to_string("/proc/diskstats")
            .map_err(|e| format!("cannot read /proc/diskstats: {}", e))?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut n = 0usize;
        for line in content.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // /proc/diskstats has 14+ fields; field indices (0-based):
            // 2=device, 3=reads_completed, 4=reads_merged, 5=sectors_read,
            // 6=time_reading_ms, 7=writes_completed, 8=writes_merged,
            // 9=sectors_written, 10=time_writing_ms, 11=ios_in_progress,
            // 12=time_io_ms, 13=weighted_io_time_ms
            if parts.len() < 14 {
                continue;
            }

            let device = parts[2];
            // Skip partition slices (e.g. sda1) — only keep whole devices and dm-*
            if device.starts_with("loop") || device.starts_with("ram") {
                continue;
            }

            let reads_completed: i64 = parts[3].parse().unwrap_or(0);
            let writes_completed: i64 = parts[7].parse().unwrap_or(0);
            // Skip devices with zero activity
            if reads_completed == 0 && writes_completed == 0 {
                continue;
            }

            let time_reading_ms: i64 = parts[6].parse().unwrap_or(0);
            let time_writing_ms: i64 = parts[10].parse().unwrap_or(0);
            let io_in_progress: i64 = parts[11].parse().unwrap_or(0);
            let time_io_ms: i64 = parts[12].parse().unwrap_or(0);
            let weighted_io_time_ms: i64 = parts[13].parse().unwrap_or(0);

            self.merge(
                "MERGE (d:HostDiskIO {hostname: $hostname, device: $device}) \
                 SET d.reads_completed = $reads, d.writes_completed = $writes, \
                 d.time_reading_ms = $tr, d.time_writing_ms = $tw, \
                 d.io_in_progress = $iop, d.time_io_ms = $tio, \
                 d.weighted_io_time_ms = $wio, d.last_seen = $now",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("device", ParamValue::Str(device.to_string())),
                    ("reads", ParamValue::Int(reads_completed)),
                    ("writes", ParamValue::Int(writes_completed)),
                    ("tr", ParamValue::Int(time_reading_ms)),
                    ("tw", ParamValue::Int(time_writing_ms)),
                    ("iop", ParamValue::Int(io_in_progress)),
                    ("tio", ParamValue::Int(time_io_ms)),
                    ("wio", ParamValue::Int(weighted_io_time_ms)),
                    ("now", ParamValue::Float(now)),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (d:HostDiskIO {hostname: $hn, device: $device}) \
                 MERGE (h)-[:HAS_DISK_IO]->(d)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("device", ParamValue::Str(device.to_string())),
                ],
            )?;

            // Flag high weighted I/O time (> 10000ms indicates I/O saturation)
            if weighted_io_time_ms > 10000 {
                let diag_q = format!(
                    "MERGE (e:HostLogEvent {{hostname: '{}', source: 'disk_io', template_hash: 'diskio-high-{}'}}) \
                     SET e.severity = 'WARNING', e.template_text = 'Disk {} weighted I/O time {}ms exceeds 10000ms threshold — possible I/O saturation.', \
                     e.count = 1, e.last_seen = {}",
                    self.hostname, device, device, weighted_io_time_ms, now,
                );
                let _ = self.client.query(&diag_q, &[]);
            }

            n += 1;
        }
        Ok(n)
    }

    // ------------------------------------------------------------------
    // 10. DNS resolution check
    // ------------------------------------------------------------------

    fn ingest_dns_check(&self) -> Result<usize, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut n = 0usize;

        // Check against 1.1.1.1 (Cloudflare)
        if let Some(ms) = self.run_dns_query("1.1.1.1") {
            self.merge(
                "MERGE (d:DnsCheck {hostname: $hostname, resolver: $resolver}) \
                 SET d.response_ms = $ms, d.status = $status, d.last_seen = $now",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("resolver", ParamValue::Str("1.1.1.1".to_string())),
                    ("ms", ParamValue::Float(ms)),
                    ("status", ParamValue::Str(if ms < 0.0 { "FAIL".to_string() } else { "OK".to_string() })),
                    ("now", ParamValue::Float(now)),
                ],
            )?;
            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (d:DnsCheck {hostname: $hn, resolver: $resolver}) \
                 MERGE (h)-[:HAS_DNS]->(d)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("resolver", ParamValue::Str("1.1.1.1".to_string())),
                ],
            )?;

            if ms < 0.0 || ms > 500.0 {
                let diag_q = format!(
                    "MERGE (e:HostLogEvent {{hostname: '{}', source: 'dns', template_hash: 'dns-slow-1.1.1.1'}}) \
                     SET e.severity = 'WARNING', e.template_text = 'DNS resolution via 1.1.1.1 {}. Possible network or DNS issue.', \
                     e.count = 1, e.last_seen = {}",
                    self.hostname,
                    if ms < 0.0 { "failed".to_string() } else { format!("took {:.0}ms (>500ms)", ms) },
                    now,
                );
                let _ = self.client.query(&diag_q, &[]);
            }
            n += 1;
        }

        // Check system-configured DNS from /etc/resolv.conf
        if let Ok(resolv) = fs::read_to_string("/etc/resolv.conf") {
            for line in resolv.lines() {
                let line = line.trim();
                if line.starts_with("nameserver") {
                    if let Some(ns) = line.split_whitespace().nth(1) {
                        if ns == "1.1.1.1" { continue; } // already checked
                        if let Some(ms) = self.run_dns_query(ns) {
                            self.merge(
                                "MERGE (d:DnsCheck {hostname: $hostname, resolver: $resolver}) \
                                 SET d.response_ms = $ms, d.status = $status, d.last_seen = $now",
                                &[
                                    ("hostname", ParamValue::Str(self.hostname.clone())),
                                    ("resolver", ParamValue::Str(ns.to_string())),
                                    ("ms", ParamValue::Float(ms)),
                                    ("status", ParamValue::Str(if ms < 0.0 { "FAIL".to_string() } else { "OK".to_string() })),
                                    ("now", ParamValue::Float(now)),
                                ],
                            )?;
                            self.merge(
                                "MATCH (h:Host {hostname: $hn}) \
                                 MATCH (d:DnsCheck {hostname: $hn, resolver: $resolver}) \
                                 MERGE (h)-[:HAS_DNS]->(d)",
                                &[
                                    ("hn", ParamValue::Str(self.hostname.clone())),
                                    ("resolver", ParamValue::Str(ns.to_string())),
                                ],
                            )?;

                            if ms < 0.0 || ms > 500.0 {
                                let diag_q = format!(
                                    "MERGE (e:HostLogEvent {{hostname: '{}', source: 'dns', template_hash: 'dns-slow-{}'}}) \
                                     SET e.severity = 'WARNING', e.template_text = 'DNS resolution via {} {}. Possible network or DNS issue.', \
                                     e.count = 1, e.last_seen = {}",
                                    self.hostname, ns, ns,
                                    if ms < 0.0 { "failed".to_string() } else { format!("took {:.0}ms (>500ms)", ms) },
                                    now,
                                );
                                let _ = self.client.query(&diag_q, &[]);
                            }
                            n += 1;
                        }
                        break; // Only check the first system nameserver
                    }
                }
            }
        }

        Ok(n)
    }

    /// Run `dig` against a resolver and return response time in ms, or None if dig is unavailable.
    /// Returns negative value on failure.
    fn run_dns_query(&self, resolver: &str) -> Option<f64> {
        if which("dig").is_none() {
            return None;
        }
        let output = Command::new("timeout")
            .args(["2", "dig", &format!("@{}", resolver), "google.com", "+time=2", "+tries=1"])
            .output()
            .ok()?;

        if !output.status.success() {
            return Some(-1.0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse ";; Query time: <N> msec" from dig output
        for line in stdout.lines() {
            let line = line.trim();
            if line.contains("Query time:") {
                let re = Regex::new(r"Query time:\s*(\d+)\s*msec").unwrap();
                if let Some(caps) = re.captures(line) {
                    return Some(caps[1].parse::<f64>().unwrap_or(-1.0));
                }
            }
        }
        Some(-1.0)
    }

    // ------------------------------------------------------------------
    // 11. TLS certificate expiry for listening ports
    // ------------------------------------------------------------------

    fn ingest_tls_certs(&self) -> Result<usize, String> {
        if which("openssl").is_none() {
            return Ok(0);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        // Parse /proc/net/tcp for LISTEN state (state = 0A)
        let content = fs::read_to_string("/proc/net/tcp").unwrap_or_default();
        let mut listen_ports: Vec<u16> = Vec::new();

        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 4 { continue; }
            // state is field 3 (0-indexed)
            if parts[3] != "0A" { continue; } // 0A = LISTEN

            // local_address is field 1, format hex_ip:hex_port
            if let Some(port_hex) = parts[1].split(':').nth(1) {
                if let Ok(port) = u16::from_str_radix(port_hex, 16) {
                    // Skip common non-TLS ports and very high ephemeral ports
                    if port > 0 && port < 49152 && !listen_ports.contains(&port) {
                        listen_ports.push(port);
                    }
                }
            }
        }

        let mut n = 0usize;
        for port in &listen_ports {
            let output = Command::new("timeout")
                .args([
                    "2",
                    "openssl",
                    "s_client",
                    "-connect",
                    &format!("localhost:{}", port),
                    "-servername",
                    "localhost",
                ])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output();

            let cert_pem = match output {
                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                Err(_) => continue,
            };

            if !cert_pem.contains("BEGIN CERTIFICATE") {
                continue;
            }

            // Pipe cert through openssl x509 to get enddate
            let mut child = match Command::new("openssl")
                .args(["x509", "-noout", "-enddate", "-subject"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => continue,
            };

            if let Some(ref mut stdin) = child.stdin {
                use std::io::Write;
                let _ = stdin.write_all(cert_pem.as_bytes());
            }

            let x509_output = match child.wait_with_output() {
                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                Err(_) => continue,
            };

            let mut end_date = String::new();
            let mut subject = String::new();
            let mut days_remaining: i64 = -1;

            for line in x509_output.lines() {
                if line.starts_with("notAfter=") {
                    end_date = line.trim_start_matches("notAfter=").trim().to_string();
                    // Parse: "Mon DD HH:MM:SS YYYY GMT" or similar openssl date format
                    // We'll compute days remaining via a date command
                    if let Ok(out) = Command::new("date")
                        .args(["-d", &end_date, "+%s"])
                        .output()
                    {
                        if let Ok(epoch) = String::from_utf8_lossy(&out.stdout).trim().parse::<f64>() {
                            days_remaining = ((epoch - now) / 86400.0) as i64;
                        }
                    }
                } else if line.starts_with("subject=") {
                    subject = line.trim_start_matches("subject=").trim().to_string();
                }
            }

            if end_date.is_empty() { continue; }

            let flagged = days_remaining >= 0 && days_remaining < 30;

            self.merge(
                "MERGE (t:TlsCert {hostname: $hostname, port: $port}) \
                 SET t.subject = $subject, t.expires = $expires, \
                 t.days_remaining = $days, t.flagged = $flagged, t.last_seen = $now",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("port", ParamValue::Int(*port as i64)),
                    ("subject", ParamValue::Str(subject)),
                    ("expires", ParamValue::Str(end_date.clone())),
                    ("days", ParamValue::Int(days_remaining)),
                    ("flagged", ParamValue::Bool(flagged)),
                    ("now", ParamValue::Float(now)),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (t:TlsCert {hostname: $hn, port: $port}) \
                 MERGE (h)-[:HAS_TLS_CERT]->(t)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("port", ParamValue::Int(*port as i64)),
                ],
            )?;

            if flagged {
                let diag_q = format!(
                    "MERGE (e:HostLogEvent {{hostname: '{}', source: 'tls', template_hash: 'tls-expiry-{}'}}) \
                     SET e.severity = 'WARNING', e.template_text = 'TLS cert on port {} expires in {} days ({}). Renew soon.', \
                     e.count = 1, e.last_seen = {}",
                    self.hostname, port, port, days_remaining, end_date, now,
                );
                let _ = self.client.query(&diag_q, &[]);
            }

            n += 1;
        }
        Ok(n)
    }

    // ------------------------------------------------------------------
    // 12. NTP synchronization status
    // ------------------------------------------------------------------

    fn ingest_ntp_status(&self) -> Result<usize, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut synchronized = false;
        let mut ntp_service = String::new();
        let mut offset_seconds: f64 = 0.0;
        let mut source_found = false;

        // Try timedatectl show (systemd-based)
        if let Ok(output) = Command::new("timedatectl").arg("show").output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let line = line.trim();
                    if line.starts_with("NTPSynchronized=") {
                        synchronized = line.ends_with("yes");
                        source_found = true;
                    } else if line.starts_with("NTP=") {
                        ntp_service = line.trim_start_matches("NTP=").to_string();
                    }
                }
            }
        }

        // Fallback: check /run/systemd/timesync/synchronized
        if !source_found {
            if Path::new("/run/systemd/timesync/synchronized").exists() {
                synchronized = true;
                ntp_service = "systemd-timesyncd".to_string();
                source_found = true;
            }
        }

        // Try to get offset from timedatectl timesync-status
        if let Ok(output) = Command::new("timedatectl").arg("timesync-status").output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let re = Regex::new(r"Offset:\s*([+-]?\d+\.?\d*)(us|ms|s)").unwrap();
                if let Some(caps) = re.captures(&stdout) {
                    let val: f64 = caps[1].parse().unwrap_or(0.0);
                    let unit = &caps[2];
                    offset_seconds = match unit {
                        "us" => val / 1_000_000.0,
                        "ms" => val / 1_000.0,
                        _ => val,
                    };
                }
            }
        }

        if !source_found {
            return Ok(0);
        }

        self.merge(
            "MERGE (n:NtpStatus {hostname: $hostname}) \
             SET n.synchronized = $synced, n.service = $svc, \
             n.offset_seconds = $offset, n.last_seen = $now",
            &[
                ("hostname", ParamValue::Str(self.hostname.clone())),
                ("synced", ParamValue::Bool(synchronized)),
                ("svc", ParamValue::Str(ntp_service)),
                ("offset", ParamValue::Float(round2(offset_seconds))),
                ("now", ParamValue::Float(now)),
            ],
        )?;

        self.merge(
            "MATCH (h:Host {hostname: $hn}) \
             MATCH (n:NtpStatus {hostname: $hn}) \
             MERGE (h)-[:HAS_NTP]->(n)",
            &[("hn", ParamValue::Str(self.hostname.clone()))],
        )?;

        // Flag if not synchronized or offset > 1 second
        if !synchronized || offset_seconds.abs() > 1.0 {
            let reason = if !synchronized {
                "NTP is not synchronized".to_string()
            } else {
                format!("Clock offset {:.3}s exceeds 1s threshold", offset_seconds)
            };
            let diag_q = format!(
                "MERGE (e:HostLogEvent {{hostname: '{}', source: 'ntp', template_hash: 'ntp-drift'}}) \
                 SET e.severity = 'WARNING', e.template_text = '{}. Time drift can cause TLS failures and log correlation issues.', \
                 e.count = 1, e.last_seen = {}",
                self.hostname, reason, now,
            );
            let _ = self.client.query(&diag_q, &[]);
        }

        Ok(1)
    }

    // ------------------------------------------------------------------
    // 13. SSH login activity
    // ------------------------------------------------------------------

    fn ingest_ssh_logins(&self) -> Result<usize, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut n = 0usize;

        // Recent logins via `last`
        if let Ok(output) = Command::new("last").args(["-n", "20"]).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Clear old SSH login nodes
                let _ = self.merge(
                    "MATCH (s:SshLogin {hostname: $hn}) DETACH DELETE s",
                    &[("hn", ParamValue::Str(self.hostname.clone()))],
                );

                for line in stdout.lines() {
                    let line = line.trim();
                    if line.is_empty()
                        || line.starts_with("wtmp begins")
                        || line.starts_with("reboot")
                    {
                        continue;
                    }

                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() < 4 { continue; }

                    let user = parts[0];
                    let terminal = parts[1];
                    let source = if parts[2].contains('.') || parts[2].contains(':') {
                        parts[2]
                    } else {
                        ""
                    };

                    self.merge(
                        "CREATE (s:SshLogin {hostname: $hostname, user: $user, \
                         terminal: $terminal, source: $source, login_type: $ltype, \
                         last_seen: $now})",
                        &[
                            ("hostname", ParamValue::Str(self.hostname.clone())),
                            ("user", ParamValue::Str(user.to_string())),
                            ("terminal", ParamValue::Str(terminal.to_string())),
                            ("source", ParamValue::Str(source.to_string())),
                            ("ltype", ParamValue::Str("recent".to_string())),
                            ("now", ParamValue::Float(now)),
                        ],
                    )?;

                    self.merge(
                        "MATCH (h:Host {hostname: $hn}) \
                         MATCH (s:SshLogin {hostname: $hn, user: $user, terminal: $terminal}) \
                         MERGE (h)-[:HAS_SSH_LOGIN]->(s)",
                        &[
                            ("hn", ParamValue::Str(self.hostname.clone())),
                            ("user", ParamValue::Str(user.to_string())),
                            ("terminal", ParamValue::Str(terminal.to_string())),
                        ],
                    )?;
                    n += 1;
                }
            }
        }

        // Check failed SSH attempts from auth.log or journalctl
        let failed_output = if Path::new("/var/log/auth.log").exists() {
            Command::new("sh")
                .args(["-c", "grep 'Failed password' /var/log/auth.log 2>/dev/null | tail -200"])
                .output()
                .ok()
        } else {
            Command::new("journalctl")
                .args(["--no-pager", "-u", "sshd", "-g", "Failed password", "-n", "200"])
                .output()
                .ok()
        };

        if let Some(output) = failed_output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let re_ip = Regex::new(r"from\s+(\S+)\s+port").unwrap();
                let mut ip_counts: HashMap<String, i64> = HashMap::new();

                for line in stdout.lines() {
                    if let Some(caps) = re_ip.captures(line) {
                        let ip = caps[1].to_string();
                        *ip_counts.entry(ip).or_insert(0) += 1;
                    }
                }

                for (ip, count) in &ip_counts {
                    if *count > 10 {
                        let diag_q = format!(
                            "MERGE (e:HostLogEvent {{hostname: '{}', source: 'ssh', template_hash: 'ssh-brute-{}'}}) \
                             SET e.severity = 'WARNING', e.template_text = '{} failed SSH login attempts from {}. Possible brute-force attack.', \
                             e.count = {}, e.last_seen = {}",
                            self.hostname, ip, count, ip, count, now,
                        );
                        let _ = self.client.query(&diag_q, &[]);
                    }
                }
            }
        }

        Ok(n)
    }

    // ------------------------------------------------------------------
    // 14. Pending security updates
    // ------------------------------------------------------------------

    fn ingest_pending_updates(&self) -> Result<usize, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut total_pending: usize = 0;
        let mut security_pending: usize = 0;
        let mut pkg_manager = String::new();

        // Try apt (Debian/Ubuntu)
        if which("apt").is_some() {
            if let Ok(output) = Command::new("apt")
                .args(["list", "--upgradable"])
                .env("DEBIAN_FRONTEND", "noninteractive")
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    pkg_manager = "apt".to_string();
                    for line in stdout.lines() {
                        if line.contains("upgradable from") || line.contains("/") {
                            if line.starts_with("Listing...") { continue; }
                            total_pending += 1;
                            if line.contains("-security") {
                                security_pending += 1;
                            }
                        }
                    }
                }
            }
        }
        // Try dnf (RHEL/Fedora)
        else if which("dnf").is_some() {
            if let Ok(output) = Command::new("dnf")
                .args(["check-update", "--quiet"])
                .output()
            {
                // dnf check-update exits 100 if updates available, 0 if none
                let stdout = String::from_utf8_lossy(&output.stdout);
                pkg_manager = "dnf".to_string();
                for line in stdout.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with("Last metadata") { continue; }
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        total_pending += 1;
                    }
                }
            }
        }
        // Try nix-channel (NixOS)
        else if which("nix-channel").is_some() {
            if let Ok(output) = Command::new("nix-channel").args(["--list"]).output() {
                if output.status.success() {
                    pkg_manager = "nix".to_string();
                    // For nix we just note the channels exist; can't easily count pending
                }
            }
        }

        if pkg_manager.is_empty() {
            return Ok(0);
        }

        // Write pending_updates to Host node
        self.merge(
            "MERGE (h:Host {hostname: $hostname}) \
             SET h.pending_updates = $total, h.security_updates = $sec, \
             h.pkg_manager = $pm, h.updates_checked = $now",
            &[
                ("hostname", ParamValue::Str(self.hostname.clone())),
                ("total", ParamValue::Int(total_pending as i64)),
                ("sec", ParamValue::Int(security_pending as i64)),
                ("pm", ParamValue::Str(pkg_manager)),
                ("now", ParamValue::Float(now)),
            ],
        )?;

        // Flag if security updates pending
        if security_pending > 0 {
            let diag_q = format!(
                "MERGE (e:HostLogEvent {{hostname: '{}', source: 'updates', template_hash: 'security-updates-pending'}}) \
                 SET e.severity = 'WARNING', e.template_text = '{} security update(s) pending ({} total). Run package updates to patch vulnerabilities.', \
                 e.count = {}, e.last_seen = {}",
                self.hostname, security_pending, total_pending, security_pending, now,
            );
            let _ = self.client.query(&diag_q, &[]);
        }

        Ok(total_pending)
    }

    // ------------------------------------------------------------------
    // 15. Hardware health (temps, SMART)
    // ------------------------------------------------------------------

    fn ingest_hardware_health(&self) -> Result<usize, String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut temps: Vec<(String, f64)> = Vec::new();
        let mut smart_status = String::new();
        let mut flagged = false;

        // Read CPU thermal zones: /sys/class/thermal/thermal_zone*/temp
        if let Ok(entries) = fs::read_dir("/sys/class/thermal") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with("thermal_zone") { continue; }

                let temp_path = entry.path().join("temp");
                let type_path = entry.path().join("type");

                let zone_type = fs::read_to_string(&type_path)
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                if let Ok(raw) = fs::read_to_string(&temp_path) {
                    if let Ok(millideg) = raw.trim().parse::<f64>() {
                        let celsius = millideg / 1000.0;
                        let label = if zone_type.is_empty() { name.clone() } else { zone_type };
                        temps.push((label, round1(celsius)));
                        if celsius > 80.0 {
                            flagged = true;
                        }
                    }
                }
            }
        }

        // Read hwmon sensors: /sys/class/hwmon/hwmon*/temp*_input
        if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
            for entry in entries.flatten() {
                let hwmon_path = entry.path();
                let hwmon_name = fs::read_to_string(hwmon_path.join("name"))
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                if let Ok(files) = fs::read_dir(&hwmon_path) {
                    for file in files.flatten() {
                        let fname = file.file_name().to_string_lossy().to_string();
                        if fname.ends_with("_input") && fname.starts_with("temp") {
                            if let Ok(raw) = fs::read_to_string(file.path()) {
                                if let Ok(millideg) = raw.trim().parse::<f64>() {
                                    let celsius = millideg / 1000.0;
                                    let label_path = file.path().to_string_lossy()
                                        .replace("_input", "_label");
                                    let label = fs::read_to_string(&label_path)
                                        .unwrap_or_else(|_| format!("{}:{}", hwmon_name, fname))
                                        .trim()
                                        .to_string();
                                    temps.push((label, round1(celsius)));
                                    if celsius > 80.0 {
                                        flagged = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Check SMART status if smartctl is available
        if which("smartctl").is_some() {
            // Try /dev/sda first, then /dev/nvme0
            for dev in &["/dev/sda", "/dev/nvme0n1"] {
                if !Path::new(dev).exists() { continue; }
                if let Ok(output) = Command::new("smartctl")
                    .args(["-H", dev])
                    .output()
                {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if stdout.contains("PASSED") {
                        smart_status = "PASSED".to_string();
                    } else if stdout.contains("FAILED") {
                        smart_status = "FAILED".to_string();
                        flagged = true;
                    }
                    if !smart_status.is_empty() { break; }
                }
            }
        }

        if temps.is_empty() && smart_status.is_empty() {
            return Ok(0);
        }

        // Build temp summary strings
        let temp_labels: Vec<String> = temps.iter().map(|(l, _)| l.clone()).collect();
        let temp_values: Vec<String> = temps.iter().map(|(_, v)| format!("{:.1}", v)).collect();
        let max_temp = temps.iter().map(|(_, v)| *v).fold(0.0f64, f64::max);

        self.merge(
            "MERGE (hw:HardwareHealth {hostname: $hostname}) \
             SET hw.temp_labels = $labels, hw.temp_values = $values, \
             hw.max_temp_celsius = $max_temp, hw.smart_status = $smart, \
             hw.flagged = $flagged, hw.last_seen = $now",
            &[
                ("hostname", ParamValue::Str(self.hostname.clone())),
                ("labels", ParamValue::StrList(temp_labels)),
                ("values", ParamValue::StrList(temp_values)),
                ("max_temp", ParamValue::Float(round1(max_temp))),
                ("smart", ParamValue::Str(smart_status.clone())),
                ("flagged", ParamValue::Bool(flagged)),
                ("now", ParamValue::Float(now)),
            ],
        )?;

        self.merge(
            "MATCH (h:Host {hostname: $hn}) \
             MATCH (hw:HardwareHealth {hostname: $hn}) \
             MERGE (h)-[:HAS_HARDWARE_HEALTH]->(hw)",
            &[("hn", ParamValue::Str(self.hostname.clone()))],
        )?;

        if flagged {
            let reason = if max_temp > 80.0 && smart_status == "FAILED" {
                format!("CPU temp {:.1}C exceeds 80C AND SMART status FAILED", max_temp)
            } else if max_temp > 80.0 {
                format!("CPU temp {:.1}C exceeds 80C threshold", max_temp)
            } else {
                "SMART disk health check FAILED".to_string()
            };
            let diag_q = format!(
                "MERGE (e:HostLogEvent {{hostname: '{}', source: 'hardware', template_hash: 'hw-health-flag'}}) \
                 SET e.severity = 'WARNING', e.template_text = '{}. Investigate hardware health immediately.', \
                 e.count = 1, e.last_seen = {}",
                self.hostname, reason, now,
            );
            let _ = self.client.query(&diag_q, &[]);
        }

        Ok(1)
    }

    // ------------------------------------------------------------------
    // 8. Journal errors
    // ------------------------------------------------------------------

    fn ingest_journal(&self) -> Result<usize, String> {
        let output = Command::new("journalctl")
            .args([
                "--no-pager",
                "-p",
                "err",
                "--since",
                "24 hours ago",
                "-o",
                "short-iso",
                "-n500",
            ])
            .output()
            .map_err(|e| e.to_string())?;

        if !output.status.success() {
            return Ok(0);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.trim().lines().collect();
        if lines.is_empty() {
            return Ok(0);
        }

        let re_unit = Regex::new(r"\S+\s+(\S+?)(?:\[\d+\])?:\s").unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut buckets: HashMap<String, JournalBucket> = HashMap::new();

        for line in &lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with("--") {
                continue;
            }

            let (tmpl, hash) = extract_template(line);

            let unit = re_unit
                .captures(line)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            let bucket = buckets.entry(hash.clone()).or_insert_with(|| JournalBucket {
                template_hash: hash,
                template_text: tmpl,
                unit,
                severity: "ERROR".to_string(),
                first_seen: now,
                last_seen: now,
                count: 0,
                examples: Vec::new(),
            });
            bucket.count += 1;
            bucket.last_seen = now;
            if bucket.examples.len() < 5 {
                bucket
                    .examples
                    .push(line.chars().take(300).collect());
            }
        }

        for b in buckets.values() {
            self.merge(
                "MERGE (e:HostLogEvent {hostname: $hostname, source: $source, \
                 template_hash: $th}) \
                 SET e.unit = $unit, e.severity = $severity, \
                 e.template_text = $tmpl, e.last_seen = $last_seen, \
                 e.count = $count, e.example_lines = $examples, \
                 e.first_seen = CASE WHEN e.first_seen IS NULL OR e.first_seen = 0 \
                                     THEN $first_seen ELSE e.first_seen END",
                &[
                    ("hostname", ParamValue::Str(self.hostname.clone())),
                    ("source", ParamValue::Str("journald".to_string())),
                    ("th", ParamValue::Str(b.template_hash.clone())),
                    ("unit", ParamValue::Str(b.unit.clone())),
                    ("severity", ParamValue::Str(b.severity.clone())),
                    ("tmpl", ParamValue::Str(b.template_text.clone())),
                    ("first_seen", ParamValue::Float(b.first_seen)),
                    ("last_seen", ParamValue::Float(b.last_seen)),
                    ("count", ParamValue::Int(b.count)),
                    ("examples", ParamValue::StrList(b.examples.clone())),
                ],
            )?;

            self.merge(
                "MATCH (h:Host {hostname: $hn}) \
                 MATCH (e:HostLogEvent {hostname: $hn, template_hash: $th}) \
                 MERGE (h)-[:EMITTED]->(e)",
                &[
                    ("hn", ParamValue::Str(self.hostname.clone())),
                    ("th", ParamValue::Str(b.template_hash.clone())),
                ],
            )?;
        }

        Ok(buckets.len())
    }
}

// ---------------------------------------------------------------------------
// Internal data structures
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ProcInfo {
    pid: i64,
    name: String,
    cmdline: String,
    cpu_ticks: i64,
    mem_mb: f64,
    user: String,
    status: String,
}

struct KernelBucket {
    template_hash: String,
    template_text: String,
    category: String,
    severity: String,
    first_seen: f64,
    last_seen: f64,
    count: i64,
    examples: Vec<String>,
}

struct JournalBucket {
    template_hash: String,
    template_text: String,
    unit: String,
    severity: String,
    first_seen: f64,
    last_seen: f64,
    count: i64,
    examples: Vec<String>,
}

struct DiskPartition {
    device: String,
    mountpoint: String,
    fstype: String,
}

struct DiskUsage {
    total: u64,
    used: u64,
    free: u64,
}

struct MemInfo {
    total_mb: i64,
    used_mb: i64,
    percent: f64,
    swap_total_mb: i64,
    swap_used_mb: i64,
}

// ---------------------------------------------------------------------------
// /proc readers
// ---------------------------------------------------------------------------

fn uname_info() -> (String, String) {
    let system = "Linux".to_string();
    let release = fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_string();
    (system, release)
}

fn read_uptime() -> io::Result<i64> {
    let content = fs::read_to_string("/proc/uptime")?;
    let secs: f64 = content
        .split_whitespace()
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0.0);
    Ok(secs as i64)
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn read_cpu_percent() -> io::Result<f64> {
    let content = fs::read_to_string("/proc/stat")?;
    let line = content.lines().next().unwrap_or("");
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 5 {
        return Ok(0.0);
    }
    let idle: u64 = parts[4].parse().unwrap_or(0);
    let total: u64 = parts[1..].iter().filter_map(|s| s.parse::<u64>().ok()).sum();
    if total == 0 {
        return Ok(0.0);
    }
    Ok(round1(100.0 * (1.0 - idle as f64 / total as f64)))
}

fn read_meminfo() -> MemInfo {
    let content = match fs::read_to_string("/proc/meminfo") {
        Ok(c) => c,
        Err(_) => {
            return MemInfo {
                total_mb: 0,
                used_mb: 0,
                percent: 0.0,
                swap_total_mb: 0,
                swap_used_mb: 0,
            }
        }
    };

    let mut info: HashMap<String, i64> = HashMap::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let key = parts[0].trim_end_matches(':').to_string();
            let val: i64 = parts[1].parse().unwrap_or(0);
            info.insert(key, val);
        }
    }

    let total = info.get("MemTotal").copied().unwrap_or(0) / 1024;
    let avail = info
        .get("MemAvailable")
        .or_else(|| info.get("MemFree"))
        .copied()
        .unwrap_or(0)
        / 1024;
    let used = total - avail;
    let swap_total = info.get("SwapTotal").copied().unwrap_or(0) / 1024;
    let swap_free = info.get("SwapFree").copied().unwrap_or(0) / 1024;

    MemInfo {
        total_mb: total,
        used_mb: used,
        percent: if total > 0 {
            round1(100.0 * used as f64 / total as f64)
        } else {
            0.0
        },
        swap_total_mb: swap_total,
        swap_used_mb: swap_total - swap_free,
    }
}

fn read_loadavg() -> io::Result<(f64, f64, f64)> {
    let content = fs::read_to_string("/proc/loadavg")?;
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() < 3 {
        return Ok((0.0, 0.0, 0.0));
    }
    Ok((
        round2(parts[0].parse().unwrap_or(0.0)),
        round2(parts[1].parse().unwrap_or(0.0)),
        round2(parts[2].parse().unwrap_or(0.0)),
    ))
}

fn get_disk_partitions() -> Vec<DiskPartition> {
    let skip_fs: HashSet<&str> = [
        "proc", "sysfs", "devtmpfs", "devpts", "tmpfs", "securityfs", "cgroup", "cgroup2",
        "pstore", "debugfs", "hugetlbfs", "mqueue", "fusectl", "configfs", "binfmt_misc",
        "autofs", "tracefs", "nsfs", "bpf", "efivarfs", "ramfs",
    ]
    .iter()
    .copied()
    .collect();

    let content = match fs::read_to_string("/proc/mounts") {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut partitions = Vec::new();
    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let device = parts[0];
        let mountpoint = parts[1];
        let fstype = parts[2];
        if skip_fs.contains(fstype) {
            continue;
        }
        if mountpoint.starts_with("/snap/") {
            continue;
        }
        partitions.push(DiskPartition {
            device: device.to_string(),
            mountpoint: mountpoint.to_string(),
            fstype: fstype.to_string(),
        });
    }
    partitions
}

fn disk_usage(mountpoint: &str) -> Result<DiskUsage, String> {
    let stat = statvfs(mountpoint).map_err(|e| e.to_string())?;
    let block_size = stat.block_size() as u64;
    let total = stat.blocks() as u64 * block_size;
    let free = stat.blocks_available() as u64 * block_size;
    let used = total.saturating_sub(free);
    Ok(DiskUsage { total, used, free })
}

fn read_proc_user(pid_path: &Path) -> String {
    let status = match fs::read_to_string(pid_path.join("status")) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    for line in status.lines() {
        if line.starts_with("Uid:") {
            let uid_str = line.split_whitespace().nth(1).unwrap_or("0");
            let uid: u32 = uid_str.parse().unwrap_or(0);
            // Try to resolve uid to username via /etc/passwd
            return resolve_uid(uid);
        }
    }
    String::new()
}

fn resolve_uid(uid: u32) -> String {
    let passwd = match fs::read_to_string("/etc/passwd") {
        Ok(s) => s,
        Err(_) => return uid.to_string(),
    };
    for line in passwd.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() >= 3 {
            if let Ok(u) = parts[2].parse::<u32>() {
                if u == uid {
                    return parts[0].to_string();
                }
            }
        }
    }
    uid.to_string()
}

fn which(binary: &str) -> Option<String> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let full = format!("{}/{}", dir, binary);
        if Path::new(&full).exists() {
            return Some(full);
        }
    }
    None
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn get_wifi_info(iface: &str) -> (String, String, String) {
    // Try nmcli for band/channel/ssid
    if let Ok(output) = Command::new("nmcli")
        .args(["-t", "-f", "ACTIVE,SSID,CHAN,BAND", "dev", "wifi"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.starts_with("yes:") {
                let parts: Vec<&str> = line.split(':').collect();
                let ssid = parts.get(1).unwrap_or(&"").to_string();
                let chan = parts.get(2).unwrap_or(&"").to_string();
                let band = if chan.parse::<u32>().unwrap_or(0) > 14 { "5GHz" } else { "2.4GHz" };
                return (band.to_string(), chan, ssid);
            }
        }
    }
    ("unknown".into(), "unknown".into(), "unknown".into())
}

// ---------------------------------------------------------------------------
// Public entry point for `savants host snapshot`
// ---------------------------------------------------------------------------

pub fn run_snapshot() {
    println!(
        "{}",
        "Collecting host state...".cyan()
    );

    let state = crate::config::State::load();
    let graph_name = state.graph_name();

    // Try with graph DB first, fall back to standalone mode
    match GraphClient::new(&graph_name) {
        Ok(client) if client.is_connected() => {
            let ingestor = HostIngestor::new(client, None, 20);
            let stats = ingestor.snapshot();
            println!("{}", stats.summary());
            if !stats.errors.is_empty() {
                eprintln!("\n{}:", "Errors".red());
                for e in &stats.errors {
                    eprintln!("  - {}", e);
                }
            }
        }
        _ => {
            // Standalone mode: collect and print without graph DB
            run_snapshot_standalone();
        }
    }
}

/// Collect and display host information without requiring a graph database.
/// This is the default for developers who just installed the CLI.
fn run_snapshot_standalone() {
    use std::process::Command;

    println!();

    // Hostname + OS
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".into());
    println!("  {} {}", "Host:".bold(), hostname);

    if let Ok(os) = std::fs::read_to_string("/etc/os-release") {
        if let Some(pretty) = os.lines().find(|l| l.starts_with("PRETTY_NAME=")) {
            let name = pretty.trim_start_matches("PRETTY_NAME=").trim_matches('"');
            println!("  {} {}", "OS:".bold(), name);
        }
    }

    // Uptime
    if let Ok(uptime) = std::fs::read_to_string("/proc/uptime") {
        if let Some(secs) = uptime.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()) {
            let days = (secs / 86400.0) as u64;
            let hours = ((secs % 86400.0) / 3600.0) as u64;
            println!("  {} {}d {}h", "Uptime:".bold(), days, hours);
        }
    }

    // CPU
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        let cores = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
        let model = cpuinfo.lines()
            .find(|l| l.starts_with("model name"))
            .and_then(|l| l.split(':').nth(1))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".into());
        println!("  {} {} ({} cores)", "CPU:".bold(), model, cores);
    }

    // Memory
    if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
        let get = |key: &str| -> Option<u64> {
            meminfo.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
        };
        if let (Some(total), Some(avail)) = (get("MemTotal:"), get("MemAvailable:")) {
            let used = total - avail;
            println!("  {} {:.1}G / {:.1}G ({:.0}% used)", "Memory:".bold(),
                used as f64 / 1048576.0, total as f64 / 1048576.0,
                (used as f64 / total as f64) * 100.0);
        }
    }

    // Disks
    if let Ok(output) = Command::new("df").args(["-h", "--output=target,size,used,avail,pcent"]).output() {
        let text = String::from_utf8_lossy(&output.stdout);
        let disks: Vec<&str> = text.lines()
            .filter(|l| l.starts_with('/') && !l.contains("/snap/") && !l.contains("/nix/store"))
            .collect();
        if !disks.is_empty() {
            println!("  {} {} mount(s)", "Disks:".bold(), disks.len());
            for d in &disks {
                println!("    {}", d.trim());
            }
        }
    }

    // Docker
    if let Ok(output) = Command::new("docker").args(["ps", "--format", "{{.Names}}: {{.Status}}"]).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let containers: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
            println!("  {} {} running", "Docker:".bold(), containers.len());
        }
    }

    // Systemd failed units
    if let Ok(output) = Command::new("systemctl").args(["--failed", "--no-pager", "--plain", "--no-legend"]).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            let failed: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
            if failed.is_empty() {
                println!("  {} no failed units", "Systemd:".bold());
            } else {
                println!("  {} {} failed unit(s)", "Systemd:".bold().red(), failed.len());
                for f in &failed {
                    println!("    {}", f.trim());
                }
            }
        }
    }

    // Load average
    if let Ok(loadavg) = std::fs::read_to_string("/proc/loadavg") {
        let parts: Vec<&str> = loadavg.split_whitespace().collect();
        if parts.len() >= 3 {
            println!("  {} {} {} {}", "Load:".bold(), parts[0], parts[1], parts[2]);
        }
    }

    println!();
    println!("  {}", "Run 'savants connect' for persistent monitoring + cloud dashboard.".dimmed());
}
