# Runtime Layer: Retention, Compaction, and Garbage Collection

**Status:** Settled. Decided 2026-04-08. Do not re-litigate.

This document defines the retention strategy for the Mazkir runtime
layer. **Pruning is not optional and not an optimization — it's a
load-bearing part of the architecture.** Build it before you build
Layer 4, not after.

---

## Why this matters

Without pruning, even a medium-sized customer (200 engineers, 100
services, 20 deploys/day average) generates roughly:

| Source | Volume / day | Volume / year |
|---|---|---|
| Deploy Episodes | ~2,000 | ~730K |
| ConfigMap apply Episodes | ~500 | ~180K |
| Incident Episodes | ~5 | ~1,800 |
| Pod state-change Episodes (after distillation) | ~200 | ~73K |
| Runtime nodes (Pod, Image, ConfigMap, etc.) | ~5,000 churned | constant ~50K live |
| **Total Layer 4 events / year** | | **~1M Episodes** |

At ~500 bytes per Episode, that's ~500 MB/year of pure runtime events
on top of the ~600 MB baseline of Layer 1 + Layer 2. **Without pruning,
a 5-year-old customer's graph hits ~3 GB.**

The graph doesn't *break* without pruning. It *degrades*:

- Layer 1+2 traversals start hitting 5-year-old runtime junk → ~5ms
  median query becomes ~50ms
- Signal-to-noise collapses → "what changed in the last 30 days" has
  to filter through 5 years of Episodes
- Indexes get unhappy → write performance degrades too

Degradation is worse than failure because nobody notices until it's slow,
and then it's a hard fix. **GC is a feature, not an afterthought.**

---

## Three-tier retention model

### Tier 1: Hot (live graph, last ~90 days)

The live FalkorDB graph as it's queried by MCP tools. Strict size
budget.

- **All Layer 1 nodes** (Files, Functions, Classes, ConfigKeys,
  EnvVars) — **never deleted**, source of truth for current code
  state, churns with code only
- **Active Layer 4 nodes** (current Deployments, Pods, Images,
  ConfigMaps, Services) — **overwritten in place**, no growth
- **Last 90 days of Episodes** — full fidelity, every deploy event,
  every incident, every state transition

### Tier 2: Warm (compacted, 90 days – 1 year)

This is the clever tier most knowledge graph designs skip. Instead of
deleting old Episodes outright, the nightly GC pass **rolls them up
into aggregate Episodes**.

For example, the GC job sees this in October 2024:

```
deploy_episode(api, 2024-10-01, alice, abc123)
deploy_episode(api, 2024-10-01, alice, def456)
deploy_episode(api, 2024-10-02, bob, ghi789)
... 44 more deploy episodes ...
deploy_episode(api, 2024-10-31, alice, xyz999)
```

47 individual Episodes for `api-service` in October 2024. At GC time
(when these Episodes age past the 90-day hot threshold), they get
**compacted** into a single rollup Episode:

```
aggregate_deploy_episode(
  service: api,
  period: "2024-10",
  count: 47,
  unique_committers: ["alice", "bob", "carol"],
  preceded_incidents: 2,
  rolled_back_count: 1,
  first_commit: "abc123",
  last_commit: "xyz999"
)
```

**One node replaces 47.** You lose per-deploy granularity but you keep
the high-signal facts: "47 deploys, 2 caused incidents, 1 was rolled
back, here's the commit range." The query "show me the deployment
cadence of api-service over the last year" still works, just at
month-granularity instead of per-event.

This is the same pattern Datadog uses for traces (full fidelity for 15
days, aggregated for 13 months) and Prometheus uses for metrics (raw
samples for 14 days, downsampled for 1 year). It works because **most
queries don't need event-level granularity past the freshness window**
— they need trends and counts and "how often did this happen."

### Tier 3: Cold (archive, > 1 year)

Anything older than ~1 year goes to a cheap object store (S3, R2, etc.)
as Parquet files. **Not in the live graph at all.**

- Queryable on-demand if a customer asks "what happened in 2023" — the
  query loads the relevant Parquet file into a temporary view, answers,
  drops it
- Costs essentially nothing (~$0.023/GB/month on S3 Standard)
- Never affects hot-path query latency

The cold tier is the "eternal archive" that compliance auditors care
about. The hot+warm tiers are the "queryable working memory" that
engineers actually use.

---

## Per-node-type retention rules

| Node type | Retention policy |
|---|---|
| **File, Function, Class, ConfigKey, EnvVar** (Layer 1) | **Never delete** — overwritten on reindex, deleted only if the source file is deleted |
| **Episode (commit)** — Layer 2 | Hot for 1 year, then compacted into monthly "47 commits in 2024-10" rollups, cold after 3 years |
| **Episode (deploy)** — Layer 4 | Hot for 90 days, compacted to monthly aggregate, cold after 1 year |
| **Episode (incident)** — Layer 4 | **Hot forever if open**, compacted after resolution + 1 year, cold after 3 years (compliance retention requirement) |
| **Episode (config apply)** | Hot for 90 days, compacted after, cold after 1 year |
| **Pod node** | Hot while alive + 24h grace, then **deleted entirely** (the deploy Episode references the image, not the pod — pods are too ephemeral to keep individually) |
| **Image node** | Hot while deployed somewhere, **deleted** when no Deployment references it for 30 days |
| **ConfigMap version** | Keep last N=10 applied per namespace, older versions compacted into "12 changes in 2024-10" |
| **FeatureFlag toggle Episodes** | Hot for 90 days, then compacted |
| **Owner / oncall edges** | **Never expire** — overwritten when the data changes |

The pattern: **structural facts persist, ephemeral events compact, raw
observations expire.**

---

## What to never prune

There's a temptation to prune things that look "old" but actually carry
permanent value. Don't:

- **Incident Episodes** — keep these forever (or compact, never delete).
  The "we had an outage 14 months ago" memory is exactly what makes the
  graph valuable for the next on-call engineer.
- **Approval / audit Episodes** — compliance requires retention, often
  7 years. These go to cold storage, never deleted.
- **Owner / oncall edges** — these reflect *current* state, not history.
  Overwritten in place, never grow.
- **The link between an Image and the Episode (commit) it was built
  from** — even if the image is no longer deployed, the deploy-history
  Episode that mentioned it might still be relevant. Don't orphan-delete
  eagerly; let the compaction step decide.
- **Liveness fingerprints** — these are tiny (one bit per function),
  cheap to keep, useful for "wait, this function hasn't been touched in
  18 months, is it dead?" queries.

---

## The GC job in concrete terms

The nightly job (a Temporal workflow, since we already have Temporal
in the stack) does three things:

### 1. Compact Episodes past the hot threshold

Walk all Episode nodes older than 90 days. Group them by `(service,
type, month)`. For each group, create one aggregate Episode with the
rollup properties. Delete the originals.

```python
# Pseudocode
for (service, episode_type, month), episodes in group_by_month(old_episodes):
    aggregate = AggregateEpisode(
        service=service,
        type=episode_type,
        period=month,
        count=len(episodes),
        # Per-type rollup logic:
        unique_actors=set(e.actor for e in episodes),
        threshold_breaches=count_threshold_breaches(episodes),
        first_event=min(e.timestamp for e in episodes),
        last_event=max(e.timestamp for e in episodes),
    )
    insert(aggregate)
    delete_all(episodes)
```

### 2. Garbage-collect dead runtime nodes

Walk all runtime nodes without an active reference:

- **Pods**: dead and >24h old → delete
- **Images**: no deployment for 30 days → delete
- **ConfigMaps**: orphaned (no Deployment reads them) for 7 days → delete
- **Services**: no longer in any namespace → delete

### 3. Archive very-old Episodes to cold storage

Walk all very-old Episodes:

- Layer 4 Episodes >1 year old → serialize to Parquet, drop from graph
- Layer 2 commit Episodes >3 years old → serialize to Parquet, drop from graph

The Parquet files live in the customer's tenant bucket on S3 with
metadata indices, queryable on demand via a "load cold view" path
that's not used by 99% of queries.

### Performance budget

- Runs at 3am customer-local time
- Target: complete in <5 minutes for a 200-engineer org
- Holds a write lock on the graph for the compaction phase only
  (~30 seconds), reads are unaffected
- Idempotent: safe to re-run if it fails partway

---

## The size budget that makes the business work

After the tiered model with compaction, the steady-state graph size for
a 200-engineer org settles at roughly:

| Layer | Size | Notes |
|---|---|---|
| Layer 1 (code structure) | ~600 MB | Stable, scales with code not time |
| Layer 2 (last 1y commits, full fidelity) | ~30 MB | Then compacted |
| Layer 2 (compacted commits, year 2-3) | ~5 MB | Monthly rollups |
| Layer 4 (last 90d events, full fidelity) | ~150 MB | The hot layer that powers most queries |
| Layer 4 (compacted events, day 91 - year 1) | ~30 MB | Monthly rollups |
| **Total live graph** | **~815 MB** | |
| Cold archive (S3) | ~5 GB / customer / 5 years | $0.12/customer/month in storage |

**Steady state is well under 1 GB live, ~$0.20/customer/month total
storage cost.** That's the math that keeps the 99% gross margin holding
even at year 5 of customer tenure. This is the discipline.

Without GC and compaction, the same customer hits ~5-10 GB live in
year 5 and:

- Per-customer storage cost is still trivial in absolute terms
- But query latency degrades from ~5ms to ~50ms
- Indexes become unhappy
- You've trained customers to expect millisecond responses and now you
  can't deliver them

Pruning is what separates a graph product that scales from one that
quietly rots.

---

## Implementation order

When building Layer 4, the GC story has to land **at the same time** as
the ingest path, not after. The order:

1. Schema for Layer 4 nodes + retention metadata (`created_at`, `tier`,
   etc.)
2. Ingest path (webhook receiver)
3. **GC job (Temporal workflow)**
4. Compaction logic for each Episode type
5. Cold archive serializer (Parquet → S3)
6. Cold query loader (Parquet → temp view on demand)

Steps 1-3 are non-negotiable for the MVP. Steps 4-6 can be staged in
over the first 3 months but the hot/warm boundary needs to exist from
day one or you'll have to do a painful migration later.
