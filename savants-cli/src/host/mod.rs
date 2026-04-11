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
    let total = stat.blocks() * block_size;
    let free = stat.blocks_available() * block_size;
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

    let client = match GraphClient::new(&graph_name) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "{}: Could not connect to Savants memory: {}",
                "Error".red(),
                e
            );
            std::process::exit(1);
        }
    };

    if !client.is_connected() {
        eprintln!(
            "{}: Savants memory is not reachable at {}:{}",
            "Error".red(),
            state.graph_host(),
            state.graph_port()
        );
        std::process::exit(1);
    }

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
