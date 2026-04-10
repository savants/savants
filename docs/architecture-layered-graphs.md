# Layered Graph Architecture

**Status:** Canonical design as of 2026-04-07
**Implementation status:** Specification phase. PoC delta computer in `src/savants/delta/`.

This document defines how SynapCode handles team collaboration, uncommitted changes, and arbitrary repo sizes via a three-layer graph composition. It is the architectural spine that justifies the cloud tiers in `BUSINESS.md`.

---

## The problem

A code intelligence tool for teams must answer two questions accurately:

1. **"What does main look like?"** — needs the authoritative shared state.
2. **"What does my refactor break?"** — needs the user's uncommitted changes, which only exist on their laptop.

Existing approaches all fail one or both:

| Approach | Problem |
|---|---|
| Pure local indexing | Doesn't scale past ~2,500 files (empirically measured); no team sharing. |
| Pure cloud indexing | Only sees committed state; user must push to ask "what does my change break?" |
| Re-index on every save | Wastes compute; query latency unacceptable; doesn't scale. |
| Send all files on every query | Bandwidth + privacy nightmare; defeats local-first pitch. |

The right answer is **layered graph composition**.

## The three layers

```
┌─────────────────────────────────────────────────────────┐
│ LAYER 3: Local Working Delta                             │
│   Where:    User's machine (memory only)                 │
│   Lifetime: Session (cleared on git commit / file save)  │
│   Size:     10-500 KB                                    │
│   Contains: Functions/classes/edges added or removed     │
│             since the latest committed state             │
│   Built by: Local tree-sitter + diff against base+overlay│
├─────────────────────────────────────────────────────────┤
│ LAYER 2: Branch Overlay                                  │
│   Where:    Cloud (per-branch storage)                   │
│   Lifetime: Until branch is merged or deleted            │
│   Size:     1 KB - 10 MB depending on branch divergence  │
│   Contains: Diff from main for this branch               │
│   Built by: Webhook on push → incremental indexer        │
├─────────────────────────────────────────────────────────┤
│ LAYER 1: Cloud Base                                      │
│   Where:    Cloud (FalkorDB cluster, possibly in TEE)    │
│   Lifetime: Permanent (rolling history of main)          │
│   Size:     Whatever the repo's main branch needs        │
│   Contains: Full graph for the authoritative branch      │
│   Built by: Webhook on merge to main → incremental index │
└─────────────────────────────────────────────────────────┘
```

## Composition rules

When a query runs, the engine composes the three layers in this order:

1. **Start with Cloud Base** — the graph for `main`
2. **Apply Branch Overlay** — for the current feature branch (if any)
3. **Apply Local Working Delta** — for uncommitted changes (if any)

**Composition semantics:**

- **Add operations** (`add_node`, `add_edge`) — append to the composed graph
- **Remove operations** (`remove_node`, `remove_edge`) — mask matching items from lower layers
- **Update operations** (`update_node`) — equivalent to remove + add for the changed properties

The composed graph exists only in memory during query execution. It is never persisted.

### Example

Say `main` defines:

```python
# src/auth/jwt.py (in main)
def authenticate(token):
    return token.startswith("eyJ")
```

Alice creates branch `alice/refactor-auth` and pushes one commit that **renames** `authenticate` to `verify_session`. The branch overlay records:

```json
{
  "remove_node": {"label": "Function", "name": "authenticate", "file": "src/auth/jwt.py"},
  "add_node":    {"label": "Function", "name": "verify_session", "file": "src/auth/jwt.py"}
}
```

Alice then edits her working copy further to add a new parameter `strict: bool`. Her local working delta is:

```json
{
  "update_node": {
    "label": "Function",
    "name": "verify_session",
    "file": "src/auth/jwt.py",
    "set": {"parameters": ["token", "strict"]}
  }
}
```

When Alice queries "what calls `verify_session`?":
- Cloud base has `authenticate` (with old callers)
- Branch overlay removes `authenticate`, adds `verify_session` (with same callers — they were already in main)
- Local delta updates the parameters

The composed graph shows `verify_session` with two parameters and the original caller set, exactly matching what Alice would see if she committed and pushed.

## Query flow

```
[Alice's MCP client]
  │ POST /v1/query
  │ {
  │   "org": "acme",
  │   "repo": "backend",
  │   "branch": "alice/refactor-auth",
  │   "session_id": "a1b2c3",
  │   "working_delta": <encrypted JSON, ~50KB>,
  │   "query": "MATCH (c:Function)-[:CALLS]->(t:Function {name: 'verify_session'}) RETURN c"
  │ }
  ▼
[Cloud edge gateway]
  │  ✓ Auth (JWT)
  │  ✓ Tenant routing
  │  ✓ Rate limiting
  │  → Forward encrypted request to TEE
  ▼
[TEE / Confidential compute]
  │  1. Verify attestation (client already verified ours during handshake)
  │  2. Decrypt working delta inside enclave
  │  3. Load cloud base for acme/backend (already in FalkorDB)
  │  4. Apply branch overlay for alice/refactor-auth
  │  5. Apply working delta on top
  │  6. Run Cypher query against the composed graph (in-memory virtual graph)
  │  7. Encrypt result for Alice
  │  8. Wipe plaintext from enclave memory
  ▼
[Cloud edge gateway]
  │  → Return ciphertext to client
  ▼
[Alice's MCP client]
  │  Decrypt result locally
  │  Return to Claude Code (or whatever MCP host)
```

**Latency budget:**

| Step | Target | Notes |
|---|---|---|
| Local delta computation (incremental) | <10 ms | Tree-sitter on changed files only |
| Network round-trip | 50-200 ms | Depends on user location |
| TEE attestation handshake | <50 ms | Cached after first call in session |
| Layer composition | <50 ms | In-memory diff overlay |
| Cypher query execution | 1-1000 ms | Depends on traversal depth |
| Result encryption + return | <20 ms | Small payloads |
| **Total p50** | **~150 ms** | Feels instant |
| **Total p99** | **~1.5 s** | Worst case 5-hop traversals |

## Session model

Each MCP client session is long-lived. The flow:

1. **Session start** (when Claude Code / Cursor opens):
   - Client opens WebSocket or long-lived HTTPS connection to the cloud
   - Sends `session.create` with org, repo, branch, attestation challenge
   - Cloud responds with session ID and TEE attestation document
   - Client verifies attestation against pinned enclave hash

2. **File-watch loop** (background):
   - Client uses fswatch / inotify to monitor working copy
   - On file change:
     - Re-parse the changed file with tree-sitter (~5 ms)
     - Compute delta against last committed state of that file
     - Update the in-memory working delta
     - Send `delta.update` over the session WebSocket (optional — can also send with each query)

3. **Query loop**:
   - User asks a question via Claude Code
   - MCP client sends `query.execute` with the latest working delta
   - Cloud composes layers, runs query, returns result
   - Client returns the result to Claude Code

4. **Session end** (when editor closes):
   - Client sends `session.close`
   - Cloud immediately drops the working delta from memory
   - Session-scoped enclave state is wiped

## Storage model on the cloud side

### Cloud base storage

Each repo's main branch graph is stored in a FalkorDB instance. For multi-tenant clouds, multiple orgs share a FalkorDB cluster but each gets its own graph (`org_acme__backend`). For dedicated/self-hosted, each customer has their own cluster.

### Branch overlay storage

Branch overlays are stored as serialized delta JSON in object storage (S3 or equivalent). Key format: `overlays/{org}/{repo}/{branch_sha}.json.zst`. Each overlay is content-addressed by its hash for cache efficiency.

When a query runs, the overlay is fetched from S3 (or memory cache), deserialized, and applied in memory inside the enclave. Hot branches are cached in enclave memory between queries.

### Working delta storage

**There is no persistent storage for working deltas.** They live only in:
- The user's local client memory
- The enclave's session memory (during query execution)

When the user's editor closes or the session times out, the working delta is forgotten.

## History layer (one graph, two layers)

The current-state graph (Layer 1) only knows what `main` looks like *right now*. The **history layer** is a second structural layer in the **same FalkorDB graph** that captures every commit as a first-class node, so the graph becomes time-travel capable.

### Why one graph instead of two

We considered three options:

| Option | Verdict |
|---|---|
| **A. Flat graph with `valid_from`/`valid_to` on every node** | ❌ Every query pays a temporal cost. Pytorch's 13M edges balloon to ~100M+. Local tier dies. |
| **B. Separate "current" and "history" graphs in different FalkorDB instances** | ❌ Cross-graph queries are nightmare. Schema drift. Backup gets 2× harder. Joins lose transaction safety. |
| **C. One graph, two layers within it** ✅ | Layer 1 stays fast. Layer 2 is queried only when needed. Single connection, single schema, single backup. |

We chose **C**.

### Schema additions

Two new types extend the existing schema:

```
Node label: Episode
  Properties:
    sha: string                  // commit SHA, message ID, agent action ID, etc.
    source_type: string          // "git_commit", "chat", "file_upload", "agent_action"
    timestamp: ISO8601 datetime
    author: string
    message: string              // commit message or equivalent
    branch: string               // for git commits, the branch tag

Edge type: CHANGES
  From: Episode
  To:   File / Function / Class
  Properties:
    op: "add" | "remove" | "modify" | "rename"
    before_props: dict (optional)  // for modify/rename, the properties prior to this commit
    after_props:  dict (optional)  // for modify/rename, the properties after
```

That is the entire history layer schema. Two new types, no changes to existing nodes or edges.

### How it composes with Layer 1

```
┌─────────────────────────────────────────────────────────────┐
│  LAYER 1: Current State (the "live" code property graph)    │
│                                                              │
│   ┌────────┐    CONTAINS    ┌──────────┐    CALLS          │
│   │  File  │ ─────────────► │ Function │ ──────────►        │
│   │ jwt.py │                │authenticate│                  │
│   └────────┘                └──────────┘                    │
│        ▲                          ▲                          │
└────────┼──────────────────────────┼──────────────────────────┘
         │                          │
         │ CHANGES                  │ CHANGES
         │ {op: "add"}              │ {op: "modify"}
         │                          │
┌────────┼──────────────────────────┼──────────────────────────┐
│  LAYER 2: Episode Log (the "history" overlay)               │
│                                                              │
│   ┌──────────────┐         ┌──────────────┐                 │
│   │ Episode      │         │ Episode      │                 │
│   │ sha: a1b2c3  │         │ sha: d4e5f6  │                 │
│   │ author: alice│         │ author: bob  │                 │
│   │ ts: 2026-01  │         │ ts: 2026-03  │                 │
│   │ msg: "init"  │         │ msg: "fix"   │                 │
│   └──────────────┘         └──────────────┘                 │
└─────────────────────────────────────────────────────────────┘
```

Episodes reference Layer 1 nodes by their canonical IDs (the same IDs defined in `delta-protocol.md`). No duplication. Layer 1 stays unchanged when no historical questions are asked.

### Time-travel queries

Five canonical query patterns the history layer enables:

```cypher
-- 1. Who last modified this function?
MATCH (e:Episode)-[c:CHANGES]->(fn:Function {name: $name})
RETURN e.author, e.timestamp, e.message
ORDER BY e.timestamp DESC LIMIT 1

-- 2. When was this function introduced?
MATCH (e:Episode)-[c:CHANGES {op: 'add'}]->(fn:Function {name: $name})
RETURN e.timestamp, e.author
ORDER BY e.timestamp ASC LIMIT 1

-- 3. What functions historically change together with X?
MATCH (e:Episode)-[:CHANGES]->(fn1:Function {name: $name})
MATCH (e)-[:CHANGES]->(fn2:Function)
WHERE fn1 <> fn2
RETURN fn2.name, count(e) AS co_changes
ORDER BY co_changes DESC LIMIT 10

-- 4. Find the expert for this file (recency-weighted contributions)
MATCH (e:Episode)-[:CHANGES]->(:File {path: $path})
WHERE e.timestamp > date() - duration('P90D')
RETURN e.author, count(e) AS recent_touches
ORDER BY recent_touches DESC LIMIT 5

-- 5. Reconstruct main as of a date (replay-based time-travel)
-- Returns all CHANGES committed *after* the target date.
-- The client reverses these to reconstruct the older state.
MATCH (e:Episode)-[c:CHANGES]->(n)
WHERE e.timestamp > $as_of_date
RETURN c.op, c.before_props, c.after_props, labels(n), n
```

### Branch interaction

The history layer extends naturally to branch overlays:

- Each Episode has a `branch` property
- Episodes from `main` are tagged `branch: "main"`
- Episodes from a feature branch are tagged with that branch name
- Time-travel against `branch=main` filters out feature-branch episodes
- Time-travel against `branch=alice/feature` includes both main's history *and* alice's branch episodes

When a feature branch merges to main, the merge job retags the relevant episodes from the branch name to `main` (or just adds duplicate references — implementation detail).

### Disk cost

Storage for the history layer scales linearly with commits, not with current code size:

| Repo | Commits | Estimated history size | Total graph size |
|---|---|---|---|
| flask | ~5,000 | ~10 MB | ~50 MB |
| fastapi | ~10,000 | ~20 MB | ~80 MB |
| django | ~30,000 | ~80 MB | ~280 MB |
| pytorch | ~70,000 | ~200 MB | ~2.2 GB |
| linux kernel | ~1.2M | ~3 GB | ~5 GB |

For local-tier users on small/medium repos, the history layer is essentially free. For pytorch-scale repos, it pushes the local tier closer to its limits — which becomes another natural cloud upgrade trigger. For linux-scale, you're decisively in cloud-only territory.

### Local tier defaults

The free local tier ships with history disabled by default and a `--history` opt-in:

```bash
savants init /path/to/repo               # current state only
savants init /path/to/repo --history     # current state + last 6 months
savants history /path/to/repo --since=1y # extend history backward later
```

This keeps the free experience fast for evaluators while letting power users opt in.

### Cloud tier defaults

The cloud tier indexes full history by default. The first sync of a repo runs a backfill job in distributed workers; subsequent commits become Episodes via webhook in real time. Users never wait for indexing after initial onboarding.

### Implementation status

| Component | Status |
|---|---|
| `Episode` node type | ✅ Already in `src/savants/graph/episodic.py` |
| `Episode` schema indices | ✅ Already defined |
| `EpisodicMemory.add_episode()` / `recall()` | ✅ Already implemented |
| `CHANGES` edge type | 🟡 To add to `src/savants/graph/schema.py` |
| `GitHistoryWalker` | 🟡 Scaffolded in `src/savants/history/walker.py` |
| `savants history` CLI command | 🟡 Stub in `src/savants/cli.py` |
| Time-travel query helpers | ❌ Not yet in `src/savants/graph/query.py` |
| MCP tools (`find_last_modifier`, `co_change_analysis`, etc.) | ❌ Not yet in `src/savants/mcp/server.py` |

The walker and schema additions are the immediate next steps. Time-travel query helpers and MCP tools are follow-on work once the data is loadable.

## Webhook integration

The cloud has GitHub/GitLab/Bitbucket apps installed by the customer at setup. The flow:

1. **Initial repo registration**:
   - Customer installs the SynapCode GitHub app on their org
   - Customer connects a repo via the SynapCode dashboard
   - Cloud clones the repo (or uses the Git provider's API to read tree state)
   - Initial full index runs in distributed workers
   - Webhook subscriptions registered for `push`, `pull_request`, `delete`

2. **On push to main**:
   - Webhook fires with commit list
   - Cloud computes file diff between previous indexed SHA and new HEAD
   - Incremental indexer re-parses only changed files
   - Updates cloud base graph in place
   - Notifies all active sessions: `base.updated`

3. **On push to a feature branch**:
   - Webhook fires
   - Cloud computes diff between branch HEAD and main
   - Re-builds branch overlay for that branch
   - Notifies sessions watching that branch: `overlay.updated`

4. **On branch deletion or PR merge**:
   - Branch overlay is deleted from object storage
   - Active sessions are notified to drop their cached overlay

## Privacy and the TEE

For the Confidential Cloud and Enterprise tiers, the cloud base + branch overlay + working delta composition happens **inside an AWS Nitro Enclave** (or equivalent: AMD SEV-SNP, Intel TDX, GCP Confidential Compute, Azure Confidential Computing).

**What this gives us:**
- The cloud operator (us) cannot read the graph data, even with root access
- Remote attestation lets clients verify the enclave is running the published code hash
- Memory encryption ensures plaintext only exists inside the enclave
- Even AWS / GCP / Azure cannot peek into customer data

**What this means for compliance:**
- GDPR: we are not a data processor in the regulatory sense — we cannot access plaintext PII
- HIPAA: same reasoning — cannot access PHI
- SOC 2: scope is dramatically reduced
- FedRAMP: confidential compute is a moderate baseline requirement
- Customer can verify all of this cryptographically — not just trust us

**What it does not give us:**
- Protection against client-side attacks (the user's laptop is still in scope)
- Protection against enclave bugs (we must keep up with patches)
- Protection against the user themselves leaking data

**See `docs/confidential-compute.md` for the full TEE architecture (TBD).**

## Offline mode

If the client cannot reach the cloud (network outage, on a plane, working in an air-gapped environment), the client falls back to local-only mode with reduced fidelity:

1. **Local cache:** the client maintains a small local FalkorDB sidecar (optional, opt-in) with cached snapshots of the cloud base and branch overlay for the user's active branches
2. **Local query execution:** queries run against the cached state + working delta
3. **Limitations:**
   - Cache may be stale (no team sync until reconnect)
   - Limited to repos that fit local hardware (~2,500 files today)
   - No access to other branches not in the cache
4. **Reconnection:** when network restores, client syncs the latest cloud base / overlays

This is opt-in for paid tiers. Free tier is local-only by default.

## Conflict resolution

What happens if Alice and Bob both edit the same function in their working copies?

**Nothing.** Each user's working delta is private to their session. They never see each other's uncommitted code. This is correct — git itself works the same way.

When Alice and Bob both push their branches:
- Each branch gets its own overlay
- Cloud serves both overlays independently
- Queries against `alice/feature` see Alice's changes
- Queries against `bob/feature` see Bob's changes
- A query against `main` sees neither until they merge

If they have a merge conflict in git, that's resolved at the git level. The graph just reflects whatever the merged state ends up being.

## Comparison to alternatives

| Approach | Sees uncommitted? | Scales to large repos? | Privacy | Latency | Notes |
|---|---|---|---|---|---|
| Local-only graph | ✅ | ❌ (~2.5k file ceiling) | ✅ | Fast | What our free tier is |
| Cloud-only, server-pulled | ❌ | ✅ | 🟡 | Fast | What Sourcegraph is |
| Cloud + push-on-save | 🟡 (slow) | ✅ | ❌ | Slow | Bad UX |
| Cloud + send-all-files-each-query | ✅ | ✅ | ❌ | Very slow | Privacy nightmare |
| **Layered (base + overlay + delta)** | ✅ | ✅ | ✅ (with TEE) | Fast | **Our architecture** |

## Implementation status

| Component | Status |
|---|---|
| Local FalkorDB sidecar (free tier) | ✅ Implemented (`src/savants/graph/`) |
| Tauri desktop app | ✅ Implemented (`desktop/`) |
| MCP server | ✅ Implemented (`src/savants/mcp/`) |
| Tree-sitter CPG builder | ✅ Implemented (`src/savants/graph/cpg.py`) |
| Local delta computer | 🟡 PoC in `src/savants/delta/computer.py` |
| Delta protocol JSON schema | ✅ Specified (`docs/delta-protocol.md`) |
| Cloud base (multi-tenant FalkorDB on K8s) | ❌ Not started |
| Branch overlay storage | ❌ Not started |
| Layer composition engine | ❌ Not started |
| Webhook receivers | ❌ Not started |
| TEE / Nitro Enclave integration | ❌ Not started |
| Self-hosted Helm chart | ❌ Not started |
| Rust port of indexer | 🟡 Scaffold in `rust-core/` |

## Roadmap

### Phase 1 (months 1-2): Cloud base + manual reindex
Ship the simplest version that gives the team tier real value.

- Multi-tenant FalkorDB cluster on AWS EKS
- Webhook receiver for GitHub push events
- Re-index on every push (no overlays yet)
- Basic auth + per-org isolation
- Same Cypher queries against the cloud as the local CLI uses

**At end of phase:** Team tier launches at $199/user/year. Sees committed state only.

### Phase 2 (months 3-4): Branch overlays
Add per-branch graphs for long-lived feature branches.

- Branch overlay storage in S3
- Webhook handles push-to-branch (not just main)
- Per-branch graph composition at query time
- Cache hot branches in memory

**At end of phase:** Long-running feature branches work correctly. Still doesn't see uncommitted.

### Phase 3 (months 5-6): Local working delta
The killer feature. Real-time impact analysis on uncommitted code.

- Local delta computer in client (Python or Rust)
- Delta protocol fully implemented
- Session WebSocket for live updates
- Layer composition engine in cloud

**At end of phase:** Sees uncommitted changes. Marketing campaign launches around this.

### Phase 4 (months 7-9): Confidential Cloud
TEE-backed cloud with attestation.

- AWS Nitro Enclave deployment
- Attestation verification SDK for clients
- Published enclave code hashes
- Migration tooling for existing customers

**At end of phase:** Confidential Cloud tier launches at $399/user/year.

### Phase 5 (months 10-12): Self-hosted productized
Helm chart, Terraform modules, customer-runs-it-themselves.

- Helm chart for Kubernetes
- Terraform modules for AWS/GCP/Azure
- Air-gapped installer bundle
- Upgrade tooling
- Documentation for ops teams

**At end of phase:** First productized self-hosted deals close.

### Phase 6 (year 2): Vertical compliance, dedicated cloud, third-party graph API
Expansion phase. Most of revenue growth happens here.

## Open questions

1. **Cypher dialect compatibility**: FalkorDB supports a subset of openCypher. Some traversal patterns we use (e.g., variable-length paths with WHERE filters) need careful testing under composition.

2. **Layer composition semantics for edges**: when an overlay removes a node, do we automatically cascade-remove edges touching it? (Yes, but the implementation needs to be efficient.)

3. **Working delta size limits**: what's the maximum delta we accept? (Tentatively 10 MB compressed — covers refactors of ~5,000 files.)

4. **Session reconnection on cloud restart**: if the cloud restarts mid-session, do we re-attest? (Yes, transparently.)

5. **Handling branches with binary file changes**: do we even try to index binary files? (No, skip.)

6. **Symlinks and submodules**: how do we represent these in the graph? (Special node types: SymLink, Submodule.)

7. **Performance of multi-hop queries on composed graphs**: empirically measured 32s for 5-hop on pytorch-scale (13M edges). Need to optimize.

## See also

- `docs/delta-protocol.md` — wire format for graph deltas
- `docs/profiling-results.md` — empirical limits of the local tier (TBD)
- `docs/confidential-compute.md` — TEE architecture details (TBD)
- `BUSINESS.md` — how the architecture maps to deployment tiers and revenue
- `src/savants/delta/computer.py` — Python PoC of the delta computer
- `rust-core/` — Rust port of the indexer hot path
