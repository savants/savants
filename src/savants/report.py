"""Generate a shareable markdown diagnosis report.

Queries all layers of the Savants graph (host, K8s clusters, log events,
code) and produces a single markdown document suitable for pasting into
Slack, GitHub issues, or blog posts. This is the "content flywheel"
feature — every user who runs `savants report` generates a story that
can be shared, which markets the product organically.

Usage:
    from savants.report import generate_report
    md = generate_report(graph_client)
    print(md)
"""

from __future__ import annotations

import time
import datetime as dt
from savants.graph.client import GraphClient


def generate_report(
    client: GraphClient,
    cluster_clients: dict[str, GraphClient] | None = None,
    since_minutes: int = 60,
    min_severity: str = "WARN",
) -> str:
    """Generate a full markdown diagnosis report."""
    now = time.time()
    since = now - (since_minutes * 60) if since_minutes > 0 else 0
    lines: list[str] = []
    ts = dt.datetime.now().strftime("%Y-%m-%d %H:%M")

    lines.append(f"# Savants Infrastructure Report")
    lines.append(f"*Generated {ts} | window: {'last ' + str(since_minutes) + 'm' if since_minutes else 'all time'} | min severity: {min_severity}*")
    lines.append("")

    # ── Host ──
    r = client.query("MATCH (h:Host) RETURN h.hostname, h.os, h.cpu_count, "
                     "h.cpu_percent, h.memory_total_mb, h.memory_used_mb, "
                     "h.memory_percent, h.load_1m, h.load_5m, h.load_15m, "
                     "h.uptime_seconds", {})
    for row in (r.result_set or []):
        hn, os_, cpus, cpu_pct, mem_t, mem_u, mem_pct, l1, l5, l15, up = row
        days = int(up or 0) // 86400
        lines.append(f"## Host: {hn}")
        lines.append(f"- **OS:** {os_} | **Uptime:** {days}d | **CPU:** {cpu_pct}% ({cpus} cores) | **Load:** {l1}/{l5}/{l15}")
        lines.append(f"- **Memory:** {int(mem_u or 0)}/{int(mem_t or 0)} MB ({mem_pct}%)")
        lines.append("")

        # Disks
        dr = client.query(
            "MATCH (d:HostDisk {hostname: $hn}) RETURN d.mountpoint, d.device, "
            "d.total_gb, d.used_gb, d.percent ORDER BY d.percent DESC",
            {"hn": hn})
        if dr.result_set:
            lines.append("### Disks")
            lines.append("| Mount | Device | Used | Total | % |")
            lines.append("|---|---|---|---|---|")
            for dr_row in dr.result_set:
                mp, dev, total, used, pct = dr_row
                flag = " **!!**" if (pct or 0) > 85 else ""
                lines.append(f"| {mp} | {dev} | {used:.0f}GB | {total:.0f}GB | {pct}%{flag} |")
            lines.append("")

        # Failed systemd units
        ur = client.query(
            "MATCH (u:SystemdUnit {hostname: $hn}) WHERE u.active_state = 'failed' "
            "RETURN u.name, u.description", {"hn": hn})
        if ur.result_set:
            lines.append("### Failed systemd units")
            for u_row in ur.result_set:
                lines.append(f"- `{u_row[0]}` — {u_row[1] or ''}")
            lines.append("")

        # Host journal/kernel events
        sev_filter = _sev_filter(min_severity)
        her = client.query(
            "MATCH (e:HostLogEvent {hostname: $hn}) "
            f"WHERE e.severity IN {sev_filter} "
            + ("AND e.last_seen >= $since " if since else "") +
            "RETURN e.source, e.unit, e.severity, e.count, e.template_text "
            "ORDER BY e.count DESC LIMIT 10",
            {"hn": hn, "since": since})
        ker = client.query(
            "MATCH (e:KernelEvent {hostname: $hn}) "
            f"WHERE e.severity IN {sev_filter} "
            + ("AND e.last_seen >= $since " if since else "") +
            "RETURN e.category, e.severity, e.count, e.template_text "
            "ORDER BY e.count DESC LIMIT 10",
            {"hn": hn, "since": since})
        if (her.result_set or []) or (ker.result_set or []):
            lines.append("### Host log events")
            for e_row in (ker.result_set or []):
                cat, sev, cnt, tmpl = e_row
                lines.append(f"- **[{sev}]** `{cat}` x{int(cnt)}: {(tmpl or '')[:120]}")
            for e_row in (her.result_set or []):
                src, unit, sev, cnt, tmpl = e_row
                lines.append(f"- **[{sev}]** `{unit or src}` x{int(cnt)}: {(tmpl or '')[:120]}")
            lines.append("")

    # ── K8s Clusters ──
    cluster_names = []
    cr = client.query("MATCH (c:K8sCluster) RETURN c.name", {})
    for row in (cr.result_set or []):
        cluster_names.append(row[0])

    # Also check provided cluster clients
    if cluster_clients:
        for cn in cluster_clients:
            if cn not in cluster_names:
                cluster_names.append(cn)

    for cluster_name in cluster_names:
        cc = (cluster_clients or {}).get(cluster_name, client)
        # Check if this client has the cluster's data
        test = cc.query("MATCH (p:K8sPod {cluster: $c}) RETURN count(p)", {"c": cluster_name})
        if not test.result_set or test.result_set[0][0] == 0:
            # Try the default graph name convention
            try:
                from savants.config import FalkorDBConfig
                gn = cluster_name.replace("-", "_")
                cc = GraphClient(FalkorDBConfig(
                    host=client._config.host, port=client._config.port,
                    graph_name=gn))
                test = cc.query("MATCH (p:K8sPod) RETURN count(p)", {})
                if not test.result_set or test.result_set[0][0] == 0:
                    continue
            except Exception:
                continue

        lines.append(f"## Cluster: {cluster_name}")
        lines.append("")

        # Pod status
        pr = cc.query(
            "MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC", {})
        if pr.result_set:
            status_str = " | ".join(f"**{row[1]}** {row[0]}" for row in pr.result_set)
            lines.append(f"**Pods:** {status_str}")
            lines.append("")

        # High restart pods
        rr = cc.query(
            "MATCH (p:K8sPod) WHERE p.restart_count > 5 "
            "RETURN p.namespace, p.name, p.restart_count, p.status "
            "ORDER BY p.restart_count DESC LIMIT 5", {})
        if rr.result_set:
            lines.append("### High-restart pods")
            for r_row in rr.result_set:
                ns, name, rc, status = r_row
                lines.append(f"- `{ns}/{name}` — {rc} restarts [{status}]")
            lines.append("")

        # Top errors
        sev_filter = _sev_filter(min_severity)
        er = cc.query(
            f"MATCH (e:LogEvent) WHERE e.severity IN {sev_filter} "
            + ("AND e.last_seen >= $since " if since else "") +
            "RETURN e.namespace, e.pod, e.severity, e.count, e.template_text "
            "ORDER BY e.count DESC LIMIT 15",
            {"since": since})
        if er.result_set:
            lines.append("### Top log events")
            for e_row in er.result_set:
                ns, pod, sev, cnt, tmpl = e_row
                lines.append(f"- **[{sev}]** `{ns}/{pod}` x{int(cnt)}: {(tmpl or '')[:120]}")
            lines.append("")

        # Mentions
        mr = cc.query(
            "MATCH (e:LogEvent)-[:MENTIONS]->(x) "
            f"WHERE e.severity IN {sev_filter} "
            + ("AND e.last_seen >= $since " if since else "") +
            "RETURN labels(x)[0], x.namespace, x.name, count(DISTINCT e) "
            "ORDER BY count(DISTINCT e) DESC LIMIT 10",
            {"since": since})
        if mr.result_set:
            lines.append("### Referenced entities")
            for m_row in mr.result_set:
                label, ns, name, n_ev = m_row
                short = (label or "").replace("K8s", "")
                lines.append(f"- **{short}** `{ns}/{name}` — {int(n_ev)} event(s)")
            lines.append("")

        # CAUSED_BY
        cbr = cc.query(
            "MATCH (e:LogEvent)-[r:CAUSED_BY]->(x) "
            "RETURN e.pod, e.namespace, labels(x)[0], x.name, "
            "r.change_type, r.delta_seconds "
            "ORDER BY r.delta_seconds ASC LIMIT 10", {})
        if cbr.result_set:
            lines.append("### Causal candidates (temporal correlation)")
            for cb_row in cbr.result_set:
                pod, ns, xlabel, xname, ctype, delta = cb_row
                lines.append(
                    f"- `{ns}/{pod}` error ← **{ctype}** on `{xname}` "
                    f"({abs(int(delta or 0))}s {'before' if (delta or 0) > 0 else 'after'})"
                )
            lines.append("")

    # Footer
    lines.append("---")
    lines.append("*Generated by [Savants](https://savants.dev) — your infrastructure savant.*")
    lines.append("")

    return "\n".join(lines)


def _sev_filter(min_severity: str) -> str:
    rank = {"INFO": 0, "WARN": 1, "ERROR": 2, "FATAL": 3}
    min_rank = rank.get(min_severity.upper(), 1)
    allowed = [s for s, r in rank.items() if r >= min_rank]
    return "[" + ", ".join(f"'{s}'" for s in allowed) + "]"
