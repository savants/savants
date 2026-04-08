"""Diff impact analysis.

Given a git ref (or a range), compute structural blast radius: what
entry points are reachable from the changed functions, and what config
keys moved. This is the query PR reviewers actually want to answer.

Used by:
  - `synapcode diff-impact <ref>` CLI command
  - `diff_impact` MCP tool
"""

from __future__ import annotations

import subprocess
from dataclasses import dataclass, field
from pathlib import Path

from synapcode.graph.client import GraphClient


# Decorators that strongly suggest a function is an entry point.
# We use endswith(...) matching so qualified forms like `@workflow.defn`,
# `@app.route`, `@cli.command` all hit without listing every permutation.
ENTRY_POINT_DECORATOR_TAILS = (
    "workflow.defn",
    "activity.defn",
    "app.route",
    "route",
    "cli.command",
    "command",
    "task",
    "scheduled_task",
    "periodic_task",
    "get",
    "post",
    "put",
    "delete",
    "patch",
    "websocket",
    "api_view",
)


@dataclass
class DiffImpactReport:
    ref: str
    changed_files: list[str] = field(default_factory=list)
    deleted_files: list[str] = field(default_factory=list)
    added_files: list[str] = field(default_factory=list)
    changed_functions: list[dict] = field(default_factory=list)
    entry_points_affected: list[dict] = field(default_factory=list)
    config_keys_changed: list[dict] = field(default_factory=list)
    transitive_caller_count: int = 0

    def to_dict(self) -> dict:
        return {
            "ref": self.ref,
            "changed_files": self.changed_files,
            "deleted_files": self.deleted_files,
            "added_files": self.added_files,
            "changed_functions": self.changed_functions,
            "entry_points_affected": self.entry_points_affected,
            "config_keys_changed": self.config_keys_changed,
            "transitive_caller_count": self.transitive_caller_count,
        }


def _git(repo_path: str, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", repo_path, *args],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def _parse_ref(repo_path: str, ref: str) -> tuple[str, str]:
    """Resolve a ref-ish string into (base_sha, head_sha).

    Accepts:
      - single ref  → (ref~1, ref)
      - a..b range  → (a, b)
      - a...b range → (merge-base(a,b), b)
    """
    if "..." in ref:
        base, head = ref.split("...", 1)
        base_sha = _git(repo_path, "merge-base", base, head).strip()
        head_sha = _git(repo_path, "rev-parse", head).strip()
        return (base_sha, head_sha)
    if ".." in ref:
        base, head = ref.split("..", 1)
        return (
            _git(repo_path, "rev-parse", base).strip(),
            _git(repo_path, "rev-parse", head).strip(),
        )
    head_sha = _git(repo_path, "rev-parse", ref).strip()
    base_sha = _git(repo_path, "rev-parse", f"{ref}~1").strip()
    return (base_sha, head_sha)


def _changed_functions_in_file(
    client: GraphClient, rel_path: str
) -> list[dict]:
    """Functions the graph knows about in this file."""
    rows = client.query(
        "MATCH (f:Function {file_path: $p}) "
        "RETURN f.name, f.start_line, f.end_line, f.class_name, f.decorators",
        {"p": rel_path},
    ).result_set
    return [
        {
            "name": r[0],
            "start_line": r[1],
            "end_line": r[2],
            "class_name": r[3] or "",
            "decorators": r[4] or [],
            "file_path": rel_path,
        }
        for r in rows
    ]


def _is_entry_point(fn: dict) -> bool:
    decs = fn.get("decorators") or []
    for d in decs:
        for tail in ENTRY_POINT_DECORATOR_TAILS:
            if d == tail or d.endswith("." + tail):
                return True
    return False


def _transitive_callers(
    client: GraphClient, fn_name: str, file_path: str, max_depth: int = 4
) -> list[dict]:
    """Find every function that eventually calls this one, via CALLS or
    REFERENCES_SYMBOL edges. Bounded depth to avoid pathological blowups.
    """
    rows = client.query(
        f"MATCH (caller:Function)-[:CALLS|REFERENCES_SYMBOL*1..{max_depth}]->"
        f"(target:Function {{name: $n, file_path: $fp}}) "
        "RETURN DISTINCT caller.name, caller.file_path, caller.decorators, caller.class_name",
        {"n": fn_name, "fp": file_path},
    ).result_set
    return [
        {
            "name": r[0],
            "file_path": r[1],
            "decorators": r[2] or [],
            "class_name": r[3] or "",
        }
        for r in rows
    ]


def _config_keys_touched_in_file(
    client: GraphClient, rel_path: str
) -> list[dict]:
    rows = client.query(
        "MATCH (k:ConfigKey {file_path: $p}) RETURN k.name, k.value, k.format",
        {"p": rel_path},
    ).result_set
    return [{"name": r[0], "value": r[1], "format": r[2]} for r in rows]


def diff_impact(
    repo_path: str, ref: str, client: GraphClient
) -> DiffImpactReport:
    """Run the full diff-impact analysis for a git ref/range."""
    base_sha, head_sha = _parse_ref(repo_path, ref)

    # File-level diff
    out = _git(repo_path, "diff", "--name-status", base_sha, head_sha)
    changed: list[str] = []
    deleted: list[str] = []
    added: list[str] = []
    for line in out.strip().splitlines():
        if not line:
            continue
        parts = line.split("\t", 1)
        status = parts[0]
        fp = parts[1] if len(parts) > 1 else ""
        if status.startswith("D"):
            deleted.append(fp)
        elif status.startswith("A"):
            added.append(fp)
        else:
            changed.append(fp)

    report = DiffImpactReport(
        ref=ref,
        changed_files=changed,
        added_files=added,
        deleted_files=deleted,
    )

    all_touched = changed + added + deleted
    transitive_set: set[tuple[str, str]] = set()

    for fp in all_touched:
        # Config file?
        if any(fp.endswith(ext) for ext in (".yaml", ".yml", ".toml", ".json")):
            for k in _config_keys_touched_in_file(client, fp):
                report.config_keys_changed.append({**k, "file": fp})
            continue

        # Code file → find functions the graph knows about
        fns = _changed_functions_in_file(client, fp)
        report.changed_functions.extend(fns)

        for fn in fns:
            callers = _transitive_callers(client, fn["name"], fn["file_path"])
            for c in callers:
                transitive_set.add((c["name"], c["file_path"]))
                if _is_entry_point(c):
                    report.entry_points_affected.append(c)
            # The changed function itself might BE the entry point
            if _is_entry_point(fn):
                report.entry_points_affected.append(fn)

    report.transitive_caller_count = len(transitive_set)

    # Dedupe entry points
    seen: set[tuple[str, str]] = set()
    unique_eps: list[dict] = []
    for ep in report.entry_points_affected:
        key = (ep.get("name", ""), ep.get("file_path", ""))
        if key in seen:
            continue
        seen.add(key)
        unique_eps.append(ep)
    report.entry_points_affected = unique_eps

    return report


def format_report(report: DiffImpactReport) -> str:
    """Render a DiffImpactReport as human-readable text for CLI/MCP output."""
    lines = [f"Diff impact for {report.ref}", "=" * 60]
    lines.append(
        f"Changed files: {len(report.changed_files)}  "
        f"Added: {len(report.added_files)}  "
        f"Deleted: {len(report.deleted_files)}"
    )
    lines.append(
        f"Changed functions (in graph): {len(report.changed_functions)}"
    )
    lines.append(
        f"Transitive callers reached:  {report.transitive_caller_count}"
    )
    lines.append("")

    if report.entry_points_affected:
        lines.append(f"Entry points affected ({len(report.entry_points_affected)}):")
        for ep in report.entry_points_affected[:30]:
            decs = ", ".join(ep.get("decorators") or [])
            cls = f"[{ep['class_name']}.]" if ep.get("class_name") else ""
            lines.append(f"  @{decs or '(no decorator)'}  {cls}{ep['name']}")
            lines.append(f"      {ep['file_path']}")
        if len(report.entry_points_affected) > 30:
            lines.append(f"  ... and {len(report.entry_points_affected) - 30} more")
        lines.append("")
    else:
        lines.append("No entry points affected (changes are internal-only).")
        lines.append("")

    if report.config_keys_changed:
        lines.append(f"Config keys in changed files ({len(report.config_keys_changed)}):")
        for k in report.config_keys_changed[:20]:
            val = k.get("value", "")[:60]
            lines.append(f"  {k['name']} = {val}  ({k['file']})")
        if len(report.config_keys_changed) > 20:
            lines.append(f"  ... and {len(report.config_keys_changed) - 20} more")

    return "\n".join(lines)
