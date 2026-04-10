# Federated Graph Architecture: Web of Sovereign Mazkir Instances

**Status:** Settled. Decided 2026-04-08. Do not re-litigate.

This document defines how multiple Mazkir instances connect to form a
queryable web of structural knowledge across an entire engineering org.
Mazkir is **not** a single mega-graph. It is a federation of sovereign
graphs that each own a scope, expose an MCP endpoint, and can be
queried individually or via a routing layer that joins across them.

---

## The pitch in one sentence

> **Multiple sovereign Mazkir instances (one per scope) each expose an
> MCP endpoint, a federation server routes queries across them, and
> Claude talks to the federation server as if it were one graph.**

That's the entire architecture. The rest of this document explains why
that's the right model and how it actually works in practice.

---

## Why federation, not one mega-graph

Three reasons. Each one alone would be enough; together they make the
case overwhelming.

### 1. Sovereignty matches reality

Code lives in repos owned by teams. Runtime state lives in clusters
owned by SREs. Git history lives in GitHub/GitLab. Slack messages live
in Slack. Each of these has:

- A different owner
- A different update cadence
- A different access policy
- A different security boundary

Trying to put them all in one graph fights the natural ownership lines.
Federation respects them. Each scope owns its graph. Each scope decides
who can query it. Cross-scope queries happen at the federation layer,
where access control can be enforced explicitly.

### 2. Scale isn't the constraint, isolation is

The math says the entire graph for a 200-engineer org fits in <1 GB
(see `runtime-layer-retention-and-gc.md`). Even at Google scale,
everything fits in a single beefy server. Performance is not why you
federate.

You federate for **boundaries**:

- **Security**: production cluster's runtime data doesn't leak to dev's
  code graph
- **Blast radius**: if one graph gets corrupted or has a bad index, only
  that scope is affected
- **Team autonomy**: one team can update their graph schema independently
  of another team's schedule
- **Compliance**: customer A's enterprise tier graph is in customer A's
  VPC, customer B's is in customer B's VPC, never the twain shall meet

### 3. MCP is already a federation protocol

MCP was designed as "a stdio protocol for LLMs to query a knowledge
source." It doesn't care if there's one source or fifty. A Claude Code
session can connect to multiple MCP servers simultaneously, and
historically does (Slack MCP, GitHub MCP, custom MCPs). Mazkir just
becomes one more — except behind the scenes, *one* MCP endpoint can
route to *many* underlying graphs.

This is exactly how Glean federates Drive + GitHub + Jira + Slack
indexes internally. They have separate indexes for each source and a
federation layer that joins them at query time. Mazkir is the same
pattern applied to engineering state.

---

## The graph topology

A typical mid-sized org's Mazkir deployment looks like this:

```
                  ┌──────────────────────────────────┐
                  │   Mazkir Federation Server       │
                  │   (cloud or self-hosted)         │
                  │                                  │
                  │   Holds:                         │
                  │   - Registry of all graphs       │
                  │   - Access control               │
                  │   - Query router + joiner        │
                  │   - Single MCP endpoint that     │
                  │     Claude/Cursor connect to     │
                  └─────┬────────────────────────┬───┘
                        │                        │
            ┌───────────┼────────┬───────────────┼─────────────┐
            │           │        │               │             │
            ▼           ▼        ▼               ▼             ▼
   ┌──────────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐
   │ Code graph   │ │ Code     │ │ Runtime  │ │ Runtime  │ │ Slack    │
   │ for monorepo │ │ graph    │ │ graph    │ │ graph    │ │ graph    │
   │              │ │ for      │ │ for      │ │ for      │ │ (later)  │
   │ ~600MB,      │ │ frontend │ │ prod-eu  │ │ prod-us  │ │          │
   │ Layer 1+2    │ │ repo     │ │ cluster  │ │ cluster  │ │          │
   └──────────────┘ └──────────┘ └──────────┘ └──────────┘ └──────────┘
   1 instance       1 instance   1 operator   1 operator   1 webhook
                                 in K8s       in K8s       receiver
   ↑                ↑            ↑            ↑            ↑
   │                │            │            │            │
   each is a Mazkir node — own FalkorDB, own MCP endpoint, own retention,
   own access policy, own update cadence
```

Each box is a sovereign Mazkir instance. Each one runs the same
Mazkir software (same parser, same MCP server, same schema). The only
difference is what data it holds and where it lives.

---

## What lives where

| Scope | What it holds | Where it runs | Update source |
|---|---|---|---|
| **Code graph** (per repo) | Layer 1 (functions, classes, configs, env vars) + Layer 2 (commit history) | One Mazkir instance per repo, deployed centrally or per-team | CI on every push, or `savants hooks` post-commit |
| **Runtime graph** (per cluster) | Layer 4 (Deployments, Pods, Images, ConfigMaps, deploy Episodes, incident Episodes) | Mazkir K8s operator inside each cluster, with a sidecar FalkorDB | K8s watch API + PagerDuty webhooks |
| **History graph** (per repo) | Layer 2 (commits, episodes, PR-level metadata) | Co-located with the code graph for that repo | Git log walker, GitHub webhooks |
| **Knowledge graph** (org-wide, future) | Slack, Linear, Jira, Notion, meeting transcripts | One central Mazkir instance | OAuth integrations + webhooks |

Each Mazkir instance:

1. Owns its own FalkorDB (or shared multi-tenant FalkorDB with strict
   namespace isolation)
2. Exposes the **same MCP tool surface** (same `function_xray`,
   `find_references_structured`, etc.)
3. Tags every node with a `source_id` indicating which scope it belongs to
4. Knows nothing about the other graphs — federation happens above it

---

## How a federated query actually works

Suppose Claude asks:

> *"What's the blast radius of changing `payment_handler`?"*

Here's the request flow:

```
Claude Code
    │
    │  MCP call: function_xray("payment_handler")
    ▼
┌───────────────────────────┐
│ Federation Server         │
│                           │
│ Step 1: Identify scope    │
│ → "payment_handler is in  │
│    the backend monorepo"  │
│                           │
│ Step 2: Query code graph  │
└────┬──────────────────────┘
     │
     │ MCP call to backend code graph
     ▼
┌───────────────────────────┐
│ Code Graph (backend repo) │
│                           │
│ Returns: callers, callees,│
│ decorators, file path     │
└────┬──────────────────────┘
     │
     │ ↑ result returned to federation server
     │
     ▼
┌───────────────────────────┐
│ Federation Server         │
│                           │
│ Step 3: For each affected │
│ service, query runtime    │
│ graphs in parallel        │
└────┬───────────┬──────────┘
     │           │
     ▼           ▼
┌─────────┐ ┌─────────┐
│ prod-eu │ │ prod-us │
│ runtime │ │ runtime │
│ graph   │ │ graph   │
└─────────┘ └─────────┘
     │           │
     │ ↑ deployment state, liveness, oncall
     │           │
     ▼           ▼
┌───────────────────────────┐
│ Federation Server         │
│                           │
│ Step 4: Join all results  │
│ into a single fact pack   │
└────┬──────────────────────┘
     │
     │ ↑ unified response to Claude
     ▼
Claude Code
```

From Claude's perspective, this looks like a single MCP call that
returned a single response. From inside Mazkir, it was actually 3-5
queries against 3-5 different graphs, joined by the federation layer.

**The federation server is the only thing Claude talks to.** It:

1. Knows the registry of all underlying graphs
2. Routes incoming queries to the right one(s) based on scope
3. Issues MCP calls to underlying graphs in parallel
4. Joins results using stable node identifiers
5. Returns a single unified response in the same MCP shape Claude expects

The underlying graphs **never talk to each other directly**. They're
sovereign and isolated. Only the federation layer knows about more than
one of them.

---

## Stable node identifiers (the federation glue)

The thing that makes federation work is that **node identifiers are
stable across graphs.** A Function node in the code graph has the same
ID format as the Function reference in the runtime graph's deploy
Episode.

The format is: `{label}:{scope}:{path}:{name}`

Examples:

- `Function:backend:src/payments/handlers.py:payment_handler`
- `Deployment:prod-eu:payments-api`
- `Episode:backend:commit:abc123`
- `EnvVar:backend:DATABASE_URL`

When the federation server joins results from two graphs, it matches on
these stable IDs. The runtime graph in `prod-eu` has a `Deployment`
node with a property `runs_function_id =
"Function:backend:src/payments/handlers.py:payment_handler"` — the
federation server uses that ID to fetch the structural details from
the backend code graph.

This is the only thing the graphs need to agree on. Schema can evolve
independently per scope. Update cadence can vary. Storage backend can
vary. The only invariant is the ID format.

---

## Deployment model: how this actually runs

### Solo developer (free local tier)

One Mazkir instance, one FalkorDB sidecar, one MCP endpoint. No
federation needed because there's only one scope. Same as today.

```
Developer laptop:
  └─ Mazkir + FalkorDB + MCP endpoint
       (queried by Claude Code locally)
```

### Small team (paid Team tier)

A central cloud-hosted Mazkir holds the code graph for all the team's
repos. Optionally, a Mazkir K8s operator runs in their staging cluster
to feed runtime data. Federation server lives in the cloud with a
single MCP endpoint per team.

```
Cloud:
  ├─ Federation server (1)
  └─ Code graph (1, multi-repo)

Team's K8s cluster:
  └─ Mazkir operator + FalkorDB sidecar (1, runtime layer)

→ All federated through the cloud server's MCP endpoint
```

### Mid-sized company (Business tier)

Multiple code graphs (one per major repo or team), multiple runtime
graphs (one per cluster), an org-wide knowledge graph for Slack/Jira,
all federated by a central server.

```
Cloud (or customer VPC):
  ├─ Federation server (1)
  ├─ Code graph: monorepo
  ├─ Code graph: ml-platform
  ├─ Code graph: data-pipeline
  ├─ Runtime graph reference: prod-eu (federated to in-cluster operator)
  ├─ Runtime graph reference: prod-us (same)
  ├─ Runtime graph reference: staging (same)
  └─ Knowledge graph: slack + linear + notion

Each customer K8s cluster:
  └─ Mazkir operator + FalkorDB sidecar (1 per cluster)
```

### Enterprise / regulated (Enterprise tier)

Self-hosted in customer VPC. Same architecture, different deployment
location. Federation server inside their network. Cluster operators in
each cluster. Optional TEE (Confidential Compute) for compliance.

```
Customer VPC:
  ├─ Federation server (1, inside their network)
  ├─ Code graphs (N, one per repo or team)
  ├─ Runtime graph operators (M, one per cluster)
  └─ All wired together via their internal DNS, never touches public internet
```

The software is the same. The deployment topology changes.

---

## Avoiding issues (the user's actual question)

The user asked: "would this be the way to link everything together to
avoid issues?" Yes, and here's specifically what issues federation
avoids:

### Issue 1: Cross-team blocking

In a single mega-graph, if Team A's repo indexer crashes, queries for
Team B's repo also degrade. In federation, each graph is independent —
Team A's failure is invisible to Team B.

### Issue 2: Schema lock-in

In a single mega-graph, every team has to agree on the schema. Adding
a new node type requires coordination across the whole org. In
federation, each scope evolves its schema independently as long as the
ID format stays compatible.

### Issue 3: Security boundary violations

In a single mega-graph, production secrets and dev metadata sit in the
same database. RBAC has to be enforced by every query. In federation,
production runtime data lives in its own graph inside the prod cluster
— nothing in dev can even reach it without explicit federation rules.

### Issue 4: Single point of failure

In a single mega-graph, the database going down breaks everything. In
federation, only the affected scope is unavailable; queries that don't
need it still work.

### Issue 5: Stale data masquerading as fresh

Each graph has its own update cadence: code graphs update on git push,
runtime graphs update on K8s watch events, knowledge graphs update on
webhook. The federation server can tell Claude "the code graph is 30s
fresh, the runtime graph is 2s fresh" so the agent can make decisions
based on freshness. In a single mega-graph, you'd just have one
"freshness" number that's always pessimistic.

### Issue 6: Query amplification

In a single mega-graph, a careless deep traversal can pull half the
database into memory. In federation, each underlying graph is small
enough that even a worst-case traversal is bounded. The federation
server can cap join cardinality at the routing layer.

### Issue 7: Compliance / data residency

EU customers need their data in EU. Healthcare customers need their
data on-prem. In federation, you just put the relevant graph instances
where they need to be. The federation server routes based on scope and
data residency rules.

---

## What the federation server actually does

It's a small piece of code with a clear contract:

```python
class FederationServer:
    """The single MCP endpoint Claude talks to.

    Holds a registry of underlying Mazkir graphs and routes incoming
    queries to the right one(s), joining results using stable node IDs.
    """

    def __init__(self, registry: GraphRegistry):
        self.registry = registry  # Maps scope_id → MCP endpoint URL
        self.access_control = AccessControl()

    def handle_tool_call(self, tool: str, args: dict, caller_identity: str) -> dict:
        # 1. Identify which graphs to query
        scopes = self.identify_relevant_scopes(tool, args)

        # 2. Enforce access control per scope
        scopes = [s for s in scopes if self.access_control.allow(caller_identity, s)]

        # 3. Issue parallel MCP calls to each scope
        results = parallel([
            self.registry.get(scope).call(tool, args)
            for scope in scopes
        ])

        # 4. Join results using stable node IDs
        joined = self.join_results(results, tool)

        # 5. Return as if it were one graph
        return joined
```

That's the whole core logic. The complexity lives in:

- **The registry** (which graphs exist, how to reach them, what scopes
  they cover)
- **Scope identification** (given a query, which graphs need to be hit)
- **Access control** (which caller can see which scope)
- **Result joining** (how to merge results from multiple graphs into one
  fact pack)

None of these are individually hard. Together they're maybe a week of
focused work for the MVP.

---

## The MCP tool surface stays identical

Critically, **the MCP tools that the federation server exposes are
the same tools each underlying graph exposes.** A user calling
`function_xray("payment_handler")` against the federation server gets
the same shape of response as calling it against a single graph
directly. The difference is that the federation version's response
*includes data from runtime graphs* without the user having to ask
for it.

This means:

- **No new tools to learn** when an org adopts federation
- **Local solo dev tier and federated enterprise tier expose the same
  API** — just with more data behind the same calls
- **Customers can develop against the local tier and seamlessly upgrade
  to the federated cloud tier** without changing any agent code
- **Documentation is unified** — "function_xray returns these fields"
  is the same statement at every tier

---

## Where this fits in the roadmap

Federation is **not** a Day 1 feature. It's a Year 1+ feature, after
the local tier and the basic cloud tier are shipped. The order:

1. **Months 1-2**: Local tier solid (where we are now). Single graph,
   single MCP endpoint.
2. **Months 3-4**: Cloud tier MVP. Hosted Mazkir + webhook for
   GitHub/GitLab. Still single-graph from the user's perspective.
3. **Months 5-6**: Multi-repo support in the cloud tier. Multiple code
   graphs but still presented as one MCP endpoint via a simple
   federation server.
4. **Months 7-9**: K8s operator + runtime graph. Federation server
   joins code + runtime.
5. **Months 10-12**: Knowledge graph integrations (Slack, Jira, Linear)
   added as additional federation scopes.

Each step adds one more underlying graph behind the same federation
server. The federation server's contract doesn't change as new graphs
are added — only its registry grows.

---

## Why this is the right architecture (one paragraph)

Federation respects the natural sovereignty boundaries of code, runtime,
and knowledge — each lives in its own scope, owned by different teams,
updated on different cadences, secured under different policies — while
still presenting a single MCP endpoint to AI agents that want to query
across all of them. It avoids every failure mode of the single
mega-graph approach (cross-team blocking, schema lock-in, security
sprawl, single point of failure) without sacrificing the unified
query experience that makes the product valuable. It's the same pattern
that Glean uses for documents and Datadog uses for service maps, applied
to engineering structural state. **Build the local tier first. Build
the federation server last. The contract between them — the MCP tool
surface and the stable node ID format — is what we have to get right
on day one so the federation server can be added later without breaking
anyone's existing integration.**
