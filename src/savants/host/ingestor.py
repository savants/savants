"""Host agent ingestor for the Savants runtime layer.

Reads the state of the local machine via /proc, systemd, dmesg, and
optionally Docker, then writes it to FalkorDB. Same pattern as the
K8s ingestor: snapshot + diff + watch. Runs as a background daemon
alongside the K8s watcher.

This is the missing layer between "cluster is fine" and "the node
itself is sick" — disk full, OOM kills, kernel panics, zombie procs,
failed systemd units, Docker container crashes.

Zero external dependencies: reads /proc and calls subprocess for
systemd/dmesg/docker. Works on any Linux host. macOS support is
future work (different /proc structure).

Usage:

    from savants.host.ingestor import HostIngestor
    from savants.graph.client import GraphClient
    from savants.config import FalkorDBConfig

    client = GraphClient(FalkorDBConfig(graph_name="astra"))
    ingestor = HostIngestor(graph_client=client)
    stats = ingestor.snapshot()
    print(stats)
"""

from __future__ import annotations

import logging
import os
import platform
import re
import shutil
import socket
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from savants.graph.client import GraphClient
from savants.graph.schema import (
    DockerContainerNode,
    HostDiskNode,
    HostLogEventNode,
    HostNetIfaceNode,
    HostNode,
    HostProcessNode,
    KernelEventNode,
    SystemdUnitNode,
    create_docker_container_query,
    create_host_disk_query,
    create_host_log_event_query,
    create_host_net_iface_query,
    create_host_process_query,
    create_host_query,
    create_kernel_event_query,
    create_systemd_unit_query,
)

logger = logging.getLogger(__name__)


@dataclass
class HostIngestStats:
    hostname: str = ""
    elapsed_seconds: float = 0.0
    disks: int = 0
    interfaces: int = 0
    processes: int = 0
    systemd_units: int = 0
    failed_units: int = 0
    docker_containers: int = 0
    kernel_events: int = 0
    journal_events: int = 0
    errors: list[str] = field(default_factory=list)

    def summary(self) -> str:
        lines = [
            f"Host ingest for '{self.hostname}' in {self.elapsed_seconds:.1f}s",
            f"  Disks:        {self.disks}",
            f"  Interfaces:   {self.interfaces}",
            f"  Processes:    {self.processes} (top by CPU/mem)",
            f"  Systemd:      {self.systemd_units} units ({self.failed_units} failed)",
            f"  Docker:       {self.docker_containers} containers",
            f"  Kernel:       {self.kernel_events} events",
            f"  Journal:      {self.journal_events} events",
        ]
        if self.errors:
            lines.append(f"  Errors:       {len(self.errors)}")
        return "\n".join(lines)


class HostIngestor:
    """Reads local machine state and writes it to FalkorDB."""

    def __init__(
        self,
        graph_client: GraphClient,
        hostname: str | None = None,
        top_n_processes: int = 20,
        dmesg_lines: int = 500,
        journal_lines: int = 500,
    ):
        self.client = graph_client
        self.hostname = hostname or socket.gethostname()
        self.top_n = top_n_processes
        self.dmesg_lines = dmesg_lines
        self.journal_lines = journal_lines

    def snapshot(self) -> HostIngestStats:
        """One-shot snapshot of the local host into the graph."""
        t0 = time.time()
        stats = HostIngestStats(hostname=self.hostname)

        # 1. Host node (top-level)
        self._ingest_host()

        # 2. Disks
        try:
            stats.disks = self._ingest_disks()
        except Exception as e:
            stats.errors.append(f"disks: {e}")

        # 3. Network interfaces
        try:
            stats.interfaces = self._ingest_interfaces()
        except Exception as e:
            stats.errors.append(f"interfaces: {e}")

        # 4. Top processes
        try:
            stats.processes = self._ingest_processes()
        except Exception as e:
            stats.errors.append(f"processes: {e}")

        # 5. Systemd units
        try:
            total, failed = self._ingest_systemd()
            stats.systemd_units = total
            stats.failed_units = failed
        except Exception as e:
            stats.errors.append(f"systemd: {e}")

        # 6. Docker containers (if available)
        try:
            stats.docker_containers = self._ingest_docker()
        except Exception as e:
            stats.errors.append(f"docker: {e}")

        # 7. Kernel events (dmesg)
        try:
            stats.kernel_events = self._ingest_dmesg()
        except Exception as e:
            stats.errors.append(f"dmesg: {e}")

        # 8. Journal errors
        try:
            stats.journal_events = self._ingest_journal()
        except Exception as e:
            stats.errors.append(f"journal: {e}")

        stats.elapsed_seconds = time.time() - t0
        logger.info(stats.summary())
        return stats

    # ------------------------------------------------------------------
    # Ingest helpers
    # ------------------------------------------------------------------

    def _merge(self, cypher_and_params: tuple[str, dict]) -> None:
        cypher, params = cypher_and_params
        self.client.query(cypher, params)

    def _ingest_host(self) -> None:
        uname = platform.uname()
        uptime = 0
        try:
            with open("/proc/uptime") as f:
                uptime = int(float(f.read().split()[0]))
        except Exception:
            pass

        cpu_count = os.cpu_count() or 0
        cpu_percent = 0.0
        try:
            # /proc/stat cpu line → quick idle% estimate
            with open("/proc/stat") as f:
                line = f.readline()
            parts = line.split()
            idle = int(parts[4])
            total = sum(int(x) for x in parts[1:])
            cpu_percent = round(100.0 * (1 - idle / max(total, 1)), 1)
        except Exception:
            pass

        mem = self._read_meminfo()
        load = (0.0, 0.0, 0.0)
        try:
            load = os.getloadavg()
        except Exception:
            pass

        node = HostNode(
            hostname=self.hostname,
            os=f"{uname.system} {uname.release}",
            kernel=uname.release,
            uptime_seconds=uptime,
            cpu_count=cpu_count,
            cpu_percent=cpu_percent,
            memory_total_mb=mem.get("total", 0),
            memory_used_mb=mem.get("used", 0),
            memory_percent=mem.get("percent", 0.0),
            swap_total_mb=mem.get("swap_total", 0),
            swap_used_mb=mem.get("swap_used", 0),
            load_1m=round(load[0], 2),
            load_5m=round(load[1], 2),
            load_15m=round(load[2], 2),
        )
        self._merge(create_host_query(node))

    def _ingest_disks(self) -> int:
        n = 0
        for part in self._get_disk_partitions():
            try:
                usage = shutil.disk_usage(part["mountpoint"])
                node = HostDiskNode(
                    hostname=self.hostname,
                    mountpoint=part["mountpoint"],
                    device=part["device"],
                    fstype=part["fstype"],
                    total_gb=round(usage.total / 1e9, 2),
                    used_gb=round(usage.used / 1e9, 2),
                    free_gb=round(usage.free / 1e9, 2),
                    percent=round(100.0 * usage.used / max(usage.total, 1), 1),
                )
                self._merge(create_host_disk_query(node))
                self.client.query(
                    "MATCH (h:Host {hostname: $hn}) "
                    "MATCH (d:HostDisk {hostname: $hn, mountpoint: $mp}) "
                    "MERGE (h)-[:HAS_DISK]->(d)",
                    {"hn": self.hostname, "mp": part["mountpoint"]},
                )
                n += 1
            except (PermissionError, OSError):
                continue
        return n

    def _ingest_interfaces(self) -> int:
        n = 0
        try:
            out = subprocess.run(
                ["ip", "-j", "addr", "show"],
                capture_output=True, text=True, timeout=5,
            )
            if out.returncode != 0:
                return 0
            import json
            ifaces = json.loads(out.stdout)
        except Exception:
            return 0

        for iface in ifaces:
            name = iface.get("ifname", "")
            if not name or name == "lo":
                continue
            state = iface.get("operstate", "UNKNOWN").lower()
            ipv4 = ""
            ipv6 = ""
            mac = iface.get("address", "")
            mtu = iface.get("mtu", 0)
            for addr in iface.get("addr_info", []):
                if addr.get("family") == "inet" and not ipv4:
                    ipv4 = addr.get("local", "")
                elif addr.get("family") == "inet6" and not ipv6:
                    ipv6 = addr.get("local", "")

            node = HostNetIfaceNode(
                hostname=self.hostname,
                name=name,
                ipv4=ipv4,
                ipv6=ipv6,
                mac=mac,
                state=state,
                mtu=mtu,
            )
            self._merge(create_host_net_iface_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (n:HostNetIface {hostname: $hn, name: $name}) "
                "MERGE (h)-[:HAS_IFACE]->(n)",
                {"hn": self.hostname, "name": name},
            )
            n += 1
        return n

    def _ingest_processes(self) -> int:
        """Top N processes by CPU + memory from /proc."""
        procs = []
        try:
            for pid_dir in Path("/proc").iterdir():
                if not pid_dir.name.isdigit():
                    continue
                try:
                    pid = int(pid_dir.name)
                    stat = (pid_dir / "stat").read_text().split()
                    name = stat[1].strip("()")
                    status_map = {"R": "running", "S": "sleeping", "D": "disk-sleep",
                                  "Z": "zombie", "T": "stopped", "I": "idle"}
                    status = status_map.get(stat[2], stat[2])
                    # RSS in pages → MB
                    rss_pages = int(stat[23])
                    mem_mb = round(rss_pages * 4096 / 1e6, 1)
                    utime = int(stat[13])
                    stime = int(stat[14])
                    cpu_ticks = utime + stime
                    # Read cmdline
                    try:
                        cmdline = (pid_dir / "cmdline").read_text().replace("\0", " ").strip()[:200]
                    except Exception:
                        cmdline = name
                    # Read user
                    try:
                        import pwd
                        uid = (pid_dir / "status").read_text()
                        uid_line = [l for l in uid.splitlines() if l.startswith("Uid:")][0]
                        real_uid = int(uid_line.split()[1])
                        user = pwd.getpwuid(real_uid).pw_name
                    except Exception:
                        user = ""
                    procs.append({
                        "pid": pid, "name": name, "cmdline": cmdline,
                        "cpu_ticks": cpu_ticks, "mem_mb": mem_mb,
                        "user": user, "status": status,
                    })
                except (PermissionError, FileNotFoundError, IndexError, ValueError):
                    continue
        except Exception:
            return 0

        # Take top N by memory + top N by cpu_ticks, deduplicate
        by_mem = sorted(procs, key=lambda p: -p["mem_mb"])[:self.top_n]
        by_cpu = sorted(procs, key=lambda p: -p["cpu_ticks"])[:self.top_n]
        seen = set()
        top = []
        for p in by_mem + by_cpu:
            if p["pid"] not in seen:
                seen.add(p["pid"])
                top.append(p)

        # Clear old process nodes for this host
        self.client.query(
            "MATCH (p:HostProcess {hostname: $hn}) DETACH DELETE p",
            {"hn": self.hostname},
        )

        for p in top:
            node = HostProcessNode(
                hostname=self.hostname,
                pid=p["pid"],
                name=p["name"],
                cmdline=p["cmdline"],
                cpu_percent=0.0,  # ticks, not %; good enough for ranking
                memory_mb=p["mem_mb"],
                user=p["user"],
                status=p["status"],
            )
            self._merge(create_host_process_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (p:HostProcess {hostname: $hn, pid: $pid}) "
                "MERGE (h)-[:RUNS]->(p)",
                {"hn": self.hostname, "pid": p["pid"]},
            )
        return len(top)

    def _ingest_systemd(self) -> tuple[int, int]:
        """Read systemd unit states via `systemctl list-units --all`."""
        try:
            out = subprocess.run(
                ["systemctl", "list-units", "--all", "--no-pager",
                 "--plain", "--no-legend", "--output=json"],
                capture_output=True, text=True, timeout=10,
            )
            if out.returncode != 0:
                # Fallback: try without --output=json
                return self._ingest_systemd_text()
            import json
            units = json.loads(out.stdout)
        except Exception:
            return self._ingest_systemd_text()

        total = 0
        failed = 0
        for u in units:
            name = u.get("unit", "")
            if not name:
                continue
            # Only track services, timers, mounts — skip slices, scopes, etc.
            unit_type = name.rsplit(".", 1)[-1] if "." in name else ""
            if unit_type not in ("service", "timer", "mount", "socket"):
                continue

            active = u.get("active", "")
            sub = u.get("sub", "")
            desc = u.get("description", "")

            node = SystemdUnitNode(
                hostname=self.hostname,
                name=name,
                type=unit_type,
                active_state=active,
                sub_state=sub,
                description=desc,
            )
            self._merge(create_systemd_unit_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (u:SystemdUnit {hostname: $hn, name: $name}) "
                "MERGE (h)-[:HAS_UNIT]->(u)",
                {"hn": self.hostname, "name": name},
            )
            total += 1
            if active == "failed":
                failed += 1
        return total, failed

    def _ingest_systemd_text(self) -> tuple[int, int]:
        """Fallback systemd parser for older systemctl without JSON output."""
        try:
            out = subprocess.run(
                ["systemctl", "list-units", "--all", "--no-pager",
                 "--plain", "--no-legend"],
                capture_output=True, text=True, timeout=10,
            )
        except Exception:
            return 0, 0

        total = 0
        failed = 0
        for line in out.stdout.splitlines():
            parts = line.split(None, 4)
            if len(parts) < 4:
                continue
            name, _, active, sub = parts[0], parts[1], parts[2], parts[3]
            unit_type = name.rsplit(".", 1)[-1] if "." in name else ""
            if unit_type not in ("service", "timer", "mount", "socket"):
                continue
            desc = parts[4] if len(parts) > 4 else ""
            node = SystemdUnitNode(
                hostname=self.hostname, name=name, type=unit_type,
                active_state=active, sub_state=sub, description=desc,
            )
            self._merge(create_systemd_unit_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (u:SystemdUnit {hostname: $hn, name: $name}) "
                "MERGE (h)-[:HAS_UNIT]->(u)",
                {"hn": self.hostname, "name": name},
            )
            total += 1
            if active == "failed":
                failed += 1
        return total, failed

    def _ingest_docker(self) -> int:
        """Read Docker containers if docker CLI is available."""
        if not shutil.which("docker"):
            return 0
        try:
            out = subprocess.run(
                ["docker", "ps", "-a", "--format",
                 "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}\t{{.CreatedAt}}"],
                capture_output=True, text=True, timeout=10,
            )
            if out.returncode != 0:
                return 0
        except Exception:
            return 0

        n = 0
        for line in out.stdout.strip().splitlines():
            parts = line.split("\t")
            if len(parts) < 4:
                continue
            cid = parts[0][:12]
            name = parts[1] if len(parts) > 1 else ""
            image = parts[2] if len(parts) > 2 else ""
            status = parts[3] if len(parts) > 3 else ""
            ports = parts[4].split(", ") if len(parts) > 4 and parts[4] else []
            created = parts[5] if len(parts) > 5 else ""

            # Derive a simple state
            state = "running" if "Up" in status else "exited"

            # Parse restart count from status if present
            rc = 0
            rc_match = re.search(r"Restarting \((\d+)\)", status)
            if rc_match:
                rc = int(rc_match.group(1))
                state = "restarting"

            node = DockerContainerNode(
                hostname=self.hostname,
                container_id=cid,
                name=name,
                image=image,
                status=status,
                state=state,
                ports=ports,
                created_at=created,
                restart_count=rc,
            )
            self._merge(create_docker_container_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (c:DockerContainer {hostname: $hn, container_id: $cid}) "
                "MERGE (h)-[:RUNS_CONTAINER]->(c)",
                {"hn": self.hostname, "cid": cid},
            )
            n += 1
        return n

    def _ingest_dmesg(self) -> int:
        """Read kernel ring buffer for significant events."""
        try:
            out = subprocess.run(
                ["dmesg", "--time-format=iso", "-l", "err,warn,crit,alert,emerg",
                 f"--read-clear=false"],
                capture_output=True, text=True, timeout=10,
            )
            if out.returncode != 0:
                # Try without --time-format (older kernels)
                out = subprocess.run(
                    ["dmesg", "-l", "err,warn,crit,alert,emerg"],
                    capture_output=True, text=True, timeout=10,
                )
        except Exception:
            return 0

        lines = out.stdout.strip().splitlines()[-self.dmesg_lines:]
        return self._ingest_kernel_lines(lines)

    def _ingest_kernel_lines(self, lines: list[str]) -> int:
        """Process dmesg lines through drain3 and write KernelEvent nodes."""
        if not lines:
            return 0

        from drain3 import TemplateMiner
        from drain3.template_miner_config import TemplateMinerConfig
        from savants.k8s.log_watcher import classify_line, extract_log_timestamp

        cfg = TemplateMinerConfig()
        cfg.drain_max_clusters = 200
        miner = TemplateMiner(config=cfg)
        now = time.time()

        # Category detection patterns
        categories = [
            (re.compile(r"Out of memory|oom-kill|oom_reaper|invoked oom-killer", re.I), "oom"),
            (re.compile(r"I/O error|Buffer I/O error|blk_update_request", re.I), "io_error"),
            (re.compile(r"segfault|general protection fault", re.I), "segfault"),
            (re.compile(r"Hardware Error|MCE|mce:|GHES", re.I), "hardware"),
            (re.compile(r"nfs|NFSD|rpc_task", re.I), "nfs"),
            (re.compile(r"Kernel panic|BUG:", re.I), "panic"),
        ]

        buckets: dict[str, dict] = {}
        for line in lines:
            line = line.strip()
            if not line:
                continue
            result = miner.add_log_message(line)
            if not result:
                continue
            cid = str(result["cluster_id"])
            tmpl = result.get("template_mined", "")

            # Detect category
            cat = "other"
            for pat, label in categories:
                if pat.search(line):
                    cat = label
                    break

            # Detect severity
            sev = "WARN"
            if any(kw in line.lower() for kw in ("emerg", "panic", "bug:", "oops")):
                sev = "FATAL"
            elif any(kw in line.lower() for kw in ("err", "crit", "alert", "error")):
                sev = "ERROR"

            ts = extract_log_timestamp(line) or now

            if cid not in buckets:
                buckets[cid] = {
                    "template_hash": cid, "template_text": tmpl,
                    "category": cat, "severity": sev,
                    "first_seen": ts, "last_seen": ts,
                    "count": 0, "examples": [],
                }
            b = buckets[cid]
            b["count"] += 1
            b["last_seen"] = max(b["last_seen"], ts)
            if len(b["examples"]) < 5:
                b["examples"].append(line[:300])

        for b in buckets.values():
            node = KernelEventNode(
                hostname=self.hostname,
                template_hash=b["template_hash"],
                category=b["category"],
                severity=b["severity"],
                template_text=b["template_text"],
                first_seen=b["first_seen"],
                last_seen=b["last_seen"],
                count=b["count"],
                example_lines=b["examples"],
            )
            self._merge(create_kernel_event_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (e:KernelEvent {hostname: $hn, template_hash: $th}) "
                "MERGE (h)-[:EMITTED]->(e)",
                {"hn": self.hostname, "th": b["template_hash"]},
            )
        return len(buckets)

    def _ingest_journal(self) -> int:
        """Read recent journald errors and process through drain3."""
        try:
            out = subprocess.run(
                ["journalctl", "--no-pager", "-p", "err",
                 "--since", "24 hours ago", "-o", "short-iso",
                 f"-n{self.journal_lines}"],
                capture_output=True, text=True, timeout=15,
            )
            if out.returncode != 0:
                return 0
        except Exception:
            return 0

        lines = out.stdout.strip().splitlines()
        if not lines:
            return 0

        from drain3 import TemplateMiner
        from drain3.template_miner_config import TemplateMinerConfig
        from savants.k8s.log_watcher import classify_line, extract_log_timestamp

        cfg = TemplateMinerConfig()
        cfg.drain_max_clusters = 200
        miner = TemplateMiner(config=cfg)
        now = time.time()

        buckets: dict[str, dict] = {}
        for line in lines:
            line = line.strip()
            if not line or line.startswith("--"):
                continue
            result = miner.add_log_message(line)
            if not result:
                continue
            cid = str(result["cluster_id"])
            tmpl = result.get("template_mined", "")

            # Extract unit name from journal format: "hostname unitname[pid]: message"
            unit = ""
            unit_match = re.search(r"\S+\s+(\S+?)(?:\[\d+\])?:\s", line)
            if unit_match:
                unit = unit_match.group(1)

            ts = extract_log_timestamp(line) or now

            if cid not in buckets:
                buckets[cid] = {
                    "template_hash": cid, "template_text": tmpl,
                    "unit": unit, "severity": "ERROR",
                    "first_seen": ts, "last_seen": ts,
                    "count": 0, "examples": [],
                }
            b = buckets[cid]
            b["count"] += 1
            b["last_seen"] = max(b["last_seen"], ts)
            if len(b["examples"]) < 5:
                b["examples"].append(line[:300])

        for b in buckets.values():
            node = HostLogEventNode(
                hostname=self.hostname,
                template_hash=b["template_hash"],
                source="journald",
                unit=b["unit"],
                severity=b["severity"],
                template_text=b["template_text"],
                first_seen=b["first_seen"],
                last_seen=b["last_seen"],
                count=b["count"],
                example_lines=b["examples"],
            )
            self._merge(create_host_log_event_query(node))
            self.client.query(
                "MATCH (h:Host {hostname: $hn}) "
                "MATCH (e:HostLogEvent {hostname: $hn, template_hash: $th}) "
                "MERGE (h)-[:EMITTED]->(e)",
                {"hn": self.hostname, "th": b["template_hash"]},
            )
        return len(buckets)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    @staticmethod
    def _read_meminfo() -> dict:
        try:
            with open("/proc/meminfo") as f:
                info = {}
                for line in f:
                    parts = line.split()
                    key = parts[0].rstrip(":")
                    val = int(parts[1])  # kB
                    info[key] = val
            total = info.get("MemTotal", 0) // 1024
            free = info.get("MemAvailable", info.get("MemFree", 0)) // 1024
            used = total - free
            swap_total = info.get("SwapTotal", 0) // 1024
            swap_free = info.get("SwapFree", 0) // 1024
            return {
                "total": total, "used": used,
                "percent": round(100.0 * used / max(total, 1), 1),
                "swap_total": swap_total,
                "swap_used": swap_total - swap_free,
            }
        except Exception:
            return {}

    @staticmethod
    def _get_disk_partitions() -> list[dict]:
        """Read /proc/mounts for real filesystems."""
        partitions = []
        skip_fs = {"proc", "sysfs", "devtmpfs", "devpts", "tmpfs", "securityfs",
                    "cgroup", "cgroup2", "pstore", "debugfs", "hugetlbfs",
                    "mqueue", "fusectl", "configfs", "binfmt_misc", "autofs",
                    "tracefs", "nsfs", "bpf", "efivarfs", "ramfs"}
        try:
            with open("/proc/mounts") as f:
                for line in f:
                    parts = line.split()
                    if len(parts) < 3:
                        continue
                    device, mountpoint, fstype = parts[0], parts[1], parts[2]
                    if fstype in skip_fs:
                        continue
                    if mountpoint.startswith("/snap/"):
                        continue
                    partitions.append({
                        "device": device,
                        "mountpoint": mountpoint,
                        "fstype": fstype,
                    })
        except Exception:
            pass
        return partitions
