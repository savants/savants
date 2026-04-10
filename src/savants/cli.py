"""SynapCode CLI: the front door to the cognitive stack.

Usage:
    savants init /path/to/repo      # First-time index
    savants index /path/to/repo     # Re-index (incremental or full)
    savants query "what calls X?"   # Query the graph
    savants impact process_data     # Cascading impact analysis
    savants search "auth"           # Search functions/classes
    savants status                  # Health check
    savants gc /path/to/repo        # Garbage collection
    savants snapshot create /repo   # Create Git LFS snapshot
    savants snapshot restore /repo  # Restore from snapshot
    savants serve                   # Start MCP server
    savants worker                  # Start Temporal worker
"""

from __future__ import annotations

import sys

import click

from savants.config import load_config
from savants.graph.client import GraphClient
from savants.graph.cpg import CodePropertyGraphBuilder
from savants.graph.query import GraphQueryEngine
from savants.sync.git_hooks import get_current_head, get_last_indexed_sha, save_last_indexed_sha


def _find_bundled_binary(filename: str) -> str | None:
    """Locate a binary bundled with the savants distribution.

    Search order:
      1. Wheel/sdist install: src/savants/binaries/<filename> via importlib.resources
      2. Monorepo dev checkout: walk up to find desktop/src-tauri/binaries/<filename>
      3. None if neither matches
    """
    import os
    from pathlib import Path

    # 1. Wheel-installed location. importlib.resources is the supported
    #    way to read package_data from an installed wheel; it works whether
    #    the wheel was unpacked, zipped, or installed via pip.
    try:
        from importlib.resources import files as _resource_files
        bundled = _resource_files("savants") / "binaries" / filename
        if bundled.is_file():
            path_str = str(bundled)
            # Some package layouts return a Traversable that resolves
            # to a real path; only return if the underlying file is
            # actually executable + readable.
            if os.access(path_str, os.X_OK | os.R_OK):
                return path_str
    except (ImportError, ModuleNotFoundError, FileNotFoundError, OSError):
        pass

    # 2. Monorepo dev checkout. Walk up looking for desktop/src-tauri/binaries.
    here = Path(__file__).resolve()
    for parent in here.parents:
        candidate = parent / "desktop" / "src-tauri" / "binaries" / filename
        if candidate.exists() and os.access(candidate, os.X_OK | os.R_OK):
            return str(candidate)
        if (parent / ".git").exists() or parent == parent.parent:
            break
    return None


def _find_falkordb_module() -> str | None:
    """Locate the FalkorDB Redis module binary.

    Search order:
      1. $FALKORDB_MODULE env var (explicit override)
      2. Bundled binary in desktop/src-tauri/binaries/ (monorepo checkout)
      3. System standard paths
    """
    import os

    env = os.environ.get("FALKORDB_MODULE")
    if env and os.path.exists(env):
        return env

    bundled = _find_bundled_binary("falkordb.so")
    if bundled:
        return bundled

    for p in (
        "/usr/lib/redis/modules/falkordb.so",
        "/usr/local/lib/redis/modules/falkordb.so",
        "/opt/homebrew/lib/redis/modules/falkordb.so",
        "/usr/lib/falkordb/falkordb.so",
    ):
        if os.path.exists(p):
            return p
    return None


def _is_nixos() -> bool:
    """NixOS can't run generic dynamic ELFs without nix-ld. We detect it
    so we can skip the bundled redis-server and use the nix-provided one.
    """
    import os
    return os.path.exists("/etc/NIXOS") or os.path.exists("/run/current-system/sw")


def _find_redis_binary() -> str | None:
    """Locate a redis-server binary.

    Search order:
      1. $REDIS_SERVER env var (explicit override)
      2. Bundled `redis-server-bundled` in desktop/src-tauri/binaries/
         (skipped on NixOS — generic ELFs can't run there without nix-ld)
      3. `redis-server` on PATH
    """
    import os
    import shutil

    env = os.environ.get("REDIS_SERVER")
    if env and os.access(env, os.X_OK):
        return env

    if not _is_nixos():
        bundled = _find_bundled_binary("redis-server-bundled")
        if bundled:
            return bundled

    return shutil.which("redis-server")


def _falkordb_env() -> dict:
    """Build env for the FalkorDB subprocess, injecting libgomp on NixOS.

    FalkorDB links against libgomp.so.1 which isn't on NixOS's default
    loader path. If we can find a suitable libgomp via nix-store, prepend it.
    """
    import os
    env = os.environ.copy()
    # Try a few known-good nix paths; silently skip if none exist.
    from glob import glob
    candidates = glob("/nix/store/*-gcc-*-lib/lib/libgomp.so.1")
    if candidates:
        gomp_dir = os.path.dirname(candidates[0])
        existing = env.get("LD_LIBRARY_PATH", "")
        env["LD_LIBRARY_PATH"] = f"{gomp_dir}:{existing}" if existing else gomp_dir
    return env


def _falkordb_pidfile():
    from pathlib import Path
    d = Path.home() / ".savants"
    d.mkdir(parents=True, exist_ok=True)
    return d / "falkordb.pid"


def _start_falkordb_process(port: int, verbose: bool = True) -> int | None:
    """Start redis-server + FalkorDB module. Returns PID on success."""
    import subprocess
    import time

    redis_bin = _find_redis_binary()
    if not redis_bin:
        if verbose:
            click.echo(
                "No redis-server found. Options:\n"
                "  - Install Redis via your package manager\n"
                "  - Set REDIS_SERVER=/path/to/redis-server\n"
                "  - Use the bundled binary (desktop/src-tauri/binaries/redis-server-bundled)",
                err=True,
            )
        return None

    module = _find_falkordb_module()
    # Persist graph to disk so it survives restarts
    data_dir = str(Path.home() / ".savants" / "data")
    Path(data_dir).mkdir(parents=True, exist_ok=True)
    cmd = [redis_bin, "--port", str(port), "--daemonize", "no",
           "--dir", data_dir, "--dbfilename", "savants.rdb",
           "--save", "60", "1"]  # RDB snapshot every 60s if ≥1 write
    if module:
        cmd.extend(["--loadmodule", module])
    else:
        if verbose:
            click.echo(
                "Warning: index backend module not found. Queries will fail. "
                "Set FALKORDB_MODULE=/path/to/module.so.",
                err=True,
            )

    if verbose:
        click.echo(f"Starting index backend on port {port}...")
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        env=_falkordb_env(),
    )

    # Wait for it
    for _ in range(20):
        time.sleep(0.5)
        try:
            import redis
            r = redis.Redis(host="127.0.0.1", port=port, socket_connect_timeout=0.5)
            r.ping()
            _falkordb_pidfile().write_text(str(proc.pid))
            if verbose:
                click.echo(f"Index backend started (PID {proc.pid}, port {port}).")
            return proc.pid
        except Exception:
            if proc.poll() is not None:
                if verbose:
                    click.echo("Index backend exited during startup.", err=True)
                return None
            continue
    if verbose:
        click.echo("Timed out waiting for index backend.", err=True)
    return None


def _ensure_falkordb() -> GraphClient:
    """Connect to FalkorDB, auto-starting it as a native process if needed."""
    config = load_config()
    client = GraphClient(config.falkordb)

    try:
        client.ensure_schema()
        return client
    except Exception:
        pass

    if _start_falkordb_process(config.falkordb.port) is None:
        sys.exit(1)

    client = GraphClient(config.falkordb)
    try:
        client.ensure_schema()
        return client
    except Exception as e:
        click.echo(f"Could not connect to index backend: {e}", err=True)
        sys.exit(1)


@click.group()
@click.version_option(package_name="savants")
def cli():
    """Savants: your infrastructure savant. Know what's wrong in 60 seconds."""


@cli.command()
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
@click.option("--history", default=300, help="Number of git commits to walk for Layer 2 (0 to skip)")
def init(repo_path: str, history: int):
    """First-time setup: create schema, full index, walk history, save bookmark."""
    client = _ensure_falkordb()

    click.echo(f"Indexing {repo_path}...")
    builder = CodePropertyGraphBuilder(repo_path=repo_path, client=client)
    stats = builder.build()

    click.echo(
        f"Code graph: {stats['files']} files, {stats['functions']} functions, "
        f"{stats['classes']} classes, {stats['edges']} edges"
    )
    if stats.get("config_keys"):
        click.echo(
            f"Config:     {stats['config_files']} files, {stats['config_keys']} keys"
        )
    if stats.get("env_vars"):
        click.echo(f"Env vars:   {stats['env_vars']} references")
    if stats.get("references_symbol"):
        click.echo(f"Symbol refs: {stats['references_symbol']} string-literal edges")

    # Layer 2: auto-walk history so risk_score / co_change / pre_change_warning
    # all work out of the box. Half the composite MCP tools used to degrade to
    # "(history not loaded)" stubs because nobody remembered to run the walker.
    if history > 0:
        try:
            from savants.history.walker import GitHistoryWalker
            click.echo(f"Walking last {history} commits for Layer 2 (history)...")
            walker = GitHistoryWalker(
                repo_path=repo_path,
                client=client,
                max_commits=history,
            )
            hist_result = walker.walk()
            click.echo(
                f"History:    {hist_result.episodes_created} episodes, "
                f"{hist_result.changes_edges_created} CHANGES edges"
            )
        except Exception as e:
            click.echo(f"Warning: history walk skipped ({e})", err=True)

    # Save bookmark
    try:
        head = get_current_head(repo_path)
        save_last_indexed_sha(repo_path, head)
        click.echo(f"Bookmark saved at {head[:8]}")
    except Exception:
        click.echo("Note: git bookmark not saved (not a git repo)", err=True)


@cli.command()
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
@click.option("--full", is_flag=True, help="Force full re-index instead of incremental")
def index(repo_path: str, full: bool):
    """Re-index a repository (incremental by default)."""
    client = _ensure_falkordb()
    client.ensure_schema()

    bookmark = get_last_indexed_sha(repo_path)

    if full or not bookmark:
        if not bookmark:
            click.echo("No bookmark found, running full index...")
        else:
            click.echo("Running full re-index...")

        builder = CodePropertyGraphBuilder(repo_path=repo_path, client=client)
        stats = builder.build()
        click.echo(
            f"Full index: {stats['files']} files, {stats['functions']} functions, "
            f"{stats['classes']} classes"
        )
    else:
        head = get_current_head(repo_path)
        if bookmark == head:
            click.echo("Graph is already up to date.")
            return

        click.echo(f"Incremental update: {bookmark[:8]} -> {head[:8]}...")
        from savants.sync.diff import compute_diff

        diff = compute_diff(repo_path, bookmark, head)
        builder = CodePropertyGraphBuilder(repo_path=repo_path, client=client)
        stats = builder.build_incremental(
            changed_files=diff.changed_files + diff.added_files,
            deleted_files=diff.deleted_files,
        )
        click.echo(f"Updated {stats['updated']} files, removed {stats['deleted']} files")

    # Update bookmark
    try:
        save_last_indexed_sha(repo_path, get_current_head(repo_path))
    except Exception:
        pass


@cli.command()
@click.argument("question")
def query(question: str):
    """Query the graph for structural context about your codebase."""
    client = _ensure_falkordb()
    engine = GraphQueryEngine(client)

    from savants.pipelines.graphrag_inlet import build_graph_context

    context = build_graph_context(question, client)
    if context:
        click.echo(context)
    else:
        # Fall back to pattern search
        refs = question.split()
        for term in refs:
            results = engine.search_by_pattern(term)
            if results:
                click.echo(f"Matches for '{term}':")
                for r in results[:20]:
                    click.echo(f"  {r['type']:10} {r['name']:30} {r.get('file', '')}")
                break
        else:
            click.echo("No graph context found. Have you indexed a repository?")


@cli.command("diff-impact")
@click.argument("ref")
@click.option("--repo", default=".", help="Path to the git repo (defaults to cwd)")
@click.option("--json", "as_json", is_flag=True, help="Output machine-readable JSON")
def diff_impact_cmd(ref: str, repo: str, as_json: bool):
    """Structural blast radius for a git ref or range.

    Examples:
        savants diff-impact HEAD
        savants diff-impact main..feature-branch
        savants diff-impact abc123...def456
    """
    from savants.analysis.diff_impact import diff_impact, format_report
    import json as _json

    client = _ensure_falkordb()
    report = diff_impact(repo_path=repo, ref=ref, client=client)
    if as_json:
        click.echo(_json.dumps(report.to_dict(), indent=2))
    else:
        click.echo(format_report(report))


@cli.command()
@click.argument("function_name")
@click.option("--depth", default=5, help="Maximum traversal depth")
@click.option("--json", "as_json", is_flag=True, help="Output machine-readable JSON")
def impact(function_name: str, depth: int, as_json: bool):
    """Analyze cascading impact of changing a function."""
    import json as _json

    client = _ensure_falkordb()
    engine = GraphQueryEngine(client)
    result = engine.impact_analysis(function_name, max_depth=depth)

    if as_json:
        click.echo(_json.dumps({
            "target": result.target,
            "direct_dependents": result.direct_dependents,
            "transitive_dependents": result.transitive_dependents,
            "affected_files": result.affected_files,
        }, indent=2))
        return

    click.echo(f"Impact analysis for '{result.target}':\n")
    if result.direct_dependents:
        click.echo(f"  Direct dependents ({len(result.direct_dependents)}):")
        for dep in result.direct_dependents:
            click.echo(f"    - {dep}")
    else:
        click.echo("  No direct dependents found.")
    if result.transitive_dependents:
        click.echo(f"\n  Transitive dependents ({len(result.transitive_dependents)}):")
        for dep in result.transitive_dependents:
            click.echo(f"    - {dep}")
    if result.affected_files:
        click.echo(f"\n  Affected files ({len(result.affected_files)}):")
        for f in result.affected_files:
            click.echo(f"    - {f}")


@cli.command()
@click.argument("pattern")
@click.option("--json", "as_json", is_flag=True, help="Output machine-readable JSON")
def search(pattern: str, as_json: bool):
    """Search for functions, classes, and config keys by name pattern."""
    import json as _json

    client = _ensure_falkordb()
    engine = GraphQueryEngine(client)
    results = engine.search_by_pattern(pattern)

    if as_json:
        click.echo(_json.dumps(results, indent=2))
        return

    if not results:
        click.echo(f"No matches for '{pattern}'")
        return

    click.echo(f"Found {len(results)} matches:")
    for r in results:
        suffix = f"  = {r['value'][:60]}" if "value" in r else ""
        click.echo(f"  {r['type']:10} {r['name']:40} {r.get('file', '')}{suffix}")


@cli.command()
def status():
    """Show graph stats, last indexed commit, service health."""
    config = load_config()

    # FalkorDB health
    click.echo("Index backend:")
    try:
        client = GraphClient(config.falkordb)
        nodes = client.node_count()
        edges = client.edge_count()
        click.echo(f"  Status:  connected ({config.falkordb.host}:{config.falkordb.port})")
        click.echo(f"  Graph:   {config.falkordb.graph_name}")
        click.echo(f"  Nodes:   {nodes}")
        click.echo(f"  Edges:   {edges}")
    except Exception as e:
        click.echo(f"  Status:  unreachable ({e})")

    # Temporal health
    click.echo("\nTemporal:")
    try:
        import asyncio
        from temporalio.client import Client as TemporalClient

        async def _check():
            c = await TemporalClient.connect(config.temporal.host)
            return c

        asyncio.run(_check())
        click.echo(f"  Status:  connected ({config.temporal.host})")
        click.echo(f"  Queue:   {config.temporal.task_queue}")
    except Exception as e:
        click.echo(f"  Status:  unreachable ({e})")

    # Hardware
    from savants.hardware.monitor import get_system_stats

    stats = get_system_stats()
    click.echo(f"\nHardware:")
    click.echo(f"  RAM:     {stats.ram_used_gb:.1f}/{stats.ram_total_gb:.1f} GB ({stats.ram_percent:.0f}%)")
    click.echo(f"  CPU:     {stats.cpu_percent:.0f}% ({stats.cpu_count} cores)")
    click.echo(f"  Platform: {stats.platform} {stats.architecture}")
    if stats.is_apple_silicon:
        click.echo(f"  Apple Silicon detected (UMA advantage)")


@cli.command()
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
def gc(repo_path: str):
    """Run garbage collection on the graph."""
    from savants.graph.gc import GraphGarbageCollector

    client = _ensure_falkordb()
    collector = GraphGarbageCollector(client)
    report = collector.run_full_gc(repo_path)

    click.echo(f"GC complete ({report.duration_ms:.0f}ms):")
    click.echo(f"  Orphan nodes removed:     {report.orphan_nodes_removed}")
    click.echo(f"  Stale files removed:       {report.stale_files_removed}")
    click.echo(f"  Expired facts removed:     {report.expired_facts_removed}")
    click.echo(f"  Contradictions resolved:   {report.contradictions_resolved}")


@cli.group()
def k8s():
    """Ingest a Kubernetes cluster into the graph (runtime layer)."""


@k8s.command("snapshot")
@click.argument("cluster")
@click.option("--context", help="kubeconfig context name (defaults to current)")
@click.option("--kubeconfig", help="Path to kubeconfig (defaults to $KUBECONFIG or ~/.kube/config)")
def k8s_snapshot(cluster: str, context: str | None, kubeconfig: str | None):
    """One-shot snapshot of a live cluster into its graph.

    Use this for the initial load or a manual refresh. For live watching,
    use `savants k8s watch` instead.
    """
    from savants.config import FalkorDBConfig
    from savants.graph.client import GraphClient
    from savants.k8s.ingestor import K8sIngestor

    graph_name = cluster.replace("-", "_")
    client = GraphClient(FalkorDBConfig(graph_name=graph_name))
    ingestor = K8sIngestor(
        graph_client=client,
        cluster_name=cluster,
        kube_context=context,
        kubeconfig_path=kubeconfig,
    )
    stats = ingestor.snapshot()
    click.echo(stats.summary())


@k8s.command("watch")
@click.argument("cluster")
@click.option("--context", help="kubeconfig context name (defaults to current)")
@click.option("--kubeconfig", help="Path to kubeconfig")
@click.option("--logs/--no-logs", default=True, help="Enable log intelligence layer")
@click.option("--tail-lines", default=0, type=int,
              help="Replay N lines of history per pod on subscribe (0 = live only)")
@click.option("--retention-hours", default=24, type=int,
              help="How long to keep LogEvent nodes before pruning")
@click.option("--rate-limit", default=1000, type=int,
              help="Max log lines per second per pod (prevents misbehaving pods from flooding)")
def k8s_watch(
    cluster: str,
    context: str | None,
    kubeconfig: str | None,
    logs: bool,
    tail_lines: int,
    retention_hours: int,
    rate_limit: int,
):
    """Run the live K8s watcher (+ optional log intelligence) forever.

    Holds long-lived watch streams against the apiserver and, when
    `--logs` is on, one streaming log tail per running pod. Writes
    cluster state + LogEvent nodes to the per-cluster graph. Block
    until SIGINT/SIGTERM.

    Example:

        savants k8s watch astra-k3s --tail-lines 500

    To run as a daemon see `docs/k8s-watch-systemd.md`.
    """
    import logging
    from savants.config import FalkorDBConfig
    from savants.graph.client import GraphClient
    from savants.k8s.ingestor import K8sIngestor
    from savants.k8s.log_watcher import LogWatcher
    from savants.k8s.watcher import K8sWatcher

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)-5s %(name)s: %(message)s",
    )

    graph_name = cluster.replace("-", "_")
    client = GraphClient(FalkorDBConfig(graph_name=graph_name))
    ingestor = K8sIngestor(
        graph_client=client,
        cluster_name=cluster,
        kube_context=context,
        kubeconfig_path=kubeconfig,
    )
    # Run snapshot first so entity index + baseline state exist before
    # the watch loops start; the watchers will then incrementally
    # reconcile on top.
    click.echo(f"[{cluster}] initial snapshot...")
    snap_stats = ingestor.snapshot()
    click.echo(snap_stats.summary())

    log_watcher: LogWatcher | None = None
    if logs:
        ingestor._load_k8s_client()
        core = ingestor._k8s_client["core"]
        log_watcher = LogWatcher(
            graph=client,
            cluster=cluster,
            core_api=core,
            tail_lines=tail_lines,
            rate_limit_per_sec=rate_limit,
            retention_seconds=retention_hours * 3600,
        )
        click.echo(f"[{cluster}] log intelligence enabled (retention={retention_hours}h)")

    watcher = K8sWatcher(ingestor, log_watcher=log_watcher)
    click.echo(f"[{cluster}] starting watch loops (Ctrl-C to stop)...")
    try:
        watcher.run_forever()
    except KeyboardInterrupt:
        pass
    finally:
        if log_watcher is not None:
            log_watcher.flush_all()
            log_watcher.stop()
        watcher.stop()
    click.echo(f"[{cluster}] shutdown complete")


@cli.group()
def snapshot():
    """Create or restore graph snapshots for team bootstrapping."""


@snapshot.command("create")
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
def snapshot_create(repo_path: str):
    """Serialize the graph to a file for Git LFS."""
    from savants.sync.bootstrap import create_snapshot

    path = create_snapshot(repo_path)
    click.echo(f"Snapshot saved to {path}")
    click.echo("Track it with: git lfs track '.savants/*.dump'")


@snapshot.command("restore")
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
def snapshot_restore(repo_path: str):
    """Restore a graph from a Git LFS snapshot."""
    from savants.sync.bootstrap import restore_snapshot

    if restore_snapshot(repo_path):
        client = _ensure_falkordb()
        click.echo(f"Graph restored: {client.node_count()} nodes, {client.edge_count()} edges")
    else:
        click.echo("No snapshot found. Run 'savants snapshot create' first.", err=True)
        sys.exit(1)


@cli.command()
def serve():
    """Start the MCP server (for Claude Code / Cursor integration)."""
    from savants.mcp.server import main as mcp_main

    mcp_main()


@cli.command()
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
@click.option("--interval", default=2.0, help="Poll interval in seconds")
def watch(repo_path: str, interval: float):
    """Long-running file watcher that auto-reindexes on save.

    Polls mtimes on code + config files every --interval seconds.
    When anything changes, runs an incremental rebuild. Kill with Ctrl-C.
    Zero extra dependencies (stdlib only).
    """
    import time
    from pathlib import Path as _P
    from savants.graph.cpg import SUPPORTED_EXTENSIONS, CONFIG_EXTENSIONS

    client = _ensure_falkordb()
    root = _P(repo_path).resolve()
    exts = tuple(SUPPORTED_EXTENSIONS) + tuple(CONFIG_EXTENSIONS)
    excluded = {"node_modules", ".git", "__pycache__", "venv", ".venv", "dist", "build", "target", ".savants"}

    def _scan() -> dict:
        """Return {path: mtime} for every indexable file under root."""
        out = {}
        for p in root.rglob("*"):
            if not p.is_file():
                continue
            if any(part in excluded for part in p.parts):
                continue
            if p.suffix not in exts:
                continue
            try:
                out[str(p)] = p.stat().st_mtime
            except OSError:
                continue
        return out

    click.echo(f"Watching {root} (every {interval}s, Ctrl-C to stop)")
    baseline = _scan()
    click.echo(f"Tracking {len(baseline)} files")

    try:
        while True:
            time.sleep(interval)
            current = _scan()
            changed = [p for p, m in current.items() if baseline.get(p) != m]
            deleted = [p for p in baseline if p not in current]
            if not changed and not deleted:
                continue

            changed_rel = [str(_P(p).relative_to(root)) for p in changed]
            deleted_rel = [str(_P(p).relative_to(root)) for p in deleted]

            click.echo(
                f"[{time.strftime('%H:%M:%S')}] "
                f"{len(changed_rel)} changed, {len(deleted_rel)} deleted → reindexing..."
            )
            try:
                builder = CodePropertyGraphBuilder(repo_path=str(root), client=client)
                stats = builder.build_incremental(
                    changed_files=changed_rel,
                    deleted_files=deleted_rel,
                )
                click.echo(f"  updated={stats.get('updated', 0)} deleted={stats.get('deleted', 0)}")
            except Exception as e:
                click.echo(f"  reindex failed: {e}", err=True)
            baseline = current
    except KeyboardInterrupt:
        click.echo("\nStopped.")


@cli.group()
def hooks():
    """Install git hooks that keep the graph in sync with your checkout."""


@hooks.command("install")
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False), default=".")
def hooks_install(repo_path: str):
    """Install post-checkout + post-merge hooks that auto-reindex.

    These run `savants index` whenever you switch branches or pull.
    Removes the "is my graph stale?" question permanently for team use.
    """
    from pathlib import Path as _P
    import stat as _stat

    repo = _P(repo_path).resolve()
    hook_dir = repo / ".git" / "hooks"
    if not hook_dir.exists():
        click.echo(f"No .git/hooks at {repo} — not a git repo?", err=True)
        sys.exit(1)

    script = f"""#!/bin/sh
# Installed by `savants hooks install`
# Auto-reindexes the SynapCode graph when you switch branches or pull.
set -e
if command -v savants >/dev/null 2>&1; then
    savants index "{repo}" >/dev/null 2>&1 &
fi
"""
    for hook_name in ("post-checkout", "post-merge"):
        path = hook_dir / hook_name
        path.write_text(script)
        path.chmod(path.stat().st_mode | _stat.S_IXUSR | _stat.S_IXGRP | _stat.S_IXOTH)
        click.echo(f"Installed {hook_name}")
    click.echo("\nRun 'savants init <repo>' first if you haven't yet.")


@cli.command()
def worker():
    """Start the Temporal worker for durable workflow execution."""
    import asyncio

    from savants.temporal.worker import main as worker_main

    asyncio.run(worker_main())


# --- FalkorDB lifecycle -----------------------------------------------------


@cli.group()
def falkordb():
    """Manage the FalkorDB sidecar (start/stop/status)."""


@falkordb.command("start")
def falkordb_start():
    """Start redis-server with the FalkorDB module."""
    config = load_config()
    # If already up, do nothing.
    try:
        GraphClient(config.falkordb).ensure_schema()
        click.echo(f"Already running on port {config.falkordb.port}.")
        return
    except Exception:
        pass
    if _start_falkordb_process(config.falkordb.port) is None:
        sys.exit(1)


@falkordb.command("stop")
def falkordb_stop():
    """Stop the FalkorDB sidecar started by `savants falkordb start`."""
    import os
    import signal

    pidfile = _falkordb_pidfile()
    if not pidfile.exists():
        click.echo("No pidfile — not started via savants, or already stopped.")
        return
    pid = int(pidfile.read_text().strip())
    try:
        os.kill(pid, signal.SIGTERM)
        click.echo(f"Sent SIGTERM to {pid}.")
    except ProcessLookupError:
        click.echo("Process already gone.")
    pidfile.unlink(missing_ok=True)


@cli.command()
def doctor():
    """One-command diagnostic: graph health, module path, staleness, orphans.

    Answers the 'why isn't this working?' question without a dozen ad-hoc
    queries. Exit code is nonzero if anything's broken.
    """
    import shutil
    from glob import glob

    problems: list[str] = []
    config = load_config()

    click.echo("SynapCode doctor")
    click.echo("=" * 50)

    # 1. FalkorDB reachable?
    click.echo("\nIndex backend:")
    try:
        client = GraphClient(config.falkordb)
        n = client.node_count()
        e = client.edge_count()
        click.echo(f"  reachable:     yes ({config.falkordb.host}:{config.falkordb.port})")
        click.echo(f"  graph:         {config.falkordb.graph_name}")
        click.echo(f"  nodes:         {n}")
        click.echo(f"  edges:         {e}")
        if n == 0:
            problems.append("Backend is up but graph is empty. Run 'savants init <repo>'.")
    except Exception as exc:
        click.echo(f"  reachable:     NO ({exc})")
        problems.append(f"Backend unreachable: {exc}")
        client = None

    # 2. Module discovery
    click.echo("\nIndex backend module:")
    module = _find_falkordb_module()
    click.echo(f"  path:          {module or 'NOT FOUND'}")
    if not module:
        problems.append("Backend module not found. Set FALKORDB_MODULE or reinstall the wheel.")

    # 3. backend host binary
    click.echo("\nBackend host:")
    rbin = _find_redis_binary()
    source = "bundled" if rbin and "binaries/redis-server-bundled" in rbin else "system"
    click.echo(f"  path:          {rbin or 'NOT FOUND'}  ({source})")
    if not rbin:
        problems.append("No redis-server found (bundled or system).")

    # 4. libgomp (NixOS gotcha)
    click.echo("\nlibgomp (OpenMP runtime):")
    gomp = glob("/nix/store/*-gcc-*-lib/lib/libgomp.so.1") or glob("/usr/lib*/libgomp.so.1")
    if gomp:
        click.echo(f"  path:          {gomp[0]}")
    else:
        click.echo(f"  path:          not auto-located (may still work via ldconfig)")

    # 5. Graph composition — what's been indexed?
    if client is not None and n > 0:
        click.echo("\nGraph composition:")
        try:
            labels_rows = client.query(
                "MATCH (n) RETURN labels(n)[0] AS lbl, count(n) ORDER BY count(n) DESC"
            ).result_set
            for lbl, cnt in labels_rows:
                click.echo(f"  {lbl:14} {cnt}")
        except Exception:
            pass

        # Layer 2 present?
        try:
            eps = client.query("MATCH (e:Episode) RETURN count(e)").result_set[0][0]
            click.echo(f"\nHistory layer:")
            click.echo(f"  episodes:      {eps}")
            if eps == 0:
                problems.append("Layer 2 (history) is empty. Re-run 'savants init <repo>' to walk git history.")
        except Exception:
            pass

        # Orphan files (files with no functions or classes)
        try:
            orphans = client.query(
                "MATCH (f:File) WHERE NOT (f)-[:CONTAINS]->() RETURN count(f)"
            ).result_set[0][0]
            if orphans:
                click.echo(f"\nOrphan files (no CONTAINS edges): {orphans}")
                if orphans > 10:
                    problems.append(f"{orphans} orphan File nodes — consider 'savants gc'.")
        except Exception:
            pass

    # Summary
    click.echo("\n" + "=" * 50)
    if problems:
        click.echo(f"Found {len(problems)} problem(s):")
        for p in problems:
            click.echo(f"  ✗ {p}")
        sys.exit(1)
    else:
        click.echo("All checks passed.")


@falkordb.command("status")
def falkordb_status():
    """Report FalkorDB health and bundled module path."""
    config = load_config()
    module = _find_falkordb_module()
    click.echo(f"Module:  {module or '(not found)'}")
    click.echo(f"Port:    {config.falkordb.port}")
    try:
        c = GraphClient(config.falkordb)
        n = c.node_count()
        e = c.edge_count()
        click.echo(f"Status:  up  ({n} nodes, {e} edges)")
    except Exception as exc:
        click.echo(f"Status:  down  ({exc})")


# --- Convenience: ask a composite question about a symbol -------------------


@cli.command()
@click.argument("name")
@click.option("--json", "as_json", is_flag=True, help="Output machine-readable JSON")
def ask(name: str, as_json: bool):
    """One-shot structural trace for a symbol: callers, references, decorators."""
    import json as _json

    client = _ensure_falkordb()

    defs = [
        {"type": r[0], "name": r[1], "file": r[2]}
        for r in client.query(
            "MATCH (n) WHERE (n:Function OR n:Class) AND n.name = $name "
            "RETURN labels(n)[0], n.name, n.file_path LIMIT 10",
            {"name": name},
        ).result_set
    ]
    callers = [
        {"name": r[0], "file": r[1]}
        for r in client.query(
            "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name}) "
            "RETURN DISTINCT c.name, c.file_path LIMIT 20",
            {"name": name},
        ).result_set
    ]
    refs = [
        {"name": r[0], "file": r[1]}
        for r in client.query(
            "MATCH (c:Function)-[:REFERENCES_SYMBOL]->(t) WHERE t.name = $name "
            "RETURN DISTINCT c.name, c.file_path LIMIT 20",
            {"name": name},
        ).result_set
    ]
    methods_of = [
        r[0] for r in client.query(
            "MATCH (m:Function)-[:METHOD_OF]->(c:Class {name: $name}) "
            "RETURN m.name ORDER BY m.name",
            {"name": name},
        ).result_set
    ]
    decorators: list[str] = []
    for row in client.query(
        "MATCH (f:Function {name: $name}) RETURN f.decorators LIMIT 1",
        {"name": name},
    ).result_set:
        if row[0]:
            decorators = row[0]

    result = {
        "name": name,
        "definitions": defs,
        "direct_callers": callers,
        "string_literal_references": refs,
        "methods_of_class": methods_of,
        "decorators": decorators,
    }

    if as_json:
        click.echo(_json.dumps(result, indent=2))
        return

    if not defs:
        click.echo(f"'{name}' not found in graph.")
        return
    click.echo(f"Found {len(defs)} definition(s):")
    for d in defs:
        click.echo(f"  [{d['type']}] {d['name']}  ({d['file']})")
    click.echo(f"\nDirect callers ({len(callers)}):")
    for c in callers:
        click.echo(f"  {c['name']}  ({c['file']})")
    click.echo(f"\nString-literal references ({len(refs)}):")
    for r in refs:
        click.echo(f"  {r['name']}  ({r['file']})")
    if methods_of:
        click.echo(f"\nMethods ({len(methods_of)}): {', '.join(methods_of[:15])}")
        if len(methods_of) > 15:
            click.echo(f"  ... and {len(methods_of) - 15} more")
    if decorators:
        click.echo(f"\nDecorators: {decorators}")
    if not callers and not refs and not methods_of:
        click.echo("\n(No structural or string-literal references — likely dead code or external entry point.)")


# --- savants up: the 60-second experience ----------------------------------


def _detect_k8s_clusters() -> list[dict]:
    """Auto-detect available kubeconfig contexts."""
    clusters = []
    try:
        from kubernetes import config as k8s_config
        contexts, active = k8s_config.list_kube_config_contexts()
        for ctx in (contexts or []):
            name = ctx.get("name", "")
            cluster = ctx.get("context", {}).get("cluster", name)
            clusters.append({"context": name, "cluster": cluster})
    except Exception:
        pass
    return clusters


def _detect_docker() -> bool:
    """Check if Docker is available."""
    import shutil
    return shutil.which("docker") is not None


def _detect_systemd() -> bool:
    """Check if systemd is running."""
    import shutil
    return shutil.which("systemctl") is not None


@cli.command()
@click.option("--repo", type=click.Path(exists=True, file_okay=False), default=None,
              help="Path to a git repo to index (defaults to cwd if it's a repo)")
@click.option("--tail-lines", default=200, type=int,
              help="Lines of log history to replay per pod")
def up(repo: str | None, tail_lines: int):
    """Start Savants: auto-detect infrastructure and show what's wrong.

    This is the 60-second experience. Savants starts the embedded graph,
    discovers your infrastructure (K8s clusters, Docker, systemd, the
    current repo), ingests everything, and shows you what's broken.

    \b
    Example:
        savants up
        savants up --repo /path/to/my/project
    """
    import os
    from pathlib import Path

    click.echo("Starting Savants...\n")

    # 1. Ensure graph is running
    client = _ensure_falkordb()
    click.echo("[graph] Connected.\n")

    # 2. Auto-detect infrastructure
    click.echo("Detecting infrastructure...")

    # K8s clusters
    clusters = _detect_k8s_clusters()
    if clusters:
        for c in clusters:
            click.echo(f"  Found K8s cluster: {c['cluster']} (context: {c['context']})")
    else:
        click.echo("  No K8s clusters found")

    # Docker
    has_docker = _detect_docker()
    if has_docker:
        click.echo("  Found Docker")

    # Systemd
    has_systemd = _detect_systemd()
    if has_systemd:
        click.echo("  Found systemd")

    # Repo
    if repo is None:
        cwd = os.getcwd()
        if Path(cwd, ".git").exists():
            repo = cwd
    if repo:
        click.echo(f"  Found git repo: {repo}")

    click.echo("")

    issues_found = []

    # 3. Ingest host
    click.echo("[host] Ingesting local machine...")
    try:
        from savants.host.ingestor import HostIngestor
        host_ing = HostIngestor(graph_client=client)
        host_stats = host_ing.snapshot()
        click.echo(f"  {host_stats.disks} disks, {host_stats.interfaces} interfaces, "
                   f"{host_stats.systemd_units} systemd units "
                   f"({host_stats.failed_units} failed), "
                   f"{host_stats.journal_events} journal events")
        if host_stats.failed_units > 0:
            issues_found.append(f"{host_stats.failed_units} failed systemd units")
        if host_stats.journal_events > 0:
            issues_found.append(f"{host_stats.journal_events} journal error patterns")
    except Exception as e:
        click.echo(f"  Error: {e}", err=True)

    # 4. Ingest K8s clusters
    for c in clusters:
        cluster_name = c["cluster"]
        graph_name = cluster_name.replace("-", "_")
        click.echo(f"\n[k8s] Ingesting cluster '{cluster_name}'...")
        try:
            from savants.config import FalkorDBConfig
            from savants.k8s.ingestor import K8sIngestor
            k8s_client = GraphClient(FalkorDBConfig(
                host=client._config.host,
                port=client._config.port,
                graph_name=graph_name,
            ))
            k8s_ing = K8sIngestor(
                graph_client=k8s_client,
                cluster_name=cluster_name,
                kube_context=c["context"],
            )
            k8s_stats = k8s_ing.snapshot()
            # Count pods by status
            r = k8s_client.query(
                "MATCH (p:K8sPod) RETURN p.status, count(p) ORDER BY count(p) DESC", {}
            )
            status_line = ", ".join(
                f"{row[1]} {row[0]}" for row in r.result_set
            )
            click.echo(f"  Pods: {status_line}")

            # Check for CrashLoopBackOff
            crash_pods = [
                row for row in r.result_set if row[0] == "CrashLoopBackOff"
            ]
            if crash_pods:
                issues_found.append(
                    f"{crash_pods[0][1]} pods in CrashLoopBackOff on {cluster_name}"
                )

            # Quick log ingest for immediate diagnosis
            click.echo(f"  Tailing logs ({tail_lines} lines/pod)...")
            try:
                from savants.k8s.log_watcher import LogWatcher
                import time

                k8s_ing._load_k8s_client()
                core = k8s_ing._k8s_client["core"]
                lw = LogWatcher(
                    graph=k8s_client, cluster=cluster_name,
                    core_api=core, flush_interval_seconds=2.0,
                    tail_lines=tail_lines,
                )
                pods = core.list_pod_for_all_namespaces().items
                running = [p for p in pods if p.status and p.status.phase == "Running"]
                for p in running:
                    lw.add_pod(p.metadata.name, p.metadata.namespace)
                time.sleep(min(15, max(5, len(running) // 5)))
                lw.flush_all()
                lw.stop()

                r = k8s_client.query(
                    "MATCH (e:LogEvent) WHERE e.severity IN ['ERROR','FATAL'] "
                    "RETURN count(DISTINCT e.pod), count(e), sum(e.count)", {}
                )
                if r.result_set and r.result_set[0][0] > 0:
                    n_pods, n_templates, n_occur = r.result_set[0]
                    click.echo(f"  Log intelligence: {int(n_templates)} error templates "
                              f"from {int(n_pods)} pods ({int(n_occur or 0)} occurrences)")
                    issues_found.append(
                        f"{int(n_templates)} error patterns across {int(n_pods)} pods on {cluster_name}"
                    )
                else:
                    click.echo("  Log intelligence: no errors detected")
            except Exception as e:
                click.echo(f"  Log tailing skipped: {e}", err=True)

        except Exception as e:
            click.echo(f"  Error: {e}", err=True)

    # 5. Index code repo
    if repo:
        click.echo(f"\n[code] Indexing {repo}...")
        try:
            builder = CodePropertyGraphBuilder(repo_path=repo, client=client)
            stats = builder.build()
            click.echo(f"  {stats['files']} files, {stats['functions']} functions, "
                      f"{stats['classes']} classes, {stats['edges']} edges")
        except Exception as e:
            click.echo(f"  Error: {e}", err=True)

    # 6. Summary
    click.echo(f"\n{'='*60}")
    if issues_found:
        click.echo(f"Found {len(issues_found)} issue(s):\n")
        for i, issue in enumerate(issues_found, 1):
            click.echo(f"  {i}. {issue}")
        click.echo(f"\nRun `savants story` for full diagnosis.")
    else:
        click.echo("No issues detected. Your infrastructure looks healthy.")
    click.echo(f"{'='*60}")
    click.echo("\nSavants is ready. Use the MCP tools from your AI assistant,")
    click.echo("or run `savants k8s watch <cluster>` for live monitoring.")
