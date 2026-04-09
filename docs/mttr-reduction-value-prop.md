# MTTR Reduction: Mazkir's Lead Value Proposition

**Status:** Settled as Mazkir's primary lead use case, decided 2026-04-09.
This document captures the single most important customer value prop
and the supporting architecture, math, and positioning.

**TL;DR:** Mazkir reduces MTTR by ~75% by auto-correlating deploys,
config changes, IAM modifications, dependency state, and past incident
patterns the moment an incident fires. PagerDuty triggers the incident;
Mazkir writes the investigation. The on-call engineer shows up to a
pre-assembled war room instead of starting from a blank console.

---

## Why MTTR is the lead use case

Every other use case we've documented (AI agent grounding, code
intelligence, cost optimization, compliance autopilot, etc.) is
real and valuable. But **MTTR reduction is the one with**:

1. **The clearest quantifiable ROI.** Every minute of MTTR = real
   dollars in lost revenue. Customers already track this metric.
2. **The most acute customer pain.** Every on-call engineer at every
   company has been woken up at 2 AM trying to piece together what
   changed. This is universal.
3. **The strongest "nobody else can do this" story.** Existing tools
   each handle one layer (Datadog = metrics, PagerDuty = alerts,
   GitHub = code). Mazkir joins across layers, which is exactly what
   investigation requires.
4. **Revenue-justifiable at the Business tier** from day one. The
   math holds up even for mid-market customers.
5. **The shortest demo-to-"oh shit"-moment** in the product. Show an
   incident triage flow side-by-side and the value is obvious in
   60 seconds.

**Every other use case should be positioned as "and by the way, it
also does X" — with MTTR reduction as the headline.**

---

## The anatomy of a typical incident

Most SREs can tell you their MTTR is ~30-60 minutes for non-trivial
incidents. What they usually can't articulate is that **60-80% of
that time is investigation, not the actual fix**.

Realistic breakdown of a 45-minute incident (mid-market SaaS):

| Phase | Time | What happens |
|---|---|---|
| **Detection** | ~2 min | Alert fires, email/SMS sent |
| **Acknowledgment** | ~2 min | Engineer wakes up, opens laptop, ACKs |
| **Investigation** | **~30 min** | **Where all the time goes** |
| **Mitigation** | ~5 min | Rollback, config change, scale up |
| **Verification** | ~5 min | Confirm error rate recovered |
| **Total MTTR** | **~44 min** | |

Investigation breaks down into 7 manual sub-steps, each requiring
context-switching across multiple tools:

| Investigation sub-step | Time | Why slow |
|---|---|---|
| What service is affected? | 1-3 min | Parse alert, map to deployment |
| What changed recently? | 5-10 min | Manually hop CloudTrail, Argo CD, GitHub, Slack |
| Known issue? Seen before? | 3-5 min | Slack search, runbook hunt |
| Blast radius? Other services affected? | 3-5 min | Check every downstream dashboard |
| Who owns it? Who to page? | 1-3 min | CODEOWNERS, PagerDuty schedule |
| Root cause? | 10-15 min | Hypothesis testing |
| What's the fix? | 3-5 min | Decide mitigation strategy |

**Every one of these is the on-call engineer manually running a graph
query in their head, against tools that weren't designed to be graph
queries.** Mazkir does the graph query in 200ms.

---

## How Mazkir reduces each investigation sub-step

The MTTR reduction isn't hand-waving. Each specific sub-step gets
faster because Mazkir already has the joined data:

### 1. "What service is affected?" — 1 min → 0 sec

Before: engineer parses alert, maps name to deployment.
With Mazkir: the Incident Correlator agent already posted to the
incident Slack channel within ~30 seconds of the page. The engineer
arrives to a pre-assembled war room.

### 2. "What changed recently?" — 5-10 min → 5 sec

Before: open CloudTrail, Argo CD, GitHub, Slack one at a time.
With Mazkir: single query returns the unified timeline across every
source — code deploys, ConfigMaps, IAM, Helm releases, secret rotations,
all joined by timestamp:

```
recent_changes(service="payments-api", window="4h") returns:
  14:22 — Code deploy: payments-api v2.4.1 (by @tom, PR #4872)
  13:58 — RDS param group change: max_connections 200 → 500 (by @jenna)
  13:45 — ConfigMap update: rate_limit 1000 → 500 (by @alice)
  NO code deploys in last 2 hours
```

### 3. "Known issue? Seen before?" — 3-5 min → 2 sec

Before: Slack keyword search, runbook hunt.
With Mazkir: Mazkir has Episode history of every past incident.
Pattern matching against past incidents returns root causes and
fixes automatically.

```
past_incidents(service="payments-api", pattern="retry_exhausted") returns:
  PD-17891 (35d ago): RDS connection pool exhaustion
  PD-16234 (78d ago): downstream gateway SLA breach
  PD-15678 (102d ago): retry_max_attempts too low
```

### 4. "Blast radius?" — 3-5 min → 1 sec

Before: manually check every downstream service's dashboards.
With Mazkir: graph traversal returns the full dependency map + current
health:

```
blast_radius(service="payments-api") returns:
  Direct callers: api-gateway (degraded), payments-webhook (degraded)
  Downstream: RDS payments-prod (487/500 connections ⚠ NEAR CAPACITY)
  Root cause likely: RDS connection exhaustion
```

### 5. "Who owns it?" — 1-3 min → 1 sec

Before: CODEOWNERS, PagerDuty lookup, tribal knowledge.
With Mazkir: ownership + oncall edges are in the graph:

```
ownership(service="payments-api") returns:
  Team: payments-platform (#payments-platform Slack)
  Current oncall: @bob (active, PagerDuty rotation)
  Recent change author: @jenna (RDS change)
  Code owner: @tom (80% of recent commits)
```

### 6. "Root cause?" — 10-15 min → already known

By the time the engineer has steps 2-5 in front of them, the root
cause hypothesis is obvious from the correlation. Mazkir doesn't
replace human judgment — it eliminates the manual data-gathering
that precedes judgment.

### 7. "What's the fix?" — 3-5 min → 30 sec

Mazkir suggests mitigation options ranked by speed vs. risk, based
on past incident patterns:

```
suggested_mitigation(incident="PD-19847") returns:
  FASTEST: enable circuit breaker (30s recovery, LOW risk)
  ROLLBACK: terraform rollback RDS param change (5 min, LOW risk)
  SCALE: increase pool size (15 min, MEDIUM risk, requires restart)
```

---

## The MTTR math, side by side

| Phase | Without Mazkir | With Mazkir | Delta |
|---|---|---|---|
| Detection | 2 min | 2 min | 0 |
| Acknowledgment | 2 min | 2 min | 0 |
| **Investigation** | **30 min** | **3 min** | **-27 min** |
| └ Service affected | 1-3 min | 0 sec | -2 min |
| └ What changed | 5-10 min | 5 sec | -8 min |
| └ Known issue | 3-5 min | 2 sec | -4 min |
| └ Blast radius | 3-5 min | 1 sec | -4 min |
| └ Ownership | 1-3 min | 1 sec | -2 min |
| └ Root cause | 10-15 min | 30 sec | -12 min |
| └ Fix decision | 3-5 min | 30 sec | -4 min |
| Mitigation | 5 min | 2 min | -3 min |
| Verification | 5 min | 1 min | -4 min |
| **Total MTTR** | **44 min** | **10 min** | **-34 min (77%)** |

**77% MTTR reduction.** Not marketing — a structural reduction from
replacing manual cross-tool data-gathering with single graph queries.

---

## Worked incident scenario: the 8-minute version

### T+0:00 — 14:02 — Incident begins
`payments-api` error rate jumps from 0.02% to 18% during morning peak.

### T+0:20 — 14:02:20 — Mazkir detects via Datadog webhook
Agent subscribes to error rate threshold events. Posts preliminary
notice to `#incidents` Slack before PagerDuty even fires.

### T+0:45 — 14:02:45 — Full analysis posted
Mazkir's Incident Correlator posts the complete investigation:
what changed, blast radius, root cause hypothesis (85% confidence
on RDS connection exhaustion), ownership, oncall, suggested
mitigations. All 7 investigation sub-steps in one message.

### T+1:30 — 14:03:30 — PagerDuty pages @bob
PagerDuty has natural ~90s alert aggregation delay. Bob wakes up.

### T+2:00 — 14:04 — Bob reads the Mazkir analysis
30 seconds to read. Full picture. No tools to open.

### T+2:30 — 14:04:30 — Bob decides mitigation
Enables circuit breaker (bandaid) while coordinating RDS rollback
with @jenna (identified via recent-change author edge).

### T+3:00 — 14:05 — Circuit breaker active
Error rate drops from 18% to 1%. Customer impact mostly mitigated.

### T+3:30 — 14:05:30 — RDS rollback initiated
Jenna confirms her RDS param change was the trigger. Runs rollback.

### T+5:00 — 14:07 — RDS stable, connections recovered

### T+6:00 — 14:08 — Circuit breaker disabled
Error rate back to 0.02% baseline.

### T+8:00 — 14:10 — Verification complete
Mazkir posts recovery confirmation + auto-generated post-mortem draft.

**Total MTTR: 8 minutes.**

**Counterfactual without Mazkir: 32-42 minutes** (engineer opens
CloudWatch, CloudTrail, Argo CD manually, takes 15-25 min to realize
the RDS change is the cause, another 10+ min to coordinate rollback).

---

## Why existing tools don't do this

The honest differentiation — existing observability and incident
tools each cover one layer of the stack. Incident investigation
requires joining across layers, which is exactly what Mazkir's graph
does and nobody else does.

| Tool | What it does | What it misses for MTTR |
|---|---|---|
| **Datadog APM** | Traces, metrics, logs per service | Doesn't know about recent deploys, IAM, config — the causes |
| **PagerDuty AIOps** | Alert correlation, deduplication | Doesn't know about code, infra, or dependencies |
| **New Relic** | Same category as Datadog | Same gap |
| **Splunk Observability** | Logs + traces | No graph model, no structural join |
| **GitHub Copilot Workspace** | Code understanding | No runtime awareness |
| **Wiz / Orca** | CSPM (security posture) | Not designed for triage |
| **AWS Resource Explorer** | Inventory | No history, no dependencies, no incidents |
| **Mazkir** | **Graph joining code + runtime + history + ownership + incidents** | Nothing — uniquely positioned |

**The SRE today is the one doing the cross-layer join manually, in
their head, at 2 AM, under pressure.** Mazkir automates that join.

**Mazkir doesn't replace Datadog.** It sits *above* Datadog + PagerDuty
+ GitHub + AWS + K8s and answers the "what the hell is going on"
question that no single tool can answer alone.

---

## The business case: dollar math

For a realistic mid-market SaaS customer (~300 engineers, ~$40M ARR):

**Before Mazkir:**
- 50 incidents/year (typical rate)
- Average MTTR: 45 minutes
- Total downtime: 37.5 hours/year
- Revenue loss @ ~1% hourly revenue per downtime hour: **$171,400**
- Engineer time (3 engineers × 45 min × 50 × $150/hr): **$16,875**
- Customer churn impact (2% reliability-driven churn): **$80,000**
- **Total annual cost of incidents: ~$268,000**

**With Mazkir:**
- 50 incidents/year (Mazkir doesn't prevent — it shortens)
- Average MTTR: 10 minutes
- Total downtime: 8.3 hours/year
- Revenue loss: **$37,900**
- Engineer time (1 engineer × 10 min × 50 × $150/hr): **$1,250**
- Customer churn impact: negligible
- **Total annual cost of incidents: ~$42,000**

**Annual MTTR savings: ~$226,000**

**Mazkir Business tier cost: $24,000/year**

**ROI from MTTR reduction alone: 9.4×**

That's before counting:
- Cloud waste cleanup (~$50K/year typical)
- Compliance / audit prep time
- Reduced on-call burnout (turnover savings)
- Faster onboarding
- Everything else Mazkir does

**The MTTR use case alone justifies the entire product at the
Business tier. Everything else is upside.**

---

## Positioning: MTTR is the lead pitch

### Old pitch (from earlier iterations)
> *"Mazkir is the MCP server that gives Claude real AWS visibility."*

### New pitch
> **"Mazkir cuts your MTTR by 75% by auto-correlating deploys, config
> changes, and dependencies the moment your pager fires. PagerDuty
> triggers the incident — Mazkir writes the investigation. Your
> on-call engineer shows up to a war room that's already assembled."**

The second pitch:
- Targets a known buyer (Head of SRE / VP Infrastructure / Staff Platform Engineer)
- Targets an acute known pain (incident response is universally hated)
- Has a quantifiable ROI the buyer can defend to finance
- Is demoable in 60 seconds (side-by-side with a manual investigation)
- Doesn't require explaining MCP, knowledge graphs, or AI agents

**Once the customer is using Mazkir for MTTR, every other use case
(AI agent grounding, code intelligence, cost optimization, compliance)
becomes an additive upsell.** But the hook is MTTR.

---

## What needs to be built to ship this

Most of the pieces are already in the architecture. What's missing:

| Component | Status | Effort |
|---|---|---|
| Graph: code + runtime + ownership + history | Designed, partially built | Built in Phase 1 anyway |
| MCP tool surface | Built | Done |
| Incident Correlator agent | Designed (per `docs/runtime-layer-*.md`) | ~2 weeks |
| Datadog webhook integration | Needs building | ~1 week |
| PagerDuty webhook integration | Needs building | ~1 week |
| New Relic integration | Needs building | ~1 week |
| Past incident pattern matching | Needs building (LLM-powered) | ~2 weeks |
| Mitigation suggestion engine | Needs building | ~2 weeks |
| Slack integration for auto-posting | Designed | ~1 week |

**Total effort to deliver the killer MTTR story at production quality:
~8-10 weeks of focused engineering.** This is a Phase 2 build that
becomes the headline feature of the cloud tier launch.

**Most of this work was already on the roadmap** — we're just
reordering priorities so MTTR becomes the lead feature instead of
general AI agent grounding.

---

## Strategic implications

### 1. The roadmap should elevate MTTR to the primary Phase 1-2 value prop
Phase 1 ships reactive MCP. Phase 2 ships the Incident Correlator +
Slack integration. **Those two phases together deliver the MTTR
story.** Everything else is supporting infrastructure.

### 2. The sales motion changes
Instead of selling to developers (AI agent grounding), sell to SRE /
platform engineering leads (MTTR). The buyer is different, the budget
is bigger, the pain is more acute, and the ROI math is defensible.

### 3. The AWS Marketplace listing should lead with MTTR
The 80-word Marketplace description should hit MTTR reduction first,
with the other capabilities as supporting bullets.

### 4. The demo video should be an incident scenario
Side-by-side: "this is MTTR without Mazkir (45 min of tool-hopping),
this is MTTR with Mazkir (8 min of reading a Slack post)." The
comparison sells itself.

### 5. The onboarding story should start with an incident
Instead of "Sarah the developer installs Mazkir for fun," the
onboarding story should start with "it's 2 AM, the pager just fired,
and Mazkir just posted the full investigation before you opened your
laptop."

---

## How this document gets used

- **Marketing / sales:** this is the #1 pitch. Reference when writing
  landing page copy, demo scripts, sales decks, or case studies.
- **Product prioritization:** if a feature doesn't contribute to MTTR
  reduction or support the incident correlator, it's Phase 3+.
- **Engineering prioritization:** the components in the "what needs
  to be built" table are the Phase 2 must-ship list.
- **Customer research:** every customer discovery interview should
  probe MTTR experience. If they don't care about MTTR, they're
  probably not the ICP yet.
- **Competitive conversations:** when a customer asks "what about
  Datadog?" — the answer is "Mazkir sits above Datadog and joins
  across layers; Datadog doesn't know about your deploys, config
  changes, or IAM modifications."

If a proposed feature, hire, or partnership doesn't support the
MTTR story — defer it. The MTTR reduction is the wedge, and every
other capability should be positioned as compounding it.
