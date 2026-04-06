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

### Prerequisites

- Python 3.11+
- Docker & Docker Compose
- Git LFS

### 1. Start Infrastructure

```bash
docker compose up -d
```

This starts FalkorDB and Temporal Server locally.

### 2. Install Python Dependencies

```bash
pip install -e ".[dev]"
```

### 3. Start the Temporal Worker

```bash
python -m synapcode.temporal.worker
```

### 4. Index a Repository

```python
from synapcode.graph.cpg import CodePropertyGraphBuilder

builder = CodePropertyGraphBuilder(repo_path="/path/to/repo")
await builder.build()
```

### 5. Install Git Hooks (for incremental sync)

```bash
./scripts/install-hooks.sh /path/to/repo
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
synapcode/
├── src/synapcode/
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
