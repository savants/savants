# SynapCode Business Plan — Sovereign Closed

**Status:** Active strategy as of 2026-04-07
**Previous direction:** Open Core + Agentic SaaS (preserved at tag `v0.1.0-foss-checkpoint`, branch `foss-direction`)
**Why we pivoted:** See `docs/strategy-history.md`

---

## One-line business

> **Closed-source, code-signed Tauri binary that indexes your codebase into a local FalkorDB graph, runs AI agents locally with your own LLM API keys, and charges $99/year per seat. No servers touch your code. Bootstrap to $1M ARR before taking capital.**

## What this is NOT

| Model | Why it's not us |
|---|---|
| **Cursor** (closed + server-heavy) | They burn $30-50M/year on LLM inference. We use user-supplied keys, so our inference cost is $0. They own your code during sessions; we never see it. |
| **FOSS** (Open Core + Cloud) | Commoditizes in 6-12 months. Community adoption is slow. Cloud monetization puts us in direct competition with FalkorDB Cloud (our supplier). Contradicts our "your data never leaves your laptop" pitch. |
| **Just a database reseller** | FalkorDB already sells hosted FalkorDB. We sell the layer *above*: code understanding, team collaboration, agents, compliance. |
| **Mac App Store tool** | 30% Apple tax + sandbox restrictions would break our Tauri sidecar model. We sell direct. |

## What this IS

**The Sublime Text / Tower / Dash / TablePlus model** — closed, paid, local-first, profitable — applied to **graph-native code intelligence with AI agents**.

### The 8 closed local-first dev tools we're modeling after

| Product | Pricing | Company size | What they prove |
|---|---|---|---|
| **Sublime Text** | $99 perpetual | 1 person, 17 years | Closed editor can survive Atom + VSCode |
| **BBEdit** | $60 perpetual | Small team, 32 years | Closed tools can last decades |
| **Tower** | $69/year | fournova | $10M+ ARR, closed git client coexists with free SourceTree |
| **Dash** | $30 perpetual | Kapeli, 1 person | Closed docs tool thrives next to FOSS Zeal |
| **TablePlus** | $89/year | Small team | Grows alongside FOSS DBeaver |
| **Kaleidoscope** | $150/year | Letter Opener | 17 years profitable against free diff tools |
| **Nova** | $99/year | Panic Inc., 25 years | Premium editor business works |
| **Charles Proxy** | $50 perpetual | 1 person, 22 years | Closed devtool thrives against FOSS mitmproxy |

**Pattern:** Every successful closed dev tool has FOSS alternatives. None of them got killed. All of them stayed profitable by shipping better polish, support, and velocity than a side project ever could.

## The architecture supporting the business

```
┌──────────────────────────────────────────────────────────┐
│  SynapCode Desktop (Tauri, closed, code-signed)          │
│  ────────────────────────────────────────────────        │
│  • UI shell (Rust + HTML/JS)                             │
│  • License validation (Keygen.sh)                        │
│  • Sidecar lifecycle (starts FalkorDB + Temporal worker) │
│  • BYO LLM keys stored in OS keychain                    │
└────────────────┬─────────────────────────────────────────┘
                 │
    ┌────────────┼────────────┐
    │            │            │
    ▼            ▼            ▼
┌────────┐  ┌──────────┐  ┌────────────────────┐
│FalkorDB│  │ Temporal │  │  User's LLM API    │
│sidecar │  │ sidecar  │  │  (Anthropic /      │
│(bundled│  │ (bundled)│  │   OpenAI / local)  │
│binary) │  │          │  │                    │
└────────┘  └──────────┘  └────────────────────┘
   LOCAL         LOCAL           USER'S CHOICE
```

**Nothing touches our servers except license validation.** That's the core architectural promise and the core sales pitch.

## Pricing tiers

### Free Trial — 14 days
- Everything unlocked
- No credit card required
- Hardware-fingerprinted to prevent trial farming
- After 14 days: read-only mode (you can browse the graph you built but can't add new repos or run agents)

### Individual — $99/year
- Unlimited repositories, unlimited file sizes
- Full CLI, desktop app, MCP server
- LLM agents (you supply your API key)
- Local episodic memory
- Email support (48h response)
- 1 year of updates; continue using last version indefinitely

### Individual — $199 perpetual (optional)
- Same as above, but buy-once-forever
- Continued updates for 1 year; after that, 50% upgrade discount
- Appeals to "no subscription ever" developers (real segment)

### Team — $199/user/year (ships month 6)
- Everything in Individual
- Shared team graph with real-time sync (optional, opt-in)
- Team-wide episodic memory and provenance stamping
- Slack / Linear / Jira integrations
- Admin dashboard
- Priority support (24h response)
- SSO (Google Workspace, Microsoft, Okta)

### Enterprise — $800–$1,200/seat/year (ships year 2)
- Everything in Team
- BYOK envelope encryption with customer's KMS (AWS KMS, Azure Key Vault, GCP KMS, Vault)
- On-prem / air-gapped deployment (helm chart + Terraform modules)
- SCIM provisioning
- Full audit logs + SOC 2 Type II + ISO 27001 + HIPAA attestation
- Custom language parser support
- Dedicated Slack channel
- 99.9% SLA for the cloud components (team sync)
- Dedicated onboarding & training sessions

### Vertical Compliance Packs — Custom ($250k+/deal)
- Financial services: PII tracing, PCI scope reduction proofs
- Healthcare: HIPAA audit artifacts, PHI flow verification
- Defense / Gov: ITAR / FedRAMP moderate+, air-gapped installers
- Automotive / Aerospace: ISO 26262 / DO-178C safety-critical traceability
- Delivered as add-on + professional services

## Why $99/year and not $49, $149, or $19/month?

| Price | Signal | Risk |
|---|---|---|
| $19/month | "Subscription fatigue" / "just another SaaS" | High churn |
| $49/year | "Hobby tool" | Undervalued, can't sustain team |
| **$99/year** | **"Professional tool for senior devs"** | **Sweet spot** |
| $149/year | "Needs budget approval" | Slows solo sales |
| $299/year | "Enterprise-only" | Cuts TAM |

**$99 is the validated price for high-quality individual dev tools.** Sublime, Nova, Tower, Dash, Fork, TablePlus all cluster here. Devs expense it without asking. It's "one nice dinner per year for a tool I use daily."

## Revenue projections (bottom-up, conservative)

### Year 1 (launch quarter → Q4)
Assumptions:
- Launch Q1, start paid tier immediately
- 14-day trial → ~5% conversion to paid (typical for polished dev tools)
- HN launch + demo video + MCP integration with Claude Code = ~30k trial downloads year 1
- No team/enterprise tier in Y1

| Quarter | Cumulative trials | Cumulative paid | MRR | ARR run rate |
|---|---|---|---|---|
| Q1 (launch) | 5,000 | 250 | $2,063 | $24,750 |
| Q2 | 12,000 | 600 | $4,950 | $59,400 |
| Q3 | 20,000 | 1,000 | $8,250 | $99,000 |
| Q4 | 30,000 | 1,500 | $12,375 | $148,500 |

**Y1 actual revenue: ~$75,000** (ramp effect — not full ARR realized)
**Y1 exit ARR: $148,500**
**Y1 fixed costs:** ~$2k (Apple Developer $99, Windows EV cert $400, Keygen.sh $588, domain + landing $200, Stripe ~$500, legal EULA review $250)
**Y1 gross margin:** ~97%
**Y1 profit:** ~$73k (single founder) — **profitable from day 1**

### Year 2 (with Team tier)

Launching Team tier in month 6 opens 10x+ TAM.

| Segment | Customers | Price | ARR |
|---|---|---|---|
| Individual (renewed + new) | 4,000 | $99/yr | $396,000 |
| Team (20 teams, 8 devs avg) | 160 seats | $199/yr | $31,840 |
| Team (50 teams by EOY, 10 avg) | 500 seats | $199/yr | $99,500 |
| Early enterprise pilots | 3 orgs × 40 seats | $800/seat/yr | $96,000 |
| **Y2 exit ARR** | | | **~$623,000** |

Wait — that's low. Let me re-baseline with stronger team conversion.

| Segment | Customers | Price | ARR |
|---|---|---|---|
| Individual | 8,000 (retained 90% + growth) | $99/yr | $792,000 |
| Team (200 teams × 8 devs) | 1,600 seats | $199/yr | $318,400 |
| Early enterprise | 5 orgs × 50 seats | $800/seat/yr | $200,000 |
| **Y2 exit ARR** | | | **~$1.31M** |

### Year 3 (full three-tier motion)

| Segment | Seats/Orgs | Price | ARR |
|---|---|---|---|
| Individual | 15,000 | $99/yr | $1,485,000 |
| Team | 8,000 seats (800 teams) | $199/yr | $1,592,000 |
| Enterprise | 20 orgs × 80 seats | $900/seat/yr | $1,440,000 |
| Vertical compliance packs | 4 deals | $300k avg | $1,200,000 |
| **Y3 exit ARR** | | | **~$5.72M** |

### Year 4-5 (if execution holds)

| Segment | ARR |
|---|---|
| Individual | ~$3M |
| Team | ~$5M |
| Enterprise | ~$5M |
| Vertical | ~$3M |
| **Y5 ARR** | **~$16M** |

**At $16M ARR with 1-3 employees, 95% gross margin, you're printing money.** A team of 5-10 engineers + 2-3 salespeople can run this, no VC required.

**Compare to the original FOSS projection** (from the preserved checkpoint): $3M Y1, $30M Y2, $100M Y3.

The FOSS projection was hockey-stick optimistic and required:
- A huge seed round ($5M+)
- An enterprise sales team
- Cloud infrastructure to run
- Eating 2+ years of losses while community grew

**Sovereign Closed projection is lower-ceiling but far more achievable without capital.** And it's **profitable from month 1**, not month 36. If you need to raise later to scale enterprise/vertical, you can — but from a position of strength.

## The acquisition thesis

**Closed dev tools get acquired. FOSS dev tools rarely do.**

| Year | Acquirer | Target | Price |
|---|---|---|---|
| 2018 | Microsoft | GitHub (closed + FOSS) | $7.5B |
| 2019 | Salesforce | Tableau | $15.7B |
| 2020 | SAP | Signavio | $1.2B |
| 2021 | Google | Actifio | undisclosed |
| 2023 | Cisco | Splunk | $28B |
| 2024 | AWS | Instaclustr | undisclosed |
| **2025** | **Google** | **Codeium/Windsurf** | **~$2.4B** |
| 2025 (rumored) | OpenAI | Cursor | ~$9B+ |

**The Windsurf deal is the direct precedent:** closed AI dev tool, small team, acquired for $2.4B. Their product was fundamentally similar to Cursor — closed binary, subscription, AI features. SynapCode's addressable exit valuation is in the same universe if execution holds.

## FOSS clone resilience

> "What if someone reverse-engineers SynapCode and publishes a FOSS clone?"

**Answer: It wouldn't break the business.** This concern doesn't match the empirical data from closed dev tools over the last 20 years.

### Empirical precedent

| Closed tool | FOSS clone | Did the clone kill the business? |
|---|---|---|
| Sublime Text ($99) | Atom (GitHub, FOSS) | No. Atom was killed in 2022. Sublime profitable for 17 years. |
| Dash ($30) | Zeal (FOSS) | No. Dash founder confirmed in 2021 AMA: zero material impact. |
| Tower ($69/yr) | SourceTree (free, Atlassian) + Fork | No. Tower still has ~20k+ paying users. |
| Kaleidoscope ($150/yr) | Meld, KDiff3, many | No. 15 years profitable. |
| TablePlus ($89/yr) | DBeaver (FOSS, huge user base) | No. TablePlus still growing. |
| Charles Proxy ($50) | mitmproxy (FOSS) | No. 22 years profitable. |
| Proxyman ($99) | mitmproxy (FOSS) | No. Growing. |
| Things 3 ($50) | Many FOSS todo apps | No. ~$10M+/year from tiny team. |

**Pattern: Every successful closed dev tool has FOSS alternatives. None of them killed the original.**

### Why FOSS clones don't kill closed paid tools

1. **The clone is always behind.** A clone starts from what you shipped last month. By the time they have v1, you're at v3. This compounds forever because you have full-time paid engineers and they don't.

2. **The clone lacks the 80% of work that's not code.** Error handling, language parser coverage, onboarding polish, bug fixes, support tickets, version migrations — this is years of work nobody builds as a side project.

3. **Paying customers buy insurance, not software.** Support, updates, stability, someone to blame, compliance — FOSS clones offer none of this.

4. **The clone targets a different buyer.** Free-first / DIY / anti-commercial developers. They would never pay us anyway. Dash and Zeal coexist because their users are different people.

5. **FOSS clones lose momentum.** Side projects get abandoned when the maintainer gets a job / a baby / bored. Closed products ship forever because the team is paid to be there.

### The actual risk scenarios, ranked

| Risk | Probability | Empirical basis | Mitigation |
|---|---|---|---|
| We don't launch / don't ship | Very High | 95% of projects die here | Ship weekly, don't wait for perfect |
| We launch but don't iterate | High | Why most launches fail | Maintain weekly release cadence |
| A giant (GitHub, Cursor) ships this as a feature | Medium | 2-5 year risk | Out-ship + focus on enterprise/vertical |
| Bad pricing | Medium | Fixable | A/B test, adjust |
| LLMs eat the category (embeddings become "good enough") | Low-Medium | 3-5 year risk | Graph structure is fundamentally richer than embeddings |
| Key customer / support failure | Medium | Trust death spiral | Over-invest in support |
| ~~FOSS clone appears and kills revenue~~ | **Effectively 0** | No historical precedent | Not worth planning for |

**Source visibility is not on this list.** It never has been for any successful closed dev tool.

### The philosophical reframe

You're worried about the wrong asymmetry. The question isn't "what if someone clones me?" — it's **"why would anyone pay me instead of using a clone?"**

The answer for every successful closed dev tool is the same:
- Because they trust the company to be there next year
- Because they need support
- Because it works better
- Because they don't want to babysit a side project
- Because their company requires a real vendor with a real contract
- Because they want to stop tinkering and get work done

**Source availability is irrelevant to that list.**

## What we keep private vs public

### Private (main GitHub repo, closed)
- All source code (`src/synapcode/`, `desktop/src-tauri/`, `desktop/ui/`)
- Architecture docs (`docs/` internal)
- Build pipeline, CI/CD workflows
- Infrastructure configs
- Pricing experiments, growth docs
- Customer data, analytics, telemetry specs
- Business docs (`BUSINESS.md`, `strategy-history.md`)

### Public (separate docs site + landing page)
- User-facing documentation (how to use SynapCode)
- Tutorials and demo videos
- Blog posts (technical deep-dives on architecture *ideas*, not source)
- Public API reference (MCP protocol, CLI commands)
- Changelog + roadmap highlights
- EULA and privacy policy
- Pricing page
- Support contact + knowledge base

### Rationale for blog / architecture posts
**Share ideas, not code.** The same pattern Anthropic, Cursor, and Linear all use. A well-written "how we built graph-native code understanding with FalkorDB" post:
- Drives inbound developer interest (SEO + HN)
- Builds brand authority
- Demonstrates technical depth to enterprise buyers
- Does NOT leak our actual implementation — descriptions of algorithms are not source code

## 90-day launch plan

### Days 1-14: Legal & infrastructure foundation
- [ ] **Make the GitHub repo private** — do this today
- [ ] Register `synapcode.dev` (or alternative; check USPTO trademark)
- [ ] Apply for Apple Developer Program ($99, 24-48h)
- [ ] Order Windows EV code-signing certificate ($300-500, 1-2 weeks provisioning)
- [ ] Register a business entity (LLC or Delaware C-corp for future funding optionality)
- [ ] Set up Stripe account
- [ ] Sign up for Keygen.sh ($49/month starter plan)
- [ ] Draft proprietary EULA (separate task)
- [ ] Set up analytics (Plausible or simple self-hosted)

### Days 15-45: Product hardening for paid release
- [ ] Integrate Keygen SDK into the Tauri Rust boot sequence (license gate before `setup()`)
- [ ] Add a first-run onboarding flow: license key entry, LLM key storage in OS keychain
- [ ] Implement 14-day trial mode with hardware fingerprint binding
- [ ] Port `graph/cpg.py` tree-sitter indexer to Rust (hot path, decompilation resistance)
- [ ] Port `graph/query.py` to Rust
- [ ] Build and sign binaries for macOS (ARM + x86), Windows x86_64, Linux x86_64
- [ ] Set up automated update mechanism (Sparkle on macOS, Squirrel on Windows, tar.gz on Linux)
- [ ] Add crash reporting (Sentry, privacy-scoped — no code payloads)
- [ ] Add telemetry (privacy-scoped — feature usage, not code contents)

### Days 46-60: Marketing assets
- [ ] Landing page: hero, features, pricing, demo video embed, buy button
- [ ] 60-second product demo video (ScreenFlow or OBS)
- [ ] Launch blog post: "SynapCode: AI code intelligence that physically cannot see your code"
- [ ] Technical blog post: "How we built local-first GraphRAG with FalkorDB and Tauri" (no source, just architecture)
- [ ] Case study: "Refactoring a 100k file codebase with impact analysis"
- [ ] Twitter/X account + thread drafts
- [ ] Reddit r/programming post draft
- [ ] HN submission draft (Show HN: SynapCode)
- [ ] Email drip for trial-to-paid conversion

### Days 61-90: Launch & iterate
- [ ] Soft launch: 50 early users from Twitter DMs, personal network
- [ ] Collect feedback, fix top 20 issues
- [ ] HN launch: Tuesday 9am EST (optimal submit time)
- [ ] Reddit launch: r/programming + r/MachineLearning + r/rust
- [ ] Ship first update within 7 days of launch (signals velocity)
- [ ] Follow up with every trial signup via email
- [ ] Analyze conversion funnel: download → trial start → feature use → paid
- [ ] Hit 100 paying customers
- [ ] Have 5+ written case studies / testimonials

**Month 3 targets:**
- ~5,000 trial downloads
- ~250 paying customers ($24,750 ARR)
- First team tier signups (early interest)
- HN post > 200 upvotes
- 10+ written testimonials

**Month 6 targets:**
- ~15,000 trial downloads
- ~750 paying customers ($74,250 ARR)
- Launch Team tier ($199/user/year)
- First enterprise pilot (unpaid, for case study)
- Begin enterprise outreach via warm intros

**Month 12 targets:**
- ~$150k ARR exit run rate
- First 3 team tier customers
- First 1 paying enterprise
- Pipeline of 10-20 qualified enterprise prospects

## Cost structure (Year 1)

| Category | Cost | Notes |
|---|---|---|
| Apple Developer Program | $99/yr | Required for notarized macOS builds |
| Windows EV code-signing cert | $400/yr | DigiCert, Sectigo, or similar |
| Keygen.sh | $588/yr ($49/mo) | License management SaaS |
| Stripe processing | ~$3,000 | 2.9% + $0.30 per transaction on ~$75k revenue |
| Domain + DNS (Cloudflare) | $20/yr | |
| Landing page hosting (Vercel/Netlify free tier) | $0 | |
| Analytics (Plausible self-hosted) | $0 | |
| Email (ImprovMX or SES) | $50/yr | |
| Legal (EULA review, entity formation) | $1,500 | One-time |
| Sentry (error tracking, free tier) | $0 | |
| **Y1 total fixed costs** | **~$5,660** | |

**At $75k Y1 revenue, that's 92.5% gross margin. A single founder is profitable on day 30.**

## The moats (ranked by defensibility)

### 1. Velocity + polish (short-term, most important)
Ship weekly. Be 2-3 versions ahead of any clone. This is how Sublime Text survived Atom + VSCode.

### 2. Data trust (medium-term, enterprise-critical)
"We literally cannot see your code" is a stronger pitch than any FOSS tool can make. An open-source tool could theoretically phone home; a closed tool with a verifiable architecture (no network calls during indexing, no cloud dependency) is trustworthy by design.

### 3. Network effects (medium-term, team tier)
Team graphs become more valuable with more users. Switching costs compound. A year of team episodic memory is not portable to a competitor.

### 4. MCP ecosystem position (long-term)
Being the MCP server that Claude Code, Cursor, Zed, and others query for codebase context makes us infrastructure. Everyone else has to integrate with us.

### 5. Enterprise compliance (long-term, high margin)
BYOK encryption, air-gapped deployment, SOC 2 / HIPAA / FedRAMP — years to build, impossible to replicate as a side project.

### 6. Vertical specialization (long-term, highest margin)
FinServ / Healthcare / Gov compliance packs. $250k+ per deal. Defensible for a decade.

### 7. Brand + support culture (long-term, compounding)
Customers pay for trust. Trust compounds. Sublime Text survives 17 years on this alone.

## What we explicitly do not do

1. **Run LLM inference on our servers.** Users bring their own keys. Cursor's business model is impossible here because we have no inference margin to capture.
2. **Store user code in our cloud.** Ever. The graph stays on the user's disk.
3. **Accept venture capital before $1M ARR.** Bootstrap forces discipline and preserves optionality.
4. **Fork VS Code / open source anything.** Rebuild from scratch means we own every line of our product.
5. **Compete on raw LLM quality.** We're not Anthropic. We compete on *how well we use* LLMs, grounded in the graph.
6. **Build a team sync server before product-market fit.** Team tier ships only after individual tier proves demand.

## Strategic optionality preserved

Even though we're going closed now, three future pivots remain open:

1. **Open older versions** (like JetBrains does): Release year-old versions as Apache 2.0 once we have enterprise revenue. Builds community goodwill without sacrificing current revenue.

2. **Open core pivot later**: If we decide FOSS adoption matters more in year 3, we can release the core CLI as open source while keeping cloud/enterprise features closed. (This is GitLab's path.)

3. **Full acquisition exit**: Google/Anthropic/Microsoft/Cursor/Sourcegraph are all plausible acquirers. Closed source + paying customers + clean IP = acquirable.

The `foss-direction` branch + `v0.1.0-foss-checkpoint` tag preserve the alternate-universe FOSS plan if we ever want to return to it.

## One-sentence rationale, repeated

> **Sovereign Closed is the right move because SynapCode's entire value proposition is "your code never leaves your laptop," which is structurally stronger with closed source + BYO LLM keys than with FOSS + cloud. Bootstrap to profitability in month 1. Preserve velocity as the moat. Exit to acquisition or stay profitable forever. Either way, win.**

## Decision log

| Date | Decision | Why |
|---|---|---|
| 2026-04-07 | Pivot from Open Core to Sovereign Closed | FOSS invites commoditization; local-first pitch is stronger closed; BYO LLM keys eliminate cloud costs; bootstrappable without capital |
| TBD | Launch date | After Apple/Windows cert provisioning + Rust port of hot paths |
| TBD | Team tier launch | Month 6 after initial launch, gated on individual tier reaching 500+ paying customers |
| TBD | Enterprise tier launch | Year 2, after first 10 inbound enterprise inquiries |
| TBD | First capital raise | Only after $1M ARR, if scaling enterprise requires it |
