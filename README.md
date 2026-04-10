# SynapCode

A local-first AI cognitive stack that leverages **FalkorDB** for GraphRAG, **Temporal** for durable workflow execution, and **Open WebUI Pipelines** for intelligent model routing. Designed to run on consumer hardware (MacBook Pro / Framework Laptop) while offering seamless cloud bursting when local resources are exhausted.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Open WebUI (Interface)                     │
│                 Pipelines (Routing Middleware)                │
│         ┌──────────────┐    ┌───────────────────┐           │
│         │  GraphRAG     │    │  Selective Deferred│           │
│         │  Inlet Filter │    │  Router (Manifold) │           │
│         └──────┬───────┘    └────────┬──────────┘           │
└────────────────┼─────────────────────┼──────────────────────┘
                 │                     │
    ┌────────────▼────────┐   ┌───────▼──────────┐
    │      FalkorDB        │   │   Local SLM /     │
    │  (Code Property      │   │   Frontier API    │
    │   Graph + Memory)    │   │   (Dynamic Route) │
    └────────────┬────────┘   └──────────────────┘
                 │
    ┌────────────▼────────────────────────────────┐
    │              Temporal                         │
    │  (Durable Workflows + Activities)            │
    │  ┌──────────┐  ┌───────────┐  ┌──────────┐ │
    │  │ Indexing  │  │ Sync      │  │ Agent    │ │
    │  │ Workflow  │  │ Workflow  │  │ Workflow │ │
    │  └──────────┘  └───────────┘  └──────────┘ │
    └─────────────────────────────────────────────┘
```

## Core Components

| Component | Technology | Purpose |
|-----------|-----------|---------|
| Structural Memory | FalkorDB + Graphiti | Code Property Graph, multi-hop GraphRAG, episodic memory |
| Orchestration | Temporal + PydanticAI | Durable agent execution, crash-proof workflows |
| Interface | Open WebUI Pipelines | Model routing, context injection, user interface |
| Sync | Git Hooks + Git LFS | Incremental graph updates, team bootstrapping |
| Security | BYOK Envelope Encryption | Data sovereignty, cryptographic kill-switch |
| Hardware Awareness | System Monitor | 60% RAM rule, cloud bursting triggers |

## Quick Start

### Option A: Desktop App (recommended — zero config)

Download the SynapCode desktop app. It manages FalkorDB, Temporal, and the
worker automatically as native sidecars. No Docker, no manual setup.

```bash
cd desktop && npm install && npm run build
```

### Option B: CLI only

```bash
pip install -e ".[dev]"
```

### Index a Repository

```bash
savants init /path/to/your/repo
```

FalkorDB starts automatically if not already running (native `redis-server`,
no Docker required). The desktop app manages this for you.

### 4. Explore Your Codebase

```bash
# Search for functions/classes
savants search "authenticate"

# Cascading impact analysis
savants impact process_data

# Query with natural language
savants query "What functions call authenticate?"

# Check system health
savants status
```

### 5. Keep the Graph in Sync

```bash
# Install git hooks for automatic sync on pull
./scripts/install-hooks.sh /path/to/repo

# Or manually re-index after changes
savants index /path/to/repo
```

### 6. Connect to Claude Code / Cursor (MCP)

```bash
# Register as an MCP server
claude mcp add-json savants --scope user '{
  "command": "python",
  "args": ["-m", "savants.mcp"],
  "env": {"FALKORDB_HOST": "localhost", "FALKORDB_PORT": "6379"}
}'
```

### Full CLI Reference

```
savants init <repo>              First-time index + setup
savants index <repo> [--full]    Re-index (incremental by default)
savants query "<question>"       Query graph for structural context
savants impact <function_name>   Cascading impact analysis
savants search <pattern>         Search functions/classes by name
savants status                   Service health + graph stats
savants gc <repo>                Garbage collection
savants snapshot create <repo>   Serialize graph for Git LFS
savants snapshot restore <repo>  Restore from snapshot
savants serve                    Start MCP server
savants worker                   Start Temporal worker
```

### Programmatic Usage

```python
from savants.graph.client import GraphClient
from savants.graph.cpg import CodePropertyGraphBuilder
from savants.graph.query import GraphQueryEngine

# Index a repo
client = GraphClient()
builder = CodePropertyGraphBuilder(repo_path="/path/to/repo", client=client)
stats = builder.build()
print(f"Indexed {stats['files']} files, {stats['functions']} functions")

# Query the graph
engine = GraphQueryEngine(client)
impact = engine.impact_analysis("my_function", max_depth=5)
print(f"Affected files: {impact.affected_files}")
```

## Features

### GraphRAG with Code Property Graphs
- Indexes codebases into a graph of files, functions, classes, and their relationships
- Multi-hop reasoning: "What are the cascading impacts of changing this API?"
- Sub-140ms p99 query latency via FalkorDB's sparse matrix engine

### Durable Agent Execution
- Temporal workflows survive crashes, laptop sleep, and network timeouts
- Automatic retry with configurable backoff for LLM API calls
- Event-sourced state — no external state database needed

### Selective Deferred Routing
- Routes simple tasks to local SLMs (Ollama/LM Studio)
- Escalates complex reasoning to frontier models (Claude, GPT-4)
- Cuts compute costs by up to 70%

### Team Collaboration
- Git LFS bootstrapping for instant onboarding
- Incremental post-merge sync via git hooks
- SHA-256 provenance stamping on all graph entries

### BYOK Shielded Cloud
- Envelope encryption with user-provided KMS keys
- Cryptographic kill-switch: revoke access instantly
- Zero-knowledge storage for cloud-hosted graphs

### Hardware Awareness
- Real-time RAM/CPU monitoring
- Automatic cloud bursting when local resources hit 60% threshold
- FalkorDB cold-start in ~1.1ms for on-demand graph loading

## Project Structure

```
savants/
├── src/savants/
│   ├── graph/          # FalkorDB client, CPG builder, schema, queries
│   ├── temporal/       # Workflows, activities, worker
│   ├── pipelines/      # Open WebUI routing, GraphRAG inlet, manifold
│   ├── sync/           # Git hooks, LFS bootstrap, incremental updates
│   ├── security/       # BYOK encryption, provenance stamping
│   ├── hardware/       # System monitor, cloud burst triggers
│   └── mcp/            # Model Context Protocol server
├── scripts/            # Git hook installers
├── tests/              # Test suite
└── docs/               # Architecture documentation
```

## License

MIT
