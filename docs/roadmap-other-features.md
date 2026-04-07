# SynapCode Roadmap — Other Useful Features

**Status:** Living document, last updated 2026-04-07
**Purpose:** Capture features and integrations that we know are valuable but haven't built yet, so we don't lose ideas as we ship the core product.

This document is intentionally exhaustive — it's a backlog, not a launch plan. Each item is rated by:
- **Build effort** (S = days, M = weeks, L = months, XL = quarters)
- **Buyer** (Individual / Team / Enterprise / Other tool company)
- **Pain it solves**
- **What we already have that supports it**

---

## 1. Daily-use developer surfaces

These are the in-the-flow features that turn SynapCode from "tool you query occasionally" into "thing your editor calls 100 times a day."

### 1.1 VS Code / JetBrains / Zed extensions
**Effort:** M-L per editor
**Buyer:** Individual / Team
**Pain:** Currently devs use SynapCode via CLI or MCP. The biggest unlock is putting it inline in the editor where they already are.
**Have:** MCP server that any editor can call. Function X-Ray, Find References, Risk Score tools all exist as MCP tools.
**Build:** A thin extension per editor that calls the MCP server, renders the structured output, and binds to keyboard shortcuts (`Cmd+Shift+X` for X-Ray, hover for context, etc.)
**Notes:** Start with VS Code (largest dev population). Zed second (Rust-friendly, growing fast). JetBrains last (Java plugin pain).

### 1.2 Inline hover popup with full context
**Effort:** S (one editor) → M (all editors)
**Buyer:** Individual
**Pain:** "Go to definition" jumps you to a file. You then have to read it to understand. SynapCode's hover would show: full structural context, recent committers, blast radius, last incident — without moving from the current line.
**Have:** All the data; just need editor integration.
**Build:** Wraps the `function_xray` MCP tool in a hover provider.

### 1.3 Pre-save warnings (in-editor linting from the graph)
**Effort:** M
**Buyer:** Individual / Team
**Pain:** You're about to delete a function. Static analysis only catches "this symbol is undefined." SynapCode catches "this symbol is *about to become* undefined and these 12 callers will break."
**Have:** Impact analysis, find references.
**Build:** A diagnostic provider that watches edits in real time and surfaces warnings via the editor's standard linting UI.

### 1.4 Architecture coupling guardrails
**Effort:** S
**Buyer:** Team
**Pain:** Architectural boundaries dissolve over time because nobody catches the moment a new cross-module dependency is added. SynapCode catches it the moment it appears.
**Have:** `coupling_check` MCP tool already exists.
**Build:** Hook the tool into pre-commit and the editor's diagnostic stream. Optional: a `.synapcode/architecture.toml` file that declares forbidden module pairs.

### 1.5 Smart Find-References replacement
**Effort:** S (already an MCP tool)
**Buyer:** Individual
**Pain:** Every IDE's Find References is text-based. Returns garbage matches.
**Have:** `find_references_structured` MCP tool ✅
**Build:** Editor binding that overrides the default Find References shortcut.

---

## 2. Code review and PR workflow

### 2.1 GitHub App: PR auto-brief
**Effort:** M
**Buyer:** Team
**Pain:** Reviewers spend 5-30 min figuring out what to focus on in a 200-line PR.
**Have:** All the underlying queries (impact, co-change, risk_score, find_references).
**Build:** A GitHub App that listens for `pull_request.opened` webhooks, runs the queries against the head SHA's graph, and posts a structured comment with: scope summary, risk score, suggested reviewers, focus lines, similar past PRs.
**Pricing leverage:** This is a single feature that demonstrates *all* the value. Free for OSS repos as marketing. Paid for private repos at $40/dev/month.

### 2.2 GitLab / Bitbucket equivalents
**Effort:** M each (after GitHub)
**Buyer:** Team
**Pain:** Same as GitHub.
**Have:** GitHub App as a reference; the queries are git-host-agnostic.
**Build:** Port the GitHub App webhook handler to each host's API.

### 2.3 PR-aware code review focus
**Effort:** M
**Buyer:** Team
**Pain:** Reviewer wants to know "of these 200 changed lines, which 5 actually matter?"
**Have:** Centrality + author novelty + historical incident correlation queries.
**Build:** A scoring function that ranks each changed line by review priority. Outputs to the PR review UI as inline comments.

### 2.4 Smart test selection
**Effort:** S
**Buyer:** Team / Enterprise
**Pain:** CI runs 4,500 tests when only 18 actually exercise the affected call graph. Result: 12 minutes of CI per push.
**Have:** Call graph traversal queries.
**Build:** `synapcode test --since=HEAD~1` outputs the minimal test set for a commit. Drop-in for any pytest/jest/go test runner.
**Notes:** Pairs naturally with the GitHub App so test selection happens automatically on each PR push.

---

## 3. AI agent grounding

### 3.1 MCP grounding tools (DONE — 6 tools shipped)
**Status:** ✅ Implemented in commit referenced by task #12
**Tools:** `find_references_structured`, `function_xray`, `co_change_partners`, `coupling_check`, `pre_change_warning`, `risk_score`
**Buyer:** Other AI tool companies (Cursor, Continue, Aider, Cline, Cody) — they pay per query
**Pain:** AI coding agents currently guess based on context window text. They hallucinate calls to functions that don't exist, miss call sites in refactors, and propose changes that break things.
**Notes:** The grounding tools exist but the *integration story* with each AI tool is the next step. See 3.2.

### 3.2 First-class integrations with major AI coding tools
**Effort:** S per integration (the MCP server already exists; we just need each tool to discover it)
**Buyer:** Other AI tool companies
**Strategy:** Reach out to Cursor, Continue, Cody, Aider, Cline, Zed, etc. and offer "drop in our MCP server, your agents stop hallucinating about codebases overnight." Most accept because it makes their product better at no cost to them.
**Pricing:** Free for individual users (they self-host SynapCode); paid metered API for AI tool companies who proxy queries through our cloud.

### 3.3 Anthropic Tool Use / OpenAI function calling adapters
**Effort:** S
**Buyer:** Other AI tool companies, internal agent builders
**Pain:** Not everyone speaks MCP. Some agents use Anthropic's tool format, some use OpenAI's, some use LangChain.
**Build:** Thin adapters that expose the same query set under different schemas. Same backend, different shape.

### 3.4 The "agent reliability moat" positioning
**Effort:** S (mostly marketing)
**Buyer:** All AI tool buyers
**Pain:** "I tried Cursor and it broke my code." "Claude made up function names." This is the #1 complaint about AI coding tools.
**Build:** A blog post + benchmark suite measuring "agent reliability with vs. without SynapCode grounding" on real codebases. Demonstrate the 80%+ reduction in hallucinated calls. Publish with a link to add SynapCode to any MCP-compatible client.

---

## 4. Engineering leadership and strategy

### 4.1 Engineering Health Dashboard
**Effort:** M
**Buyer:** Team / Enterprise (sold to VP Eng)
**Pain:** Engineering leaders have no structural visibility into bus factor, drift, knowledge silos, hot files. They run on vibes + retroactive incident postmortems.
**Have:** All the queries are already in `src/synapcode/analysis/queries.py`.
**Build:** A weekly cron that runs the queries and emails an HTML report. Phase 2: a hosted dashboard.
**Pricing:** $100/dev/month for orgs of 50-500 devs.

### 4.2 Bus Factor Heat Map
**Effort:** S
**Buyer:** Team / Enterprise
**Pain:** "Who else can safely modify this if Alice leaves?" — currently answered by intuition.
**Have:** `bus_factor` query, `top_contributors` query.
**Build:** A heat-map visualization (file tree → red where bus factor is 1, green where it's 4+). Standalone webpage from `synapcode bus-factor --html`.

### 4.3 Architecture Drift Detection
**Effort:** M
**Buyer:** Enterprise
**Pain:** Modular architectures dissolve over years. Nobody notices it happen.
**Have:** Layered graph (current state + history); coupling detection.
**Build:** A scheduled job that snapshots the call graph monthly, computes module-to-module edge counts, and alerts when boundaries dissolve. Optionally trains on a "good architecture" snapshot from 6 months ago.

### 4.4 Knowledge Transfer Audit
**Effort:** S
**Buyer:** Team / HR (one-time per departure)
**Pain:** Engineer gives notice. Manager has 2 weeks. What knowledge dies with them?
**Have:** Bus factor + recency-weighted contribution.
**Build:** `synapcode audit-departure <author>` outputs:
- Files where they're the dominant maintainer
- Functions only they understand (sole substantive contributor)
- Co-change partners showing what's coupled to their work
- Suggested pairing sessions before they leave
**Pricing:** $5k flat fee per departure audit, or bundled with the leadership tier.

### 4.5 Codebase Health Score (longitudinal)
**Effort:** M
**Buyer:** Enterprise / Board reports
**Pain:** "Is our codebase getting better or worse?" — currently no answer beyond LOC growth.
**Build:** Track over time: bus factor distribution, average call depth, test ratio, hot file count, drift score. Output a single 0-100 health score with sub-scores. Show trend lines over quarters.

---

## 5. Compliance and audit

### 5.1 SOC 2 evidence generator
**Effort:** L (lots of detail; needs auditor input)
**Buyer:** Enterprise / Compliance officer
**Pain:** Auditors require evidence of code review, separation of duties, access controls. Generated manually today, takes weeks.
**Have:** Episode log + author tracking + call graph.
**Build:** A "compliance pack" that for any time window outputs: every commit + author + reviewer, every change touching a tagged sensitive path (PII, payments, auth), every deployment with the commits it included, signed cryptographically. Becomes the auditor evidence package.
**Pricing:** $100k+ contracts. Compliance officers approve immediately.

### 5.2 HIPAA / PHI traceability
**Effort:** M
**Buyer:** Healthcare enterprise
**Pain:** "Prove no commit allows PHI to leave the encrypted boundary."
**Build:** Tag specific function names as PHI-sensitive (e.g., `read_patient_record`). Run a query that finds every function that calls them, transitively, and verify they're all inside the encrypted module. Alert on any new edge that crosses the boundary.

### 5.3 PCI-DSS scope reduction
**Effort:** M
**Buyer:** Financial services enterprise
**Pain:** Currently every system that *might* touch cardholder data is in PCI scope. SynapCode can prove which functions actually do.
**Build:** Tag known PCI surfaces (e.g., `tokenize_card`, `charge`). Compute the transitive closure. Output an "in scope" file list and an "out of scope" file list. Reduces audit cost dramatically.

### 5.4 ITAR / FedRAMP traceability
**Effort:** L (mostly compliance certification, not engineering)
**Buyer:** Defense / Gov enterprise
**Pain:** Same idea as HIPAA but for export-controlled / government data.
**Build:** Same engine, different tags. The hard part is the certification, not the code.

---

## 6. SRE and incident response

### 6.1 Post-incident root cause assistant
**Effort:** S
**Buyer:** Team / Enterprise
**Pain:** Incident fires. Currently it's 30 min of `git log` + Slack scrambling.
**Have:** `pre_change_warning`, `risk_score`, history queries.
**Build:** `synapcode explain --since=3h --service=payments` outputs the recent risky commits ranked by their bug correlation, with revert hints. Pairs with the leadership dashboard.

### 6.2 Real-time deployment risk scoring
**Effort:** M
**Buyer:** Team / Enterprise
**Pain:** "Should I deploy this commit on a Friday afternoon?"
**Have:** `risk_score` MCP tool already exists.
**Build:** A pre-deploy hook that runs against the commit's affected functions and outputs a 0-10 risk score. Optionally blocks deploys above a threshold (configurable per environment).

### 6.3 PagerDuty / Opsgenie integration
**Effort:** S
**Buyer:** Team
**Pain:** Page wakes you up, you have no context.
**Build:** When a page fires for service X, automatically attach to the incident: the last 5 commits touching that service, the maintainers, the risk scores. Comment on the incident channel with structured context.

### 6.4 Automated runbook generation
**Effort:** M
**Buyer:** Team / Enterprise
**Pain:** Runbooks go stale within 6 months.
**Build:** Generate a runbook from the graph: for each service entrypoint, the call chain, the data dependencies, the config it reads, the environment variables it uses, the maintainers. Re-generated nightly so it's never stale.

---

## 7. Cross-repo and ecosystem features

### 7.1 Multi-repo / monorepo aggregation
**Effort:** M
**Buyer:** Team / Enterprise
**Pain:** Most companies have 5-50 repos. Each has its own SynapCode graph. You can't query across them.
**Have:** Per-repo graphs work today.
**Build:** A "workspace" concept that joins multiple graphs at query time. Cross-repo impact analysis: "if I change this gRPC interface in repo A, what breaks in repos B, C, D?"

### 7.2 OSS dependency intelligence
**Effort:** M
**Buyer:** Team / Enterprise
**Pain:** "We depend on 200 OSS libraries. Which ones are bus-factored to one person, abandoned, or about to break?"
**Build:** SynapCode-as-a-service that crawls a customer's `package.json` / `requirements.txt` / `Cargo.toml`, runs SynapCode against each dependency's source repo, and generates a risk report. **Pairs with vendor lock-in detection.**
**Notes:** Like Tidelift but with structural intelligence, not just license checking.

### 7.3 Vendor contribution forensics
**Effort:** S
**Buyer:** M&A / Enterprise procurement
**Pain:** Considering acquiring or partnering with a company. How much of their open-source code is genuinely theirs vs. forked / contributed by community?
**Build:** Run SynapCode on the target's repos. Compute the recency-weighted contribution map. Surface real authorship.

### 7.4 Acquihire intelligence
**Effort:** S
**Buyer:** VC firms / acquihire teams
**Pain:** Want to acquihire team X. Are the right people on it?
**Build:** Run SynapCode against the company's public repos. Identify the contributors who are *load-bearing* (high recency-weighted contribution to architecturally critical files) versus who's just along for the ride. Output a list ranked by technical leverage.

---

## 8. Infrastructure and cloud

### 8.1 Webhook receiver service
**Effort:** M
**Buyer:** Team (cloud tier)
**Pain:** Local SynapCode requires manual indexing. Cloud users want automatic re-indexing on every push.
**Build:** A small Rust/Python service that receives GitHub/GitLab webhook events, runs the incremental indexer, updates the cloud-hosted graph. Single process, stateless aside from the FalkorDB connection.

### 8.2 Multi-tenant cloud (the team tier backend)
**Effort:** L
**Buyer:** Team / Enterprise
**Pain:** Currently SynapCode is single-tenant local. Team features require a shared graph somewhere.
**Build:** A FalkorDB cluster on Kubernetes (KubeBlocks operator), per-tenant graph isolation, auth via Keygen.sh, simple HTTP API in front of MCP for non-MCP clients. Confidential compute via Nitro Enclaves for the privacy story.
**Notes:** The substrate exists; this is operational engineering. ~3-4 months for production-ready.

### 8.3 Confidential compute via Nitro Enclaves
**Effort:** L
**Buyer:** Enterprise (privacy-conscious)
**Pain:** "We can't put our code on a third-party cloud." TEE-based architecture means we cryptographically cannot read it.
**Build:** Run the FalkorDB sidecar inside a Nitro Enclave. Publish enclave code hash. Provide attestation SDK so customers can verify before sending data.
**Notes:** This is the differentiator for finance/healthcare/gov. See `docs/architecture-layered-graphs.md` for the architecture.

### 8.4 Self-hosted Helm chart
**Effort:** M
**Buyer:** Enterprise
**Pain:** Some enterprises will only run software inside their own VPC.
**Build:** A Helm chart deploying FalkorDB + Temporal + Python worker + MCP server + webhook receiver. Terraform modules for AWS/GCP/Azure.

### 8.5 Air-gapped installer
**Effort:** M
**Buyer:** Defense / Gov enterprise
**Pain:** Some networks have no internet access at all.
**Build:** A self-contained installer bundle (Helm chart + container images + binaries + docs) that can be sneakernet-ed onto an air-gapped network and brought up.

---

## 9. Integrations with the dev tool ecosystem

### 9.1 Linear / Jira integration
**Effort:** S
**Buyer:** Team
**Pain:** Bug reports in Linear/Jira don't link to the actual functions they're about.
**Build:** When a Linear ticket mentions a function or file, auto-comment with the SynapCode X-Ray. When a commit references a Linear ticket, update the ticket with structural context.

### 9.2 Slack integration
**Effort:** S
**Buyer:** Team
**Pain:** Devs ask "who knows this code?" in Slack constantly.
**Build:** A Slack bot. `/synapcode owner src/auth/jwt.py` returns the bus factor + contributors. `/synapcode impact authenticate` returns the impact analysis.

### 9.3 Notion / Confluence integration
**Effort:** S
**Buyer:** Team
**Pain:** Architecture docs go stale within months because they're hand-written.
**Build:** A scheduled job that publishes architecture summaries (hubs, modules, cross-module edges, drift) to Notion/Confluence. Always fresh.

### 9.4 Sentry / Datadog / New Relic integration
**Effort:** M
**Buyer:** Team / Enterprise
**Pain:** Sentry shows a stack trace. You don't know which function in the stack is the actually-risky one.
**Build:** Sentry plugin that for each stack frame queries SynapCode for risk score, recent committers, and blast radius. Annotates the error with structural context.
**Notes:** This is a *huge* opportunity — the inverse of grounding. We're not just enabling AI agents; we're enabling observability tools too.

### 9.5 GitHub Code Search integration
**Effort:** M
**Buyer:** Other tool companies
**Pain:** GitHub's code search is text-based. SynapCode is structural.
**Build:** A GitHub Action / CLI that augments GitHub search results with SynapCode findings.

### 9.6 OpenTelemetry / distributed tracing integration
**Effort:** L
**Buyer:** Enterprise
**Pain:** Distributed traces show what *actually* happened. Static call graphs show what *can* happen. Together they're more powerful than either alone.
**Build:** Cross-reference OTel traces against the static call graph. Find: dead branches (declared but never traced), surprise paths (traced but not in the graph), perf anomalies (slow paths through specific functions).

---

## 10. Pricing and billing infrastructure

### 10.1 Stripe + Keygen.sh integration
**Effort:** S
**Buyer:** Internal (us)
**Pain:** No way to take money yet.
**Build:** Stripe checkout for paid tiers. Keygen.sh for license validation. Hardware-bound license keys. Trial expiration.

### 10.2 Per-query API billing
**Effort:** M
**Buyer:** Internal (us)
**Pain:** AI tool integrations need usage-based billing.
**Build:** Metering layer in front of the cloud MCP server. Stripe usage records. Monthly invoices.

### 10.3 Usage analytics
**Effort:** S
**Buyer:** Internal (us)
**Pain:** Need to know what features users actually use to prioritize roadmap.
**Build:** Self-hosted Plausible instance + privacy-preserving event tracking. No PII, no code content — just feature usage counts.

---

## 11. Observability for SynapCode itself

### 11.1 Internal metrics
**Effort:** S
**Buyer:** Internal (us)
**Build:** Prometheus exporter in the MCP server + workers. Metrics: query latency p50/p99, memory usage, error rates, FalkorDB query counts.

### 11.2 Slow query detection
**Effort:** S
**Buyer:** Internal (us)
**Build:** Log every query that takes > 1 second with the query text. Used to find performance regressions in the graph.

### 11.3 Tracing
**Effort:** S
**Buyer:** Internal (us)
**Build:** OTel traces for the MCP server. Useful for debugging customer issues without seeing their code.

---

## 12. The Rust path

### 12.1 Maturin build pipeline (currently scaffolded)
**Status:** Scaffold exists in `rust-core/`. Builds clean. 10/10 tests pass.
**Next:** Wire the PyO3 module into `cpg.py` so existing Python code can opt in.

### 12.2 Native CLI binary
**Effort:** M
**Buyer:** Individual / Team
**Pain:** `python -m synapcode.cli` cold-starts in ~1100ms because of Python interpreter overhead. Real graph queries are sub-100ms. The CLI feels slow.
**Have:** Rust crate compiles cleanly.
**Build:** Wrap the Rust crate in a `clap`-based CLI that talks to the FalkorDB sidecar directly. Replaces the Python CLI for users who don't need the agent layer. Cold start: ~50ms.

### 12.3 Embedded mode (no FalkorDB sidecar)
**Effort:** L
**Buyer:** Individual / privacy-conscious
**Pain:** Some users don't want to run a sidecar.
**Build:** Use FalkorDB as an embedded library (it's a Redis module — embeddable in C). Single static binary, no sidecar process, no port. Trades off sharing for ultimate simplicity.

---

## 13. Documentation and education

### 13.1 Architecture deep-dive blog series
**Effort:** S each
**Buyer:** Marketing
**Build:** A series of posts: "How we built layered graphs", "Why we chose FalkorDB over Neo4j", "What we found in fastapi", "Function X-Ray internals", etc. Each post is a standalone marketing artifact.

### 13.2 Public OSS analysis reports
**Effort:** S each (queries are reusable)
**Buyer:** Marketing
**Build:** "What SynapCode found in [Django / React / Kubernetes / TypeScript]." We did one for fastapi already (`docs/fastapi-analysis.md`). Each post is HN-front-page material.

### 13.3 Customer case studies
**Effort:** S each (after first customer)
**Buyer:** Sales
**Build:** "How [Customer X] reduced incident MTTR by 80% using SynapCode" — once we have customers.

### 13.4 Free assessment tool
**Effort:** S
**Buyer:** Marketing / Lead generation
**Build:** A landing page that runs SynapCode against any public GitHub repo and emails the user a free 5-page report. Lead capture for the paid tier.

---

## Priority order (recommended sequencing)

If we had to ship in order:

1. **Build #2.1 (GitHub App PR auto-brief)** — single feature that demonstrates everything, viral, monetizable immediately
2. **Build #1.1 (VS Code extension)** — daily-use surface that drives individual subscriptions
3. **Build #4.1 (Engineering Health Dashboard)** — sells to engineering leaders (highest ARPU)
4. **Build #2.4 (Smart test selection)** — universal value, easy to drop in
5. **Build #6.1 (Post-incident root cause assistant)** — SRE killer demo
6. **Build #7.1 (Multi-repo workspace)** — unlocks the team and enterprise tiers
7. **Build #8.1 + 8.2 (Webhook receiver + multi-tenant cloud)** — required for any team-tier launch
8. **Build #5.1 (SOC 2 evidence generator)** — opens enterprise door
9. **Build #8.3 (Confidential compute)** — opens regulated enterprise door
10. **Build #12.2 (Native Rust CLI)** — turns the dev experience snappy
11. **Build #13.4 (Free assessment landing page)** — lead-gen flywheel
12. **Build #4.2 + #4.5 (Bus factor heat map + health score)** — leadership dashboard polish

Everything else is a function of which customer segment we hit first and what they ask for next.

## What this document is not

This is **not** a launch plan. It's a backlog. We do not need to ship all of this. In fact, **trying to ship more than 5 of these in the first year would be a strategic error.** Pick the 3-5 that best serve the chosen customer segment and ship them well.

The point of this document is: **when we're staring at the roadmap and someone asks "what could we build next?", we have a list to pick from instead of inventing on the spot.**
