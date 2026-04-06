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


def _ensure_falkordb() -> GraphClient:
    """Connect to FalkorDB, auto-starting it via Docker if needed."""
    import subprocess
    import time

    config = load_config()
    client = GraphClient(config.falkordb)

    try:
        client.ensure_schema()
        return client
    except Exception:
        pass  # Not running, try to start it

    click.echo("FalkorDB not running. Starting via Docker...")
    try:
        subprocess.run(
            ["docker", "run", "-d", "--name", "synapcode-falkordb",
             "-p", "6379:6379", "falkordb/falkordb:latest"],
            capture_output=True, check=True,
        )
    except subprocess.CalledProcessError:
        # Container might already exist but be stopped
        subprocess.run(
            ["docker", "start", "synapcode-falkordb"],
            capture_output=True,
        )

    # Wait for it
    for _ in range(20):
        time.sleep(0.5)
        try:
            client = GraphClient(config.falkordb)
            client.ensure_schema()
            click.echo("FalkorDB started.")
            return client
        except Exception:
            continue

    click.echo("Could not start FalkorDB. Install Docker or start it manually.", err=True)
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
