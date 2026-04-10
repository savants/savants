# SynapCode Business Plan — v3 (Free Local + Layered Cloud)

**Status:** Active as of 2026-04-07
**Previous directions preserved at:**
- Tag `v0.1.0-foss-checkpoint` / branch `foss-direction` — Open Core + Agentic SaaS
- See `docs/strategy-history.md` for the full pivot history

---

## One-line business

> **Free local-first code intelligence for solo devs. Paid cloud tiers (multi-tenant, confidential, dedicated, self-hosted) for teams. Five deployment modes off one core architecture. Bootstrap to $1M ARR; scale to $100M+ via enterprise self-hosted and vertical compliance.**

## What this IS

A **graph-native code intelligence platform** with five deployment modes off the same core codebase. The architecture: a **layered graph** that composes a cloud base (main branch), per-branch overlays, and local working-copy deltas — so users get accurate answers about uncommitted changes without ever uploading their code.

This is **the Cloudflare distribution model + the GitHub deployment model + 1Password's privacy guarantees + the Sublime Text customer-love model** — applied to code intelligence.

## What this is NOT

| Model | Why it's not us |
|---|---|
| **Cursor / Windsurf** (closed + server-heavy) | They burn $30-50M/year on LLM inference. We use BYO LLM keys → $0 inference cost. They own your code during sessions; we never have to. |
| **FOSS only** | Commoditizes in 6-12 months; cloud monetization fights FalkorDB Cloud; community adoption is slow. |
| **Just a database reseller** | FalkorDB Cloud already sells hosted FalkorDB. We sell the layer *above*: code understanding, team collaboration, agents, compliance. |
| **Cloud-only SaaS** (Linear's mistake) | Loses every regulated enterprise deal. We ship multiple deployment modes including air-gapped self-hosted. |
| **Pure local-only** | Local tier breaks at ~2,500 files (empirically measured on real OSS repos). Real teams need cloud. |

## The architecture (the moat)

### Layered graph composition

```
┌─────────────────────────────────────────────────────────┐
│ LAYER 3: Local Working Delta (ephemeral, in memory)     │
│  • The user's uncommitted changes                        │
│  • Computed on the fly by tree-sitter                    │
│  • Tiny (10-500 KB), never persisted anywhere            │
├─────────────────────────────────────────────────────────┤
│ LAYER 2: Branch Overlay (cloud, per-branch)             │
│  • Delta from main: what this branch has changed         │
│  • Built from webhooks when user pushes commits          │
│  • Updated incrementally                                 │
├─────────────────────────────────────────────────────────┤
│ LAYER 1: Cloud Base (main branch, shared team state)    │
│  • Authoritative graph for the repo's main branch        │
│  • Auto-updated on every push                            │
│  • Shared across the team                                │
└─────────────────────────────────────────────────────────┘
```

Every query composes all three layers in memory inside a TEE, runs against the composed virtual graph, returns the result. Users always see accurate answers including their uncommitted refactors. Local client is ~15 MB. Works on any repo size because the heavy lifting is in the cloud.

**See `docs/architecture-layered-graphs.md` for the full design.**

### Why this is the moat

1. **No competitor has this**: Cursor doesn't, Sourcegraph doesn't, GitHub doesn't, Aider doesn't. It's a category-defining architecture.
2. **Solves the "uncommitted changes" problem**: every other tool either uploads your whole repo or only sees committed state.
3. **Scales to any repo size**: cloud handles 100k+ file monorepos that local alone can't touch.
4. **Privacy-preserving**: working delta processed inside TEE, never persisted.
5. **Deployment-agnostic**: same architecture runs in multi-tenant cloud, dedicated cloud, or customer's own VPC.

## Deployment modes

Same core codebase, five packagings:

### Mode 1: Free Local
**Who:** Solo devs, hobbyists, students, evaluators
**Runs on:** Your laptop
**Data:** Entirely local, no cloud dependency
**Price:** **$0 forever**
**What it is:** Tauri desktop app + FalkorDB sidecar + local Temporal worker + MCP server
**Ceiling:** ~2,500 files (today) → ~15,000 files (after fixing the CALLS edge bug)
**Strategic role:** Marketing engine, viral distribution, brand-building

### Mode 2: Multi-Tenant Cloud
**Who:** Startups, small teams, devs comfortable with SaaS
**Runs on:** Our AWS infrastructure
**Data:** Encrypted at rest + in transit, multi-tenant FalkorDB cluster
**Price:** **$199/user/year**
**What it is:** Cloud base + branch overlays + thin local client (~15 MB)
**Strategic role:** Volume tier, the "everyone's first paid tier"

### Mode 3: Confidential Cloud
**Who:** Privacy-conscious teams, security-aware orgs, smaller regulated companies
**Runs on:** AWS Nitro Enclaves (TEE) — we cryptographically cannot read your data
**Data:** Encrypted with user-held keys, decrypted only inside the enclave for the duration of a query
**Price:** **$399/user/year**
**What it is:** Same as Multi-Tenant Cloud + confidential compute + remote attestation
**Strategic role:** The "we couldn't see your code if we tried" pitch — wins compliance-conscious deals

### Mode 4: Dedicated Cloud
**Who:** Mid-market enterprises, Series C+ startups, companies that want isolation without ops burden
**Runs on:** A separate AWS account we provision and manage per customer
**Data:** Single-tenant, isolated infrastructure, customer-specific encryption keys
**Price:** **$1,500-$2,500/user/year (min $50k/year)**
**What it is:** Dedicated FalkorDB cluster + dedicated Temporal + dedicated everything, managed by us
**Strategic role:** Revenue per customer is 10x multi-tenant; right tier for $20-200M companies

### Mode 5: Self-Hosted Enterprise
**Who:** Banks, healthcare, defense, governments, regulated enterprises
**Runs on:** Customer's own Kubernetes cluster, bare metal, or air-gapped network
**Data:** Never leaves customer infrastructure
**Price:** **$3,000-$6,000/user/year + setup fees + support contract**
**What it is:** Helm chart + Terraform modules + air-gapped installer + professional services
**Strategic role:** Highest deal size, where 30%+ of revenue lives

### Vertical Compliance Packs (overlay on Modes 4-5)
**Who:** Industries with specialized compliance requirements
**Price:** **$500k-$2M+ per deal** (annual)
**What it is:** Pre-baked compliance artifacts, regulatory reporting, audit templates, white-glove onboarding
**Verticals:**
- **FinServ:** PII tracing, PCI scope reduction, regulatory reporting templates
- **Healthcare:** HIPAA audit trails, PHI flow verification
- **Defense / Gov:** ITAR / FedRAMP moderate+, air-gapped installers
- **Automotive / Aerospace:** ISO 26262, DO-178C safety-critical traceability
**Strategic role:** Highest margin, most defensible — these deals last 5-10 years

## Pricing summary

| Tier | Price | Volume share | Revenue share |
|---|---|---|---|
| Free Local | $0 | ~70% of users | 0% |
| Commercial License | $49/year | ~10% | 2% |
| Multi-Tenant Cloud | $199/user/year | ~12% | 18% |
| Confidential Cloud | $399/user/year | ~5% | 12% |
| Dedicated Cloud | $1,500-$2,500/user/year | ~2% | 18% |
| Self-Hosted Enterprise | $3,000-$6,000/user/year | ~0.8% | 30% |
| Vertical Compliance | $500k-$2M/deal | ~0.2% | 20% |

**Notice:** ~1% of users generate ~68% of revenue. This is the GitLab / Atlassian / Cloudflare pattern.

## Why people pay (the upgrade triggers)

The free tier is genuinely complete for solo work. The paid tiers exist because **certain things are physically impossible without a cloud**:

| Trigger | Tier needed | Why |
|---|---|---|
| Joins a team using SynapCode | Multi-Tenant Cloud | Team graphs require a server two laptops can both reach |
| Uses multiple devices | Multi-Tenant Cloud | Cross-device sync requires cloud storage |
| Repo grows past local RAM (~2,500 files today) | Multi-Tenant Cloud | Physics — graph won't fit |
| Wants CI integration on PRs | Multi-Tenant Cloud | GitHub Actions can't reach a sleeping laptop |
| Needs SOC 2 attestation | Enterprise tier | Only enterprise tier ships audit logs + access controls |
| Bank legal team reviews tools | Commercial License at minimum | Honor system + EULA |
| Privacy team objects to "code on a SaaS" | Confidential Cloud | TEE proof addresses the objection |
| Network policy blocks outbound to SaaS | Self-Hosted Enterprise | Runs entirely in their VPC |
| FedRAMP / HIPAA / FinServ compliance | Vertical Compliance pack | Pre-baked artifacts for the audit |

**No artificial caps.** No "you can only run 5 queries per day on free." No nag screens. The upgrades are organic — driven by the user's situation changing, not by us forcing them.

## Empirical data: when local breaks

We profiled real OSS repos (results in `docs/profiling-results.md`):

| Repo | Files | Index time | CALLS edges | Verdict |
|---|---|---|---|---|
| flask | 83 | 1.7s | 5,350 | ✅ Free fits perfectly |
| fastapi | 1,121 | 8.4s | 17,006 | ✅ Free works great |
| django | 2,892 | **5 minutes** | 212,840 | 🟡 Pain point — users will upgrade |
| pytorch | 4,386 | **40+ min, didn't finish** | 13,012,045 | ❌ Local broken — must use cloud |

**The local tier breaks naturally at ~2,500 files.** That's the physical upgrade trigger for any team beyond the smallest. We don't need to artificially restrict — the algorithm hits its scaling wall and users have to move to cloud.

## Revenue projections

Bottom-up, conservative. Free Local launches Q1, Multi-Tenant Cloud Q2, manual Self-Hosted enterprise deals starting Q3, productized Self-Hosted in Y2, Confidential + Dedicated in Y2, Vertical packs in Y3.

| | Y1 | Y2 | Y3 | Y5 |
|---|---|---|---|---|
| Free users | 150,000 | 800,000 | 2,000,000 | 5,000,000 |
| Commercial Licenses | $49k | $343k | $980k | $4M |
| Multi-Tenant Cloud | $299k | $1,592k | $3,980k | $15M |
| Confidential Cloud | — | $599k | $1,995k | $12M |
| Dedicated Cloud | — | $400k | $2,500k | $18M |
| Self-Hosted Enterprise | $450k (3 manual) | $2,400k (12) | $12,000k (40) | $45M |
| Vertical Compliance | — | $750k (1) | $6,000k (6) | $25M |
| Graph API | — | — | $500k | $5M |
| **Total ARR** | **~$798k** | **~$6.08M** | **~$27.96M** | **~$124M** |

## Cost structure

**Year 1 fixed costs (~$8k):**
- Apple Developer Program: $99
- Windows EV cert: $400
- Keygen.sh: $588/year
- Domain + DNS: $20
- Email: $50
- Legal (EULA, entity): $1,500
- Stripe processing: ~$3,000 (on $400k revenue)
- Sentry: $0 (free tier)
- Plausible self-hosted: $0
- Cloud infra (Y1 small scale): ~$2,400/year

**Year 1 gross margin:** ~95%
**Year 1 profit:** ~$650k from 1-2 founders

**Year 2 infrastructure** (with Multi-Tenant Cloud + Confidential Cloud + early enterprise):
- AWS: ~$3,000/month → $36k/year
- AWS Nitro Enclaves: ~$500/month → $6k/year
- Total cloud: ~$42k/year on $6M revenue → 99.3% gross margin

**Year 5 infrastructure:** ~$500k/year on $124M revenue → 99.6% gross margin

These margins are higher than Cursor (which spends 70-100% on inference) because we BYO LLM keys and use confidential compute that scales linearly with revenue.

## Acquisition thesis

Closed AI dev tool acquisitions in the last 24 months:
- **Windsurf → Google: ~$2.4B** (July 2025, after OpenAI's reported $3B deal collapsed)
- **Cursor: $9B+ valuation** (rumored $50B round late 2025)
- **GitHub Copilot inside Microsoft: $7.5B** acquisition of GitHub
- **Sourcegraph: ~$2.6B valuation**

A SynapCode hitting $25-50M ARR by Year 3 with this architectural moat is in the same conversation. **Realistic 5-year exit valuation: $1-5B** depending on growth and category leadership.

## Why this is the right strategy (recap)

| Concern | How this strategy addresses it |
|---|---|
| "What if everyone stays free?" | Free is local-only and breaks at ~2,500 files. Real teams hit the wall organically. |
| "What if a FOSS clone appears?" | They can't replicate the layered cloud architecture or TEE infrastructure. Free tier preserves distribution. |
| "What if Cursor / GitHub ships this?" | We're the MCP context provider. They become our customers, not competitors. |
| "What about regulated enterprises?" | Self-Hosted + Vertical Compliance covers them. Five deployment modes = no customer turned away. |
| "What about privacy-conscious teams?" | Confidential Cloud (TEE) is a structural privacy guarantee they can't get anywhere else. |
| "What about huge monorepos?" | Cloud base handles them; local delta keeps the experience interactive. |
| "What about uncommitted changes?" | Layered graph composition — local delta merges with cloud base/overlay at query time. |
| "What about cost of acquisition?" | Free tier drives adoption at $0 marginal cost; conversion happens organically when users hit ceilings. |

## What we explicitly do NOT do

1. **Run LLM inference on our servers.** BYO API keys, always.
2. **Store user code in our cloud unencrypted.** Confidential compute or nothing.
3. **Take VC before $1M ARR.** Bootstrap forces discipline and preserves optionality.
4. **Cripple the free tier with feature gates.** Free is genuinely complete; the upgrades are physical, not artificial.
5. **Sell free user data.** Ever. Trust compounds; selling it kills the business.
6. **Enter the LLM model business.** We use Claude / GPT / open-weight models via BYO keys. We don't train models.
7. **Build a proprietary IDE.** We integrate via MCP into Claude Code, Cursor, Zed, VS Code. We don't fork VS Code.

## 90-day execution plan

### Days 1-14: Legal & infrastructure
- [ ] Make GitHub repo private
- [ ] Register savants.dev (verify trademark)
- [ ] Apply for Apple Developer Program
- [ ] Order Windows EV code-signing certificate
- [ ] Form business entity (LLC or Delaware C-corp)
- [ ] Set up Stripe + Keygen.sh
- [ ] Draft proprietary EULA
- [ ] Set up analytics (Plausible)

### Days 15-45: Product hardening
- [ ] **Fix the CALLS edge explosion bug** (raises local ceiling from ~2.5k to ~15k files)
- [ ] Port `graph/cpg.py` and `graph/query.py` to Rust (`rust-core/` crate, see task #6)
- [ ] Build the local delta computer (see task #4)
- [ ] Add license validation to Tauri boot sequence
- [ ] First-run onboarding flow
- [ ] Implement 14-day trial mode
- [ ] Build and sign binaries for macOS / Windows / Linux
- [ ] Set up automated update mechanism (Sparkle / Squirrel)

### Days 46-60: Marketing assets
- [ ] Landing page: hero, features, pricing, demo video
- [ ] 60-second product demo video
- [ ] Launch blog post: "SynapCode — code intelligence that physically cannot see your code"
- [ ] Technical blog post: layered graph architecture
- [ ] HN, Reddit, Twitter draft posts

### Days 61-90: Launch & iterate
- [ ] Soft launch to ~50 beta users
- [ ] HN launch (Tuesday 9am EST)
- [ ] Collect feedback, ship updates weekly
- [ ] First 100 paying customers
- [ ] First enterprise inbound (manual self-hosted deal)

**Month 6 target:** $200k ARR
**Month 12 target:** $800k ARR
**Year 2 target:** $6M ARR

## Decision log

| Date | Decision | Why |
|---|---|---|
| 2026-04-07 | Pivot from Open Core to Sovereign Closed | Local-first pitch is stronger closed; FOSS commoditizes |
| 2026-04-07 | Add free local tier (Cloudflare model) | Distribution beats artificial paywalls; free → cloud is the natural funnel |
| 2026-04-07 | Add 5-tier deployment (multi-tenant → self-hosted) | Linear's cloud-only mistake; enterprises need on-prem |
| 2026-04-07 | Adopt confidential compute for cloud tiers | Architectural privacy beats policy promises |
| 2026-04-07 | Adopt layered graph architecture (base + overlay + delta) | Solves uncommitted-change problem without uploading code |
| 2026-04-07 | Plan Rust port of indexer hot path | Local tier needs single-binary distribution; 5-10x faster indexing |
| TBD | Public launch | After Apple/Windows cert + Rust port + demo video |
| TBD | First enterprise deal | Q3-Q4 Y1 (manual deployment) |
| TBD | Productized self-hosted (Helm chart) | Y2 Q1, after learning from manual deals |
| TBD | First capital raise | After $1M ARR, only if scaling enterprise requires it |

## Strategic optionality preserved

Even in Sovereign Closed, three exits remain open:

1. **Open older versions** (JetBrains pattern): Release year-old versions as Apache 2.0 once enterprise revenue is solid.
2. **Partial open core**: If FOSS adoption matters more in Y3+, release the core CLI as OSS while keeping cloud closed.
3. **Acquisition exit**: Google, Anthropic, Microsoft, GitHub, Sourcegraph, Atlassian all potential acquirers. Closed source + clean IP = acquirable.

The `foss-direction` branch + `v0.1.0-foss-checkpoint` tag preserve the alternate-universe FOSS plan.
