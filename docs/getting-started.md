# Getting Started with Savants

Your infrastructure savant. Know what's wrong in 60 seconds.

## Install

```bash
curl -fsSL savants.sh | sh
```

Or with pip:

```bash
pip install savants
```

## Quick start: `savants up`

One command discovers your infrastructure and tells you what's broken:

```bash
savants up
```

Savants will:
1. Start the embedded graph database (first run only)
2. Auto-detect K8s clusters, Docker, systemd, and git repos
3. Ingest everything: cluster state, pod logs, host metrics, code graph
4. Show a summary of issues found

```
Starting Savants...

Detecting infrastructure...
  Found K8s cluster: production (94 pods)
  Found Docker, systemd
  Found git repo: ./my-service

[host] 534 systemd units (4 failed), 42 journal events
[k8s]  61 Running, 33 Succeeded
[k8s]  Log intelligence: 48 error templates from 16 pods
[code] 500 functions, 159 classes, 1903 edges

Found 3 issue(s):
  1. 4 failed systemd units
  2. 42 journal error patterns
  3. 48 error patterns across 16 pods

Run `savants story` for full diagnosis.
```

## Full diagnosis: `savants story`

Get a detailed narrative of everything that's wrong:

```bash
savants story                          # last 60 minutes
savants story --since-minutes 0        # all time
savants story --min-severity ERROR     # errors only
savants story --cluster production     # one cluster
```

## Live monitoring: `savants k8s watch`

Keep the graph updated in real-time with K8s watch streams:

```bash
savants k8s watch my-cluster --logs --tail-lines 500
```

This runs until you Ctrl-C and:
- Holds watch connections to the K8s API (1.3s propagation delay)
- Tails every pod's logs through the significance pipeline
- Refreshes host metrics every 30 seconds
- Creates CAUSED_BY edges when log errors follow config changes

## AI integration: MCP

Savants works as an MCP server for Claude Code, Cursor, and any MCP-compatible AI tool.

### Setup (one command)

```bash
# Project-level (writes .mcp.json)
savants mcp install

# Global (registers with Claude Code for all projects)
savants mcp install --scope user

# For Cursor
savants mcp install --tool cursor
```

### Using with your AI

After setup, restart your AI tool and ask:

- "What's wrong with my cluster?"
- "Show me the pod story for production"
- "What's the blast radius of my last commit?"
- "What functions call `authenticate`?"
- "What config does the api-gateway pod depend on?"

Available tools (26):
- **pod_story** — what's wrong with a pod or cluster
- **host_state** — CPU, memory, disk, failed services
- **host_story** — what's wrong with the host
- **cluster_state** — full cluster overview
- **list_pods** — filter pods by status, namespace, name
- **deployment_info** — replica status, image, labels
- **pod_dependencies** — configmaps and secrets a pod reads
- **namespace_summary** — everything in a namespace
- **diff_impact** — blast radius of a code change
- **function_xray** — full structural profile of a function
- **find_references** — who calls this function?
- **impact_analysis** — cascading dependents
- **search_code** — find functions/classes by name
- **risk_score** — how risky is this function to change?
- **decorated_with** — find all functions with a decorator
- **resolves_to** — trace a string literal to its implementation
- **co_change_partners** — what changes together?
- **community_summary** — hub files in the codebase
- **dependency_chain** — shortest path between files
- **recall_history** — what happened to this code?
- **federated_symbol_in_cluster** — find code symbols in K8s
- **pre_change_warning** — safety check before editing
- **coupling_check** — hidden coupling between modules
- **advanced_graph_query** — raw Cypher queries
- **graph_stats** — node and edge counts
- **reindex** — rebuild the code graph

## Other commands

```bash
# Index a specific repo
savants init /path/to/repo

# Incremental re-index
savants index /path/to/repo

# One-shot cluster snapshot (no live watching)
savants k8s snapshot my-cluster

# One-shot host snapshot
savants host snapshot

# Generate a shareable report
savants report > diagnosis.md

# Check what's configured
savants mcp status
savants status

# Structural analysis
savants impact my_function
savants diff-impact HEAD~3..HEAD
savants search "authenticate"
savants ask my_function
```

## Architecture

Savants builds a multi-layer knowledge graph:

```
Layer 1: Code          Functions, classes, imports, call graphs, config keys
Layer 2: History       Git commits, co-change partners, who-touched-what
Layer 3: K8s State     Pods, deployments, services, configmaps, secrets
Layer 4: Log Events    Deduplicated error templates from pod logs
Layer 5: Host          CPU, memory, disk, systemd, Docker, kernel events
Layer 6: Edges         MENTIONS, CAUSED_BY, READS, EMITTED — the cross-layer links
```

Every layer is connected by typed edges. The value isn't in any single layer — it's in the cross-layer queries:

> "This pod crashed because this configmap was edited, which is read by this code path, changed in this commit by this person."

## Data & Privacy

- **No source code is stored.** The graph contains metadata only: function names, file paths, call relationships, config key names (never values), log templates (parameterized).
- **No data leaves your machine.** Everything runs locally. The graph is stored in `~/.savants/data/`.
- **Verifiable.** Run a packet capture while Savants operates — you'll see zero outbound connections.
- **~450x compression.** A 100MB codebase produces ~220KB of graph metadata.
