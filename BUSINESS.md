# SynapCode Business Plan

## What business is this?

**Open Core + Agentic SaaS.** Not Cursor's model. Not a database company. Specifically: the Sourcegraph playbook (team code intelligence) powered by the Cursor playbook (LLM agents as the premium feature), built on the Temporal playbook (open source core + managed cloud + enterprise tier).

## Why not Cursor's model?

Cursor is **closed-source, server-heavy, fork-based**. It works because:
- They fork VS Code for instant distribution
- They train custom models (Cursor Tab, Composer) — expensive infra moat
- Everything touches their servers (no local-only mode)
- They have massive capital to out-ship FOSS competitors

**SynapCode has none of these advantages.** Its defining promise is "local-first, your data stays on your machine, FalkorDB runs on your hardware." Closing the source actively contradicts the pitch. A closed local-first code indexer gets crushed in HN comments the day it launches.

## What the business actually is

```
┌────────────────────────────────────────────────────────┐
│  Team Collaboration + Agents + Enterprise Compliance   │  ← $$$ layer
├────────────────────────────────────────────────────────┤
│  Code Property Graph + Episodic Memory + Queries       │  ← FOSS (the moat via adoption)
├────────────────────────────────────────────────────────┤
│  FalkorDB (infrastructure we consume, don't sell)      │  ← not our business
└────────────────────────────────────────────────────────┘
```

SynapCode is **not a database company**. FalkorDB already sells hosted FalkorDB. You're selling the layer *above* it — code understanding, team collaboration, agents, and enterprise compliance. Same relationship Linear has to Postgres.

## Product Tiers

| Tier | Price | What you get | Who buys |
|---|---|---|---|
| **Free** | $0 forever | CLI, desktop app, full graph indexing, impact analysis, MCP server, single-user, local-only | Individual devs, OSS contributors, evaluators |
| **Pro** | $19/user/month | Agent features (LLM-powered queries), historical graph (episodic memory), cross-session memory, cloud sync of your personal graph | Power users, consultants, senior ICs |
| **Team** | $39/user/month | Shared team graph, collaborative impact analysis, team-wide episodic memory, Slack/Linear/GitHub integrations, provenance stamping, team audit | 5-500 dev teams |
| **Enterprise** | $500-$1,000/seat/year | BYOK envelope encryption, SSO + SCIM, on-prem / air-gapped deployment, audit logs, SOC 2 / HIPAA / FedRAMP, dedicated support, SLAs, custom parsers | 500+ dev orgs, regulated industries |
| **Vertical** | Custom ($250k+/deal) | Financial services PII tracing, healthcare HIPAA, defense / ITAR, automotive ISO 26262 compliance packs | Fortune 500 in regulated verticals |

## Revenue model

**Primary:** Per-seat recurring subscriptions. Team tier drives volume, Enterprise + Vertical drive contract value.

**Secondary:**
- **Agent usage fees** (Pro/Team) — usage-based on expensive LLM calls, similar to Cursor's "fast requests" model
- **Marketplace revenue share** — cut on paid third-party integrations, plugins, language parsers
- **OEM / white-label licensing** — 6-7 figure deals with other dev tool companies (GitHub Copilot Enterprise, Snyk, Datadog, etc.)
- **Professional services** — enterprise onboarding, custom deployment, vertical compliance packs

## Revenue projections (bottom-up, realistic)

Assuming FOSS launch drives 100k users in year 1 (credible for a dev tool with a strong HN launch — ollama, aider, uv all hit this):

| Segment | Y1 count | Price | Y1 ARR |
|---|---|---|---|
| Free individual | 100,000 | $0 | $0 |
| Pro individual (3% convert) | 3,000 | $19/mo | ~$684k |
| Team (10 devs × 200 teams) | 2,000 seats | $39/mo | ~$936k |
| Enterprise (10 deals × 100 seats) | 1,000 seats | $800/seat/yr | ~$800k |
| Vertical (3 compliance deals) | — | $250k/deal | ~$750k |
| **Y1 ARR** | | | **~$3.2M** |

**Year 2 with network effects + enterprise pipeline: $15-30M ARR**
**Year 3 with vertical penetration + OEM deals: $60-100M ARR**

Comparable companies at these stages:
- Sourcegraph: $100M ARR selling team code intelligence
- Sentry: $400M ARR selling open core + SaaS
- Temporal: ~$50M+ ARR selling open source + cloud
- GitLab: $700M+ ARR on open core + enterprise

## Why this works (the moats)

### 1. Distribution moat: FOSS
The CLI + desktop app are Apache 2.0. Free forever. This drives viral adoption on HN, Reddit, dev Twitter. You become the default "grep-but-for-architecture" tool.

### 2. Data moat: Team graphs compound
Once a team has 6 months of episodic memory (who changed what, why, what was valid when), switching costs become enormous. The graph gets smarter the longer it's used. You can't export someone else's 6 months of team decisions.

### 3. Integration moat: MCP server
You become the "context provider" for Claude Code, Cursor, Codex, and every future AI agent. When other tools ask "what calls this function?", they ask SynapCode. That's infrastructure you own.

### 4. Compliance moat: Enterprise features
BYOK encryption, air-gapped deployment, audit logs, FedRAMP — these take years to build and certify. FalkorDB Cloud can't ship them (they're infrastructure). Cursor can't ship them (they're SaaS-only). This is wide-open.

### 5. Vertical moat: Specialized solutions
Financial services compliance tracing, healthcare HIPAA, defense ITAR. These verticals pay 10-20x for the same underlying tool because nobody else ships it and the buyers have big budgets.

## What NOT to monetize

1. **Hosted FalkorDB** — FalkorDB already sells this, you'd lose on margin
2. **The free CLI** — it's your adoption engine, keep it free forever
3. **Basic impact analysis** — commoditizes fast, give it away
4. **Solo developer features** — charge only for team/cloud/agent value

## Open / Closed split

### Apache 2.0 (public GitHub repo)
- `src/synapcode/` — Python CLI, graph, CPG, pipelines, MCP server
- `desktop/` — Tauri shell + sidecar management
- Documentation, examples, tests
- Community plugins and language parsers

### Private / commercial (separate repo)
- Cloud orchestration (multi-user graph sharing, team sync, cloud bursting)
- BYOK envelope encryption + KMS integration
- Audit logs, SSO, SCIM, RBAC
- Enterprise deployment tooling (Helm charts, Terraform modules)
- The hosted service running on your infrastructure
- Vertical compliance packs
- Custom LLM fine-tunes (if we train them later)

## Go-to-market strategy

### Phase 1: Launch (months 0-3)
- Apache 2.0 + public repo + website
- HN launch: "SynapCode: local-first GraphRAG for your codebase"
- Feature-complete free tier
- MCP server integration with Claude Code + Cursor + Codex
- Target: 10k GitHub stars, 5k active users

### Phase 2: Pro tier (months 3-6)
- Launch Pro ($19/mo) with agent features + cloud sync
- Content marketing: "How we refactored X using SynapCode" case studies
- Integrations: Linear, Slack, Jira, GitHub, Notion
- Target: 1k paying Pro users ($228k ARR)

### Phase 3: Team tier (months 6-12)
- Launch Team ($39/user/month)
- Self-serve team signups + admin dashboard
- Sales motion starts for teams 20+
- Target: 100 paying teams (~$500k ARR from teams)

### Phase 4: Enterprise + Vertical (months 12-24)
- Enterprise contracts with compliance, SSO, self-hosted
- Vertical solution launches (FinServ, Healthcare, Gov)
- Partnerships / OEM deals
- Target: 10-20 enterprise logos ($2-5M ARR from enterprise)

## Key risks and mitigations

| Risk | Mitigation |
|---|---|
| **FalkorDB deprecates or changes license** | Abstract the graph layer; we could swap in Memgraph or a custom engine if needed |
| **A fork appears (FOSS competitor)** | We out-ship on team features + enterprise + verticals where margins live — forks rarely replicate the full SaaS stack |
| **Cursor or GitHub builds this into their product** | Our MCP positioning turns them into customers; we're the context layer they query |
| **Enterprise sales cycle is too long / expensive** | Self-serve Pro + Team tiers fund operations while enterprise pipeline builds |
| **LLM agent features commoditize** | The differentiation is the structured graph, not the LLM — we're less sensitive to model changes than Cursor |
| **Capital required** | Much less than Cursor — we don't train models. A ~$5M seed should take us to $5-10M ARR |

## Why this is the right model for SynapCode specifically

| | Cursor (closed) | SynapCode (open core) |
|---|---|---|
| Proprietary model moat | ✅ custom models | ❌ uses frontier APIs |
| Distribution fork advantage | ✅ forked VS Code | ❌ new tool from scratch |
| Server-side dependency | ✅ everything phones home | ❌ local-first is the pitch |
| Trust from FOSS | ❌ closed binary | ✅ fully auditable |
| Community-built integrations | ❌ | ✅ MCP + language parsers |
| Compliance-ready for enterprise | 🟡 SaaS only | ✅ on-prem + air-gapped |
| Capital efficiency | ❌ hundreds of millions to compete | ✅ open-source velocity |

**SynapCode's natural shape is open core.** The local-first pitch requires openness. The revenue lives in team/enterprise/vertical layers that FalkorDB can't build. The moats are network effects (team graphs) + compliance + verticals + marketplace — none of which close-sourcing helps.

## One-sentence business plan

> **SynapCode is open-core code intelligence: Apache-licensed CLI and desktop app drive adoption, paid tiers monetize team collaboration, agent features, and enterprise compliance — targeting $3M ARR year 1, $30M year 2, $100M year 3.**

## Concrete next steps

1. **Add `LICENSE` (Apache 2.0)** to the repo
2. **Write `CONTRIBUTING.md`** and set up GitHub issue templates
3. **Split the repo**: move enterprise features to a separate private repo that depends on the open core
4. **Landing page + pricing page** — synapcode.dev (or similar, avoid trademark conflict)
5. **Build the Pro cloud backend** — minimal viable: auth + hosted graph sync + agent API
6. **Ship the HN launch** — Apache 2.0 + demo video + blog post about the architecture
7. **Instrument everything** — DAU, retention, graph size, query volume, conversion funnel
