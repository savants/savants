# Runtime Layer: Jobs to be Done

**Status:** Settled. Decided 2026-04-08. Do not re-litigate.

This document defines what the Mazkir runtime layer is *for*. Every
architectural decision about Layer 4 (runtime state) should be in
service of one of the eight jobs below. If a piece of data doesn't
serve any of these jobs, **it doesn't belong in the graph.**

This is the design contract. When in doubt about whether to add a
feature, ask: "which of these eight jobs does it serve?" If the
answer is "none," delete the idea.

---

## The 8 jobs

These are the questions an AI agent (Claude Code, Cursor, a Slack bot,
an incident commander) will actually ask when given access to a
Mazkir-indexed cluster. Listed in approximate order of how often a
real engineer asks them in a normal workday.

### Job 1: "Is this safe to change?"

**Sample query:** *"I want to delete `payment_handler`. What breaks?"*

**What it needs:**
- Structural callers (Layer 1 CALLS edges)
- String-literal references (Layer 1 REFERENCES_SYMBOL edges)
- **Was this called by real production traffic in the last 7 days?** (Layer 4 liveness fingerprint)
- Currently deployed in which environments (Layer 4 deploy state)
- Was this function the subject of recent incidents (Layer 4 incident Episodes)

**Why it matters:** This is the PR-review killer query. Today, every senior
engineer asks this question 2-5 times per day and answers it by reading
six files for 20 minutes. Mazkir answers it in milliseconds with verifiable
receipts.

---

### Job 2: "What's running where?"

**Sample query:** *"Which version of `api` is in `prod-eu` right now?"*

**What it needs:**
- Latest deployment Episode per (service × environment)
- Current image SHA on each Deployment node
- The commit Episode the image was built from

**Why it matters:** Engineers and oncall constantly need to know "what's
actually deployed." Today this requires `kubectl` access to N clusters
and mental gymnastics. Mazkir collapses it to one query.

---

### Job 3: "What broke and why?"

**Sample query:** *"Prod is on fire. What changed in the last 2 hours that
touches the auth path?"*

**What it needs:**
- Deploy Episodes within the time window
- Commit lineage for each deploy (link to Layer 2)
- Open incident Episodes
- Structural reachability from changed functions to the affected service

**Why it matters:** Incident triage is the highest-leverage moment to have
a structural memory. Engineers under pressure don't have time to grep
through six repos. Mazkir gives them the answer in one query.

---

### Job 4: "Who do I talk to?"

**Sample query:** *"Who owns `payment-service`? Who's oncall right now?"*

**What it needs:**
- `Service -[:OWNED_BY]-> Team` edges
- `Service -[:CURRENT_ONCALL]-> Engineer` edges (refreshed hourly from PagerDuty)
- Slack handles, email, escalation policy

**Why it matters:** "Who do I page about this service?" wastes hours of
engineering time per week per company. Mazkir answers instantly.

---

### Job 5: "Where is this configured?"

**Sample query:** *"Prod has `feature_new_checkout=true`. Where is that set
in source?"*

**What it needs:**
- ConfigMap → DEPLOYED_FROM → ConfigKey edge back to the YAML in source repo
- Last applied timestamp + who applied it
- Active feature flag state per service

**Why it matters:** Config drift between source-of-truth and runtime is
one of the most common debugging time-sinks. Mazkir bridges code to
runtime config in one edge traversal.

---

### Job 6: "Is anyone using this code path?"

**Sample query:** *"Can I delete this function? It looks unused."*

**What it needs:**
- Liveness fingerprint boolean: `Function.was_called_in_prod_last_7d`
- Structural caller absence (no Layer 1 callers)
- Test coverage (callers from `tests/` directories)

**Why it matters:** Static "find unused code" tools get fooled by dynamic
dispatch, registries, reflection, and string-keyed dispatch. Liveness
fingerprints from real trace data give empirical proof of "no, nothing
calls this in production." This is the dead-code detection that actually
works.

---

### Job 7: "What changed between then and now?"

**Sample query:** *"Diff-impact between yesterday's release and the
incident at 14:00."*

**What it needs:**
- Episode timeline filtered by time window
- Structural reach of each changed function
- Aggregate "X functions touched, Y entry points affected, Z config keys
  changed"

**Why it matters:** Post-mortem analysis and "what shipped that broke
this" investigations need a temporal slice of structural changes.
Mazkir's Layer 2 + Layer 4 join makes this a single query.

---

### Job 8: "What's the blast radius of changing this thing in prod?"

**Sample query:** *"If I change this function, which currently-deployed
services break?"*

**What it needs:**
- Structural callers (Layer 1)
- Joined with currently-deployed services that contain those callers (Layer 4)
- Optionally weighted by liveness (live services first, dormant last)

**Why it matters:** This is the combined Job 1 + Job 2 question and
the one that produces the most "oh shit, I almost broke prod" moments
in user demos. It's the pitch slide.

---

## The killer query that proves the architecture

A single MCP tool call should produce a response shaped like this:

```
function_xray("payment_handler") returns:

  Definition:
    src/payments/handlers.py:142
    @app.route("/payments/charge")

  Structural callers (Layer 1):
    - process_order (src/orders/processing.py)
    - retry_failed_payment (src/cron/retry.py)
    - test_payment_flow (tests/test_payments.py)

  Production liveness (Layer 4):
    ✅ called by real prod traffic in the last 7 days
    last seen: 47 minutes ago

  Currently deployed in:
    ✅ prod-eu  (image registry/api:v2.4.1, deployed 6h ago by alice@)
    ✅ prod-us  (image registry/api:v2.4.1, same)
    ✅ staging  (image registry/api:v2.4.2-rc, deployed 12m ago by bob@)
    ❌ dev      (image registry/api:v2.3.0, 3 days stale)

  Current oncall:
    @bob (PagerDuty rotation: payments-primary)

  Recent incidents touching this code path:
    - PD-12345: 2 days ago, affected /payments/* for 3 min, root-caused
      to commit abc123 (which YOU are about to modify)

  Config dependencies:
    - payments.timeout_ms = 30000  (config/prod.yaml)
    - payments.max_retries = 3     (config/prod.yaml)

  Last touched in source:
    Episode (commit def456) by alice@, 6 hours ago
    Same commit was deployed to prod-eu and prod-us 6h ago
    Episode commit message: "fix race condition in retry logic"

  Verdict (composed from above):
    ⚠ HIGH RISK to modify:
      - actively serving prod traffic (last call 47min ago)
      - was the subject of an incident 2 days ago
      - same deploy is in prod-eu, prod-us, and staging
      - oncall is @bob, ping them before merging
```

That response is a *fact pack*. Every line is a structural fact retrieved
by traversing one or two edges. There are NO metrics, NO time series,
NO raw logs. It's a wiki entry about a function — except every fact is
fresh, link-traversable, and the wiki updates itself.

**That is the product.** Everything else in the design exists to
generate responses like this in <100ms. If a feature doesn't contribute
to making this kind of fact pack better, it doesn't belong in Mazkir.

---

## How to use this document

When designing any new feature for the runtime layer:

1. **Identify which job it serves.** If you can't name a job from the
   list above, the feature doesn't belong.
2. **Identify the minimum data needed.** Don't store more than what
   the job requires. Distill before storing.
3. **Identify the update cadence.** If the data changes more than once
   per minute, it's probably a metric, not a graph fact.
4. **Identify the storage tier.** New ephemeral data should default to
   the hot tier (last 90 days) and be eligible for compaction or cold
   archival per the retention policy.

If a proposed feature passes all four checks, build it. If it fails
any of them, push back or redesign.
