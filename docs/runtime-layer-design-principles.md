# Runtime Layer: Design Principles and Antipatterns

**Status:** Settled. Decided 2026-04-08. Do not re-litigate.

This document defines the rules and antipatterns for adding any data
to Mazkir's runtime layer (Layer 4). The 8 jobs in
`runtime-layer-jobs-to-be-done.md` define what we're trying to
accomplish; this document defines what we're allowed to store and
what we must never store.

---

## The fundamental rule

> **A knowledge graph is a wiki that updates itself. It is NOT a dashboard.**

Wikis store: structure, ownership, decisions, configurations,
relationships, reasons.

Dashboards store: metrics, time series, current numbers, trends.

If we violate this distinction, we don't end up with "a really
powerful graph." We end up with a slow, expensive, poorly-indexed
dashboard that does neither job well. Every successful knowledge
graph in production (Glean, LinkedIn Galaxy, Uber's service mesh,
Netflix's Atlas Insight) follows this rule rigorously. The ones
that violated it collapsed under their own write volume.

**The litmus test:** If you'd put it in an internal wiki, put it
in the graph. If you'd put it in a Grafana dashboard, don't.

---

## The 10 antipatterns

These are the failure modes we must never allow into the runtime layer.
Each is paired with the right alternative.

### Antipattern 1: Store metrics in the graph

**Don't:** "Number of requests per second" as a Function property.

**Why it fails:**
- Properties update constantly → index thrash → query latency degrades
- The graph competes with Prometheus and loses (Prometheus is
  ~1000× faster at this)
- Write volume destroys the gross margin advantage

**Right move:** Store a *reference* (`function.trace_id =
"mazkir.cli.init"`) and query the metrics system separately. The graph
holds the identity, the TSDB holds the measurements.

---

### Antipattern 2: Store every event

**Don't:** Create a node for every pod restart, every health check,
every container start.

**Why it fails:** Millions of nodes per day per cluster. Within a month
the graph is unusable.

**Right move:** Distill to **interesting transitions** only. A pod going
`Healthy → CrashLoopBackOff` is an Episode worth recording. A pod going
`Pending → Running` is normal startup, no signal, don't store it.

---

### Antipattern 3: Try to be real-time

**Don't:** Build the graph as a streaming system that answers "what's
the current CPU usage" with sub-second freshness.

**Why it fails:** A knowledge graph is **eventually consistent** with
reality. Trying to be real-time means you've built a slow, expensive
Prometheus.

**Right move:** ~1-second freshness for K8s state via the operator (good
enough for "what's deployed"), reference Prometheus for sub-second
metrics. The graph is fresh enough to answer "what's running right now"
but not fresh enough to be a streaming system.

---

### Antipattern 4: Store ephemeral state without compression

**Don't:** Keep every state change of a pod as a separate node or
property history.

**Why it fails:** Either you lose history (overwrites) or you explode
in volume (every change as a node).

**Right move:**
- Store the *current* state inline as a property (`pod.last_status =
  "Running"`)
- Emit an Episode only for transitions a human cares about
  (`Running → CrashLoopBackOff`, `Failed`, `Evicted`)
- Never store the boring transitions

---

### Antipattern 5: Mix structural facts with operational telemetry

**Don't:** Put `function.calls_last_hour = 4723` on a Function node.

**Why it fails:** Structural queries get slow and telemetry queries get
expensive. The two access patterns fight each other for index attention.

**Right move:** Layer them. Layer 1-3 = structure. Layer 4 = current
state references. Telemetry stays in its native store, referenced by ID.

---

### Antipattern 6: Store nodes that have no edges

**Don't:** Add a node just because you have data about it.

**Why it fails:** The graph's value is in the edges. A node with no
edges is just a property bag and belongs in a key-value store, not a
graph.

**Right move (litmus test):** "What edges does this node have?" If the
answer is "none, it's a property bag," it's not a graph node.

---

### Antipattern 7: Let the graph drift from sources of truth

**Don't:** Cache K8s state in the graph and serve stale data when K8s
disagrees.

**Why it fails:** Stale beats wrong, but only barely. If the graph
says "Pod X is running" but K8s says it died 10 minutes ago, the
graph is lying and engineers will stop trusting it.

**Right move:**
- Watch-based refresh, not poll-and-cache
- TTL on every ephemeral node so it expires if the source stops reporting
- Source-of-truth identity (`pod.k8s_uid = "..."`) so re-queries are cheap
- When in doubt, mark as unknown rather than serving stale data

---

### Antipattern 8: Update high-volume properties on stable nodes

**Don't:** Update `function.last_called_at` every time the function runs
in production.

**Why it fails:** Puts write pressure on a node that should be stable.
Indexes thrash. The Function node — which is the most-queried node type
— becomes the slowest one to read.

**Right move:** A nightly batch job that updates
`function.was_called_last_7d = true|false` once per day. **One write per
function per day, not one write per call.** Distillation, not raw
ingestion.

---

### Antipattern 9: No GC story for ephemeral nodes

**Don't:** Ingest every Pod ever and never delete the dead ones.

**Why it fails:** The graph grows unbounded. Query latency degrades.
Storage costs creep. Eventually the graph is unusable on old customer
accounts.

**Right move:** See `runtime-layer-retention-and-gc.md`. Three-tier
retention with aggressive compaction. Pods deleted 24h after they
terminate. Images deleted 30 days after they stop being deployed.
Episodes compacted into monthly aggregates after 90 days.

---

### Antipattern 10: Build for dashboard refresh patterns

**Don't:** Optimize the graph for high-frequency reads of recent data.

**Why it fails:** Dashboard query patterns (high read rate, shallow
queries on recent windows) actively harm deep structural query
performance. The two workloads compete for the same indexes and the
deep queries lose.

**Right move:** Let Grafana/Datadog handle dashboards. The graph
answers one-shot questions about structure, history, and current state
— not high-frequency polling. If a customer asks for a "live dashboard
view," that's a sign the feature is in the wrong tier.

---

## What to store (the constructive list)

These are the data types that DO belong in the runtime layer because
they serve one or more of the 8 jobs.

| Entity | Purpose | Update frequency | Source |
|---|---|---|---|
| **Deployment** node | Jobs 2, 8 | Per deploy event (~ minutes to days) | CI/CD webhook or K8s operator |
| **Image** node | Jobs 2, 8 | Per build (~ hourly) | CI webhook, links via `BUILT_FROM → Episode (commit)` |
| **Liveness fingerprint** boolean on Function | Jobs 1, 6 | Once nightly | Batch job over OpenTelemetry traces |
| **Deploy Episode** | Jobs 3, 7 | Per deploy | Webhook |
| **Incident Episode** | Jobs 3, 7 | Per incident open/close | PagerDuty webhook |
| **Owner** edge: `Service → OWNED_BY → Team` | Job 4 | Manual / annual | Repo config file or service catalog |
| **Oncall** edge: `Service → CURRENT_ONCALL → Engineer` | Job 4 | Hourly refresh | PagerDuty API |
| **K8sConfigMap** + `DEPLOYED_FROM → ConfigKey` edge | Job 5 | On apply | K8s operator + commit-source matching |
| **FeatureFlag** node | Job 5 | On toggle | LaunchDarkly/Split/Unleash webhook |

---

## What NOT to store (decision table)

| Signal | Graph? | Where it goes |
|---|---|---|
| Pod CPU / memory at any granularity | NO | Prometheus / VictoriaMetrics |
| Per-request trace spans | NO | Tempo / Jaeger |
| Healthcheck results | NO | Prometheus |
| Per-restart pod events | NO (aggregate) | One Episode if threshold crossed |
| HTTP request count per second | NO | Prometheus |
| Pod IPs / ports / nodes | NO | K8s API directly |
| Memory of every container at every second | NO | Prometheus |
| Every config-map field-level diff | NO | Store current value, one Episode per apply |
| Real-time pod restart counts | NO | One Episode if threshold crossed |
| Pod entered CrashLoopBackOff at 14:23 | YES | Episode |
| Current pod count for deployment | YES | Property, refresh on watch |
| Deployment rolled back at 02:14 to commit abc | YES | Episode |
| Latest image SHA per deployment | YES | Property, refresh on event |
| Service unreachable 3m at 14:00 | YES | Episode (incident) |
| ConfigMap updated | YES | Episode + property |
| Function called by production traffic in last 7 days | YES | Boolean property, nightly refresh |
| New feature flag toggled | YES | Episode + property |
| Each individual flag check | NO | Metric |

---

## The architecture in three stores

The single most important architectural decision: **the runtime layer
is a *projection* of source-of-truth systems, not the system of record
itself.**

Three stores, each doing what it's good at:

```
                    ┌─────────────────────┐
                    │  Mazkir Graph       │
                    │  (FalkorDB / cloud) │
                    │                     │
                    │  Layer 1: Code      │
                    │  Layer 2: History   │
                    │  Layer 3: Inferred  │
                    │  Layer 4: Runtime   │
                    │  (last-known state, │
                    │   transition Episodes,
                    │   identifiers)      │
                    └────────┬────────────┘
                             │
                             │ references by ID
                             │
                ┌────────────┼─────────────┐
                ▼            ▼             ▼
         ┌──────────┐ ┌──────────┐ ┌──────────────┐
         │ TSDB     │ │ Trace    │ │ Log store    │
         │ (Prom,   │ │ store    │ │ (Loki,       │
         │  VM,     │ │ (Tempo,  │ │  Elastic,    │
         │  CH)     │ │  Jaeger) │ │  S3)         │
         └──────────┘ └──────────┘ └──────────────┘
         metrics      traces        logs
```

Mazkir holds **identifiers** that link to the other two:

- `Pod.metric_id = "kube_pod_info{pod=foo-abc,namespace=prod}"` →
  Prometheus
- `Function.trace_id = "mazkir.cli.init"` → Tempo
- `Episode.incident_id = "PD-12345"` → PagerDuty

When the user asks a deep question, the graph navigates the structural
relationships and the linked external systems provide the time-series
numbers. **The graph is the index; the metric stores are the body.**

This is the same pattern as Glean for documents (Glean stores structure,
S3/Drive holds the bytes), Sourcegraph for code (graph + git), Linear
for issues (graph + GitHub). The ones that scaled all use this pattern.
The ones that tried to put everything in the graph all collapsed.

---

## Decision flowchart for new data

When someone proposes adding new data to the runtime layer, run this:

```
Does it serve at least one of the 8 jobs?
  ├── No → don't add it
  └── Yes ↓

Does it have edges to existing nodes?
  ├── No → it's a property bag, store in KV not graph
  └── Yes ↓

Does it update more than once per minute on a stable node?
  ├── Yes → it's a metric, reference by ID instead
  └── No ↓

Can it be distilled into a single fact (boolean, last-known value)
or a transition Episode?
  ├── No → it's raw data, doesn't belong in the graph
  └── Yes ↓

Does it have a clear retention/GC policy?
  ├── No → design that first
  └── Yes ↓

Add it.
```

If any answer is "no," the data either doesn't belong in the graph
or needs to be redesigned before it does.

---

## The killer ephemeral signal

Of all the runtime data we could capture, **the single highest-value
signal** is:

> **Has this code path been hit by a real production request in the
> last N days?**

A single boolean per Function node, refreshed nightly from trace data.
It unlocks:

- Real dead code detection (vs static analysis that misses dynamic dispatch)
- Refactor safety with empirical evidence
- "Which functions matter for this incident?" → only the ones currently warm
- "Where should we focus refactoring?" → busy code paths first
- "What's our actual API surface?" → only the routes that have been hit

One bit per function. Tiny write load. Enormous query value. **This is
the irreplaceable Layer 4 feature** — every other capability is
incrementally useful, but the liveness fingerprint is what makes Mazkir
structurally different from grep, LSP, or any code-only intelligence
tool.

Build the liveness fingerprint before any of the other Layer 4 features.
Without it, Layer 4 is "another deploy tracker." With it, Layer 4 is
"the only graph that knows what your code is actually doing."
