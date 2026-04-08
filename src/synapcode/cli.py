"""SynapCode CLI: the front door to the cognitive stack.

Usage:
    synapcode init /path/to/repo      # First-time index
    synapcode index /path/to/repo     # Re-index (incremental or full)
    synapcode query "what calls X?"   # Query the graph
    synapcode impact process_data     # Cascading impact analysis
    synapcode search "auth"           # Search functions/classes
    synapcode status                  # Health check
    synapcode gc /path/to/repo        # Garbage collection
    synapcode snapshot create /repo   # Create Git LFS snapshot
    synapcode snapshot restore /repo  # Restore from snapshot
    synapcode serve                   # Start MCP server
    synapcode worker                  # Start Temporal worker
"""

from __future__ import annotations

import sys

import click

from synapcode.config import load_config
from synapcode.graph.client import GraphClient
from synapcode.graph.cpg import CodePropertyGraphBuilder
from synapcode.graph.query import GraphQueryEngine
from synapcode.sync.git_hooks import get_current_head, get_last_indexed_sha, save_last_indexed_sha


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

    # Walk up from this file looking for desktop/src-tauri/binaries/falkordb.so
    from pathlib import Path
    here = Path(__file__).resolve()
    for parent in here.parents:
        bundled = parent / "desktop" / "src-tauri" / "binaries" / "falkordb.so"
        if bundled.exists():
            return str(bundled)
        if (parent / ".git").exists() or parent == parent.parent:
            break

    for p in (
        "/usr/lib/redis/modules/falkordb.so",
        "/usr/local/lib/redis/modules/falkordb.so",
        "/opt/homebrew/lib/redis/modules/falkordb.so",
        "/usr/lib/falkordb/falkordb.so",
    ):
        if os.path.exists(p):
            return p
    return None


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
    d = Path.home() / ".synapcode"
    d.mkdir(parents=True, exist_ok=True)
    return d / "falkordb.pid"


def _start_falkordb_process(port: int, verbose: bool = True) -> int | None:
    """Start redis-server + FalkorDB module. Returns PID on success."""
    import shutil
    import subprocess
    import time

    redis_bin = shutil.which("redis-server")
    if not redis_bin:
        if verbose:
            click.echo(
                "redis-server not found on PATH. Install Redis or use the desktop app.",
                err=True,
            )
        return None

    module = _find_falkordb_module()
    cmd = [redis_bin, "--port", str(port), "--daemonize", "no", "--save", ""]
    if module:
        cmd.extend(["--loadmodule", module])
    else:
        if verbose:
            click.echo(
                "Warning: FalkorDB module not found. Graph queries will fail. "
                "Set FALKORDB_MODULE=/path/to/falkordb.so.",
                err=True,
            )

    if verbose:
        click.echo(f"Starting FalkorDB on port {port}...")
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
                click.echo(f"FalkorDB started (PID {proc.pid}, port {port}).")
            return proc.pid
        except Exception:
            if proc.poll() is not None:
                if verbose:
                    click.echo("FalkorDB process exited during startup.", err=True)
                return None
            continue
    if verbose:
        click.echo("Timed out waiting for FalkorDB.", err=True)
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
        click.echo(f"Could not connect after starting FalkorDB: {e}", err=True)
        sys.exit(1)


@click.group()
@click.version_option(package_name="synapcode")
def cli():
    """SynapCode: local-first GraphRAG cognitive stack."""


@cli.command()
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
def init(repo_path: str):
    """First-time setup: create schema, full index, save bookmark."""
    client = _ensure_falkordb()

    click.echo(f"Indexing {repo_path}...")
    builder = CodePropertyGraphBuilder(repo_path=repo_path, client=client)
    stats = builder.build()

    # Save bookmark
    try:
        head = get_current_head(repo_path)
        save_last_indexed_sha(repo_path, head)
        click.echo(f"Bookmark saved at {head[:8]}")
    except Exception:
        click.echo("Warning: could not save git bookmark (not a git repo?)", err=True)

    click.echo(
        f"Done: {stats['files']} files, {stats['functions']} functions, "
        f"{stats['classes']} classes, {stats['edges']} edges"
    )


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
        from synapcode.sync.diff import compute_diff

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

    from synapcode.pipelines.graphrag_inlet import build_graph_context

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


@cli.command()
@click.argument("function_name")
@click.option("--depth", default=5, help="Maximum traversal depth")
def impact(function_name: str, depth: int):
    """Analyze cascading impact of changing a function."""
    client = _ensure_falkordb()
    engine = GraphQueryEngine(client)

    result = engine.impact_analysis(function_name, max_depth=depth)

    click.echo(f"Impact analysis for '{result.target}':")
    click.echo()

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
def search(pattern: str):
    """Search for functions and classes by name pattern."""
    client = _ensure_falkordb()
    engine = GraphQueryEngine(client)

    results = engine.search_by_pattern(pattern)
    if not results:
        click.echo(f"No matches for '{pattern}'")
        return

    click.echo(f"Found {len(results)} matches:")
    for r in results:
        click.echo(f"  {r['type']:10} {r['name']:30} {r.get('file', '')}")


@cli.command()
def status():
    """Show graph stats, last indexed commit, service health."""
    config = load_config()

    # FalkorDB health
    click.echo("FalkorDB:")
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
    from synapcode.hardware.monitor import get_system_stats

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
    from synapcode.graph.gc import GraphGarbageCollector

    client = _ensure_falkordb()
    collector = GraphGarbageCollector(client)
    report = collector.run_full_gc(repo_path)

    click.echo(f"GC complete ({report.duration_ms:.0f}ms):")
    click.echo(f"  Orphan nodes removed:     {report.orphan_nodes_removed}")
    click.echo(f"  Stale files removed:       {report.stale_files_removed}")
    click.echo(f"  Expired facts removed:     {report.expired_facts_removed}")
    click.echo(f"  Contradictions resolved:   {report.contradictions_resolved}")


@cli.group()
def snapshot():
    """Create or restore graph snapshots for team bootstrapping."""


@snapshot.command("create")
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
def snapshot_create(repo_path: str):
    """Serialize the graph to a file for Git LFS."""
    from synapcode.sync.bootstrap import create_snapshot

    path = create_snapshot(repo_path)
    click.echo(f"Snapshot saved to {path}")
    click.echo("Track it with: git lfs track '.synapcode/*.dump'")


@snapshot.command("restore")
@click.argument("repo_path", type=click.Path(exists=True, file_okay=False))
def snapshot_restore(repo_path: str):
    """Restore a graph from a Git LFS snapshot."""
    from synapcode.sync.bootstrap import restore_snapshot

    if restore_snapshot(repo_path):
        client = _ensure_falkordb()
        click.echo(f"Graph restored: {client.node_count()} nodes, {client.edge_count()} edges")
    else:
        click.echo("No snapshot found. Run 'synapcode snapshot create' first.", err=True)
        sys.exit(1)


@cli.command()
def serve():
    """Start the MCP server (for Claude Code / Cursor integration)."""
    from synapcode.mcp.server import main as mcp_main

    mcp_main()


@cli.command()
def worker():
    """Start the Temporal worker for durable workflow execution."""
    import asyncio

    from synapcode.temporal.worker import main as worker_main

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
    """Stop the FalkorDB sidecar started by `synapcode falkordb start`."""
    import os
    import signal

    pidfile = _falkordb_pidfile()
    if not pidfile.exists():
        click.echo("No pidfile — not started via synapcode, or already stopped.")
        return
    pid = int(pidfile.read_text().strip())
    try:
        os.kill(pid, signal.SIGTERM)
        click.echo(f"Sent SIGTERM to {pid}.")
    except ProcessLookupError:
        click.echo("Process already gone.")
    pidfile.unlink(missing_ok=True)


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
def ask(name: str):
    """One-shot structural trace for a symbol: callers, references, decorators."""
    client = _ensure_falkordb()

    # 1. Locate it
    rows = client.query(
        "MATCH (n) WHERE (n:Function OR n:Class) AND n.name = $name "
        "RETURN labels(n)[0], n.name, n.file_path LIMIT 10",
        {"name": name},
    ).result_set
    if not rows:
        click.echo(f"'{name}' not found in graph.")
        return
    click.echo(f"Found {len(rows)} definition(s):")
    for r in rows:
        click.echo(f"  [{r[0]}] {r[1]}  ({r[2]})")

    # 2. Structural callers (CALLS)
    callers = client.query(
        "MATCH (c:Function)-[:CALLS]->(t:Function {name: $name}) "
        "RETURN DISTINCT c.name, c.file_path LIMIT 20",
        {"name": name},
    ).result_set
    click.echo(f"\nDirect callers ({len(callers)}):")
    for c in callers:
        click.echo(f"  {c[0]}  ({c[1]})")

    # 3. String-literal references (REFERENCES_SYMBOL)
    refs = client.query(
        "MATCH (c:Function)-[:REFERENCES_SYMBOL]->(t) WHERE t.name = $name "
        "RETURN DISTINCT c.name, c.file_path LIMIT 20",
        {"name": name},
    ).result_set
    click.echo(f"\nString-literal references ({len(refs)}):")
    for c in refs:
        click.echo(f"  {c[0]}  ({c[1]})")

    # 4. Decorators on the symbol itself (if Function)
    decs = client.query(
        "MATCH (f:Function {name: $name}) RETURN f.decorators LIMIT 5",
        {"name": name},
    ).result_set
    for row in decs:
        if row[0]:
            click.echo(f"\nDecorators on {name}: {row[0]}")

    if not callers and not refs:
        click.echo("\n(No structural or string-literal references — likely dead code or external entry point.)")
