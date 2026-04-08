# Mazkir Roadmap to $1B

**Status:** Settled framework, decided 2026-04-08. The phase content
is opinionated and prescriptive. The two questions at the end
(ambition + capital strategy) determine which variant of this
roadmap to actually execute.

This document complements `docs/strategy-and-business-model.md`
(which covers licensing, pricing, trust framing) by adding the
**timing, sequencing, and execution plan** for the maximum-ambition
path.

---

## The thesis

> Mazkir becomes a $1B company by being **the structural memory
> layer for AI coding agents** — Python first, then TS/Go/Rust/Java
> to capture full-stack teams, then runtime state via K8s +
> OpenTelemetry, then non-code knowledge sources (Slack, Jira,
> Notion, meetings) to displace Glean for engineering use cases.

The moat is **not** language support. Language support is necessary
but not sufficient. The moat is the combination of:

1. **Structural correctness** — answers the question grep can't
2. **Metadata-not-source** — security and cost advantage no competitor has
3. **Federated multi-graph architecture** — sovereignty boundaries done right
4. **MCP-native** — first-class integration with the AI agent wave
5. **Runtime-aware joins** — "what's actually deployed and running" connected to source

Each piece individually has competitors. The combination is unique.

---

## The competitive window

~18-24 months before Cursor, GitHub Copilot, Glean, Sourcegraph, or
Anthropic add structural code intelligence as a feature in their
existing products. Mazkir's roadmap must ship faster than they wake
up to the gap.

| Player | Threat | Mitigation |
|---|---|---|
| **Cursor** | $100M ARR in 18 months. Distribution moat. Could add structural intelligence in 6 months. | Ship faster. Be in their MCP integration before they build their own. |
| **GitHub Copilot** | Microsoft owns LLM, IDE, repo. Adding structural awareness is a roadmap bullet. | Be the third-party tool customers ask GitHub to integrate, not compete with. |
| **Glean** | $4B valuation. Will add code as a "federated source." | Federation architecture means we can be the *code source* in their world, not compete head-on. |
| **Sourcegraph + Cody** | Established structural code search. Cody is the AI layer. | Better metadata-not-source story, native MCP, runtime layer they don't have. |
| **Anthropic / OpenAI** | Could ship native code intelligence. Would commoditize MCP servers. | Be deeply embedded in customer workflows so switching cost is real. |

The mitigation pattern is consistent: **ship the architectural depth
faster than they ship the surface-level features.**

---

## The 5 phases

### Phase 0 — Foundation (NOW → month 1)

**Theme:** Stop being a research project, become a real product.

| Deliverable | Detail |
|---|---|
| Rename | SynapCode → Mazkir, lock domains/handles |
| Distribution Phase 1 | `pip install mazkir` works on Linux x86_64 |
| Universal installer | `curl -fsSL get.mazkir.io \| sh` |
| Language honesty fix | TypeScript broken, Go/Rust/Java declared but not installed — either ship working or fail loudly |
| Marketing site | 3 killer use cases front and center: agent grounding, PR review (`diff_impact`), refactor safety |
| Secret scrubber polish | Required before any cloud tier launch |
| README rewrite | Show Mazkir answering one real question end-to-end in <10 seconds |

**Cost:** 1 person × 1 month
**Revenue:** $0
**Team:** founder
**Goal:** look like a product, not a side project

---

### Phase 1 — Wedge product (months 1-4)

**Theme:** Be the best Python structural intelligence tool. Win the
hearts of Python developers first.

| Deliverable | Detail |
|---|---|
| TypeScript working | Fix the `tree_sitter_typescript` import bug, add full TS extraction |
| 100 design partners | OSS Python maintainers (FastAPI, Django, Flask, Pydantic, Polars), AI/ML teams, Python-heavy startups |
| **Tauri desktop launcher** | Spacebar shortcut + visual graph explorer + click-to-explore. **The legibility breakthrough.** |
| Native MCP integrations | One-click setup for Claude Code, Cursor, Continue |
| Public launch | HN front page, r/Python, dev podcasts, Twitter |
| Free local tier | Ruthlessly complete, zero limits, never crippled |

**Cost:** 2 people × 4 months ≈ $80-120K
**Revenue:** $0 (free tier)
**Team:** founder + 1 eng
**Success metric:** 10,000 weekly active local users + 50% W4 retention

---

### Phase 2 — Cloud tier MVP (months 4-9)

**Theme:** Convert free users into paying teams.

| Deliverable | Detail |
|---|---|
| Hosted Mazkir | api.mazkir.io, multi-tenant FalkorDB with strict isolation |
| Webhook ingest | GitHub / GitLab on every push, auto-reindex |
| Web UI | Shared team graph, visual explorer, search-as-you-type |
| Team SSO | Google + GitHub OAuth, basic |
| Slack integration | Channel mention → graph context |
| Billing | Stripe, $20/dev/month, 15-day free trial |

**Cost:** 4 people × 5 months ≈ $250-400K
**Revenue target:** $100K MRR ($1.2M ARR run rate) — ~500 paid devs across ~50 orgs
**Team:** founder + 3 eng (no DR/sales yet)
**Success metric:** $100K MRR, NPS > 50

---

### Phase 3 — Multi-language + Layer 4 runtime (months 9-18)

**Theme:** Become irreplaceable. Add what competitors can't easily clone.

Two parallel tracks:

#### Track A: Language expansion

| Language | Months | Why |
|---|---|---|
| Go | 9-10 | Biggest backend market after Python |
| Rust | 10-11 | We use it ourselves, embarrassing not to support |
| Java | 11-13 | Enterprise demand |
| C# / Kotlin / Ruby | 13-18 | Long tail, customer-driven priority |

#### Track B: Runtime layer (per `docs/runtime-layer-*.md`)

| Capability | Months | Why |
|---|---|---|
| K8s operator + webhook ingest | 9-12 | The "what's deployed" question |
| OpenTelemetry trace integration → liveness fingerprints | 12-15 | The killer "is this code actually used in prod" feature |
| PagerDuty / incident integration | 15-18 | Incident triage use case |

**Cost:** 8 people × 9 months ≈ $1.5M
**Revenue target:** $1M MRR ($12M ARR run rate) — Business tier customers paying $50/dev/month
**Team:** ~10 people (5 eng, 1 design, 1 DevRel, 1 sales, 1 customer success, 1 founder)
**Success metric:** $12M ARR, expansion revenue from existing customers, first 5-10 enterprise inbound interest

---

### Phase 4 — Enterprise + Knowledge integrations (months 18-30)

**Theme:** Become the engineering knowledge graph nobody else has.

| Deliverable | Detail |
|---|---|
| Self-hosted | Helm chart for customer VPC deployment |
| TEE / Confidential Compute | Nitro Enclaves for paranoid customers |
| Compliance certifications | SOC 2 Type II, ISO 27001 |
| Identity / audit | SAML SSO, SCIM provisioning, SIEM export |
| Linear / Jira / Notion | Knowledge graph integrations |
| Meeting transcripts | Granola, Otter, Zoom ingestion |
| Federation server | Per `docs/federated-graph-architecture.md` |
| Verticalized bundles | "Mazkir for security audit", "Mazkir for refactor planning", "Mazkir for incident response" |

**Cost:** 25 people × 12 months ≈ $5-7M
**Revenue target:** $5M MRR ($60M ARR run rate) — 50+ enterprise contracts at $50-150K each + ~3000 mid-market team customers
**Team:** ~30 people
**Success metric:** $60M ARR, 5 reference customers in F500, repeatable enterprise sales motion

---

### Phase 5 — Platform / marketplace (months 30-48)

**Theme:** Become a platform other tools build on.

| Deliverable | Detail |
|---|---|
| Public API | For third-party agent developers |
| MCP tool marketplace | "Install the Mazkir-Datadog integration" |
| Cross-customer federation | For consultancies, contractors, audit firms |
| Verticalized products | Mazkir for Compliance, SRE, FinOps, Code Quality |
| International expansion | EU data residency, AsiaPac |
| Channel partnerships | Consulting firms |

**Cost:** 60 people × 18 months
**Revenue target:** $20M MRR ($240M ARR run rate) → 15-20x multiple = **$3-5B valuation**
**Team:** ~75 people
**Success metric:** Platform velocity, network effects starting, repeatable land-and-expand

---

## Three paths to $1B

### Path A: Organic growth
- Hit $80-150M ARR by end of Phase 4 / start of Phase 5
- 15x ARR multiple (typical for high-margin dev tools) = $1.2-2.2B valuation
- Year 3-4
- Requires execution on every phase without major missteps
- **Most predictable, hardest to actually execute**

### Path B: Strategic acquisition (most likely $1B exit)
- Microsoft, GitHub, Anthropic, Google, or Datadog acquires for $500M-2B
- Sometime between Phase 2 and Phase 4
- Cursor was acquisition-courted at $300M; multiplier is shrinking but real
- **Highest likelihood of "$1B-shaped outcome" overall**
- Triggers: clear traction in Phase 2 (cloud tier proof), enterprise interest in Phase 3, or strategic threat to an incumbent

### Path C: Category-defining outlier
- Mazkir becomes the *thing* AI agents query for code, the way Stripe became the thing apps query for payments
- Standard infrastructure, not a tool
- Valuation decouples from ARR multiple
- "What would replacing Mazkir cost everyone?" becomes the price
- **Hardest to predict, biggest payoff**

---

## The 4 things that have to be true

These are the load-bearing assumptions. If any of these fail, the
$1B path fails — settle for a different outcome.

1. **Ship the launcher within 6 months.** Without a visual UI nobody finds the tool, nobody understands what it does, and the press cycle dies on launch day. The launcher is the legibility breakthrough.

2. **Hit $1M ARR within 12 months of launching the cloud tier.** This is the proof point that converts free users to paid teams. If conversion is bad, the model doesn't work and you're capped at $20-50M ARR forever — a good lifestyle business, not a $1B company.

3. **Raise at least $5M by month 18.** You can't build Phase 3 (multi-language + runtime layer) on bootstrap savings. Each language is ~1 month of work; the K8s operator is multi-month; you need 5+ engineers, customer success, and a sales motion. Bootstrapping past Phase 2 is possible but slows you down enough that competitors catch up.

4. **Don't get pre-empted by Cursor or GitHub adding structural intelligence to their existing products.** The only one not entirely in your control. Mitigation: ship faster, build the architectural moat (federation + runtime + non-code) before they wake up to it, and make Mazkir indispensable to power users so they evangelize it for you.

---

## Top competitive risks

| Risk | Probability | Mitigation |
|---|---|---|
| Cursor / Windsurf adds structural intelligence | High | Speed; deep MCP integration first |
| GitHub Copilot bundles it natively | Medium | Be the integration option, not the competitor |
| Glean expands into code | High | Federation pattern lets us be the code source in their world |
| Sourcegraph + Cody bundles agent + structural search | Medium | Better runtime layer, better metadata story |
| Anthropic ships native code intelligence | Low-medium | Deep customer workflow embedding |
| Open-source clone of architecture emerges | Low | Closed source decision; proprietary MCP tool surface |
| MCP standard gets fragmented or replaced | Low | Tool surface is portable, not protocol-locked |
| AI agent market doesn't materialize as expected | Medium | The fallback is "very good code search tool" — still a $50M+ business |

---

## Two questions that determine which roadmap to actually run

These have to be answered before committing to any phase. The wrong
answer kills the right outcome later.

### Question 1: What's the actual ambition?

Three honest options:

| Ambition | Roadmap implication |
|---|---|
| **$1B company** | Execute the maximum-ambition path above. Ship fast, raise capital, hire aggressively. |
| **Lifestyle business at $5-20M ARR** | Skip Phase 3 onwards. Stay solo or 2-3 person team. Focus on Phase 1-2 indefinitely. Cap growth deliberately. |
| **Strategic acquisition at $50-300M** | Execute Phases 0-2 hard, then optimize for clarity-of-story instead of revenue. Make yourself attractive to acquirers in months 18-30. |

### Question 2: Bootstrap or raise capital?

| Path | Implication |
|---|---|
| **Bootstrap** | Caps at ~$20M ARR realistically. Skip Phases 3-5. Lifestyle business or small acquisition. Full control, no investor pressure. |
| **Raise** | Opens the $1B path. Phase 2 funded by seed, Phase 3+ requires Series A. Give up some control. |

The cloud tier (Phase 2 onwards) is the inflection point. Phase 1 can
be bootstrapped. Phase 3 cannot.

---

## Tomorrow's first irreversible step

Buy `mazkir.io`, `mazkir.cloud`, `mazkir.dev` + GitHub org + PyPI name.

5 minutes from a phone. Locks the brand identity before doing any of
the rest. This is the cheapest, fastest, most reversible-if-needed
commitment that proves the project is real.

---

## How this document gets used

- Reference before any "what should we build next" conversation
- Reference before any hiring decision (each phase has headcount targets)
- Reference before any fundraising conversation (must-be-true #3)
- Reference before any "should we pivot" conversation (the 4 must-be-trues are the test)
- Reference before any "should we add X" feature debate (does X serve the current phase's success metric?)

If a proposed feature or hire or partnership does not advance the
current phase's success metric, push back. Most startups die from
doing too many things, not too few.
