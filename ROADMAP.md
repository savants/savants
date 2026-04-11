# Savants Roadmap — From Today to $1B

## Where we are (April 2026)

**Built and working:**
- 5.4MB Rust binary, zero dependencies
- 20 host monitoring categories (3.7s full scan)
- K8s cluster state + real-time watch streams
- Log intelligence (template extraction, severity classification)
- eBPF security probes (process exec, network connections)
- 27 MCP tools for Claude Code / Cursor / any AI
- 31 knowledge patterns with v2 dynamic diagnosis engine
- SaQL query language (hides internal architecture)
- Cloud API server (Axum, Postgres, JWT auth, Stripe)
- Website (savants.dev, 23KB)
- Install script (savants.sh)
- Pricing model ($50/1K resources under management)

**Proven on live infrastructure:**
- Diagnosed coredns DNS cascade (15 CrashLoopBackOff pods → 1 root cause → 45s)
- Found DHCP DAD conflict causing intermittent 502s (invisible to K8s monitoring)
- Identified WiFi 2.4GHz interference (16K dropped packets)
- All fixes verified working

---

## Phase 1: Launch (Weeks 1-2)

**Goal: First 100 users from Hacker News.**

- [ ] Deploy savants.dev to Cloudflare Pages
- [ ] Host binary at savants.sh (install script serves correct arch)
- [ ] Create GitHub releases with pre-built binaries (linux-x86_64, linux-aarch64)
- [ ] Post on Hacker News (draft ready in docs/hn-post.md)
- [ ] Post on r/kubernetes, r/devops, r/sre
- [ ] Record 2-minute terminal demo video (savants up → diagnosis)
- [ ] Submit to MCP server directories (Claude Code, Cursor)

**Success metric:** 500 installs, 10 GitHub stars, 3 people reach out.

---

## Phase 2: Product-Market Fit (Weeks 3-8)

**Goal: Find 10 users who can't live without it.**

- [ ] Talk to every person who installed it. What did they try? Where did it break?
- [ ] Fix the top 3 pain points from user feedback
- [ ] Add GitHub Actions integration (PR comment: blast radius + risk score)
- [ ] Add Slack bot (the primary enterprise interface)
- [ ] Wire v2 dynamic diagnosis into the diagnose MCP tool
- [ ] Add learning mode for eBPF (replace whitelist with baseline)
- [ ] Implement adapter system (TOML-based extensible resource ingestion)
- [ ] Add `savants report --format html` for shareable incident reports

**Success metric:** 10 weekly active users who use it in real incidents.

---

## Phase 3: Cloud Tier + Revenue (Weeks 9-16)

**Goal: First paying customer. $1K MRR.**

- [ ] Deploy savants.cloud to GCP Cloud Run + Cloud SQL
- [ ] Complete OAuth device flow (savants connect → browser auth)
- [ ] Firebase Auth for Google/GitHub SSO
- [ ] Agent keys for headless remote clusters
- [ ] Federation: CLI pushes deltas to cloud, cloud stores federated graph
- [ ] Cross-cluster queries ("show all failing pods across all clusters")
- [ ] Billing: Stripe metering on resources under management
- [ ] Team features: shared graphs, member invites, org management
- [ ] Dashboard: web UI for viewing graphs and incidents

**Success metric:** 3 paying teams, $3K MRR, 1K total installs.

---

## Phase 4: Integrations + Growth (Months 5-8)

**Goal: Become the default infrastructure intelligence tool. $50K MRR.**

- [ ] GitHub bot: auto-comment on PRs with blast radius analysis
- [ ] PagerDuty integration: auto-enrich incidents with graph context
- [ ] Datadog/Grafana: import metrics, connect to code graph
- [ ] ArgoCD/Flux: track deployments, know what version is running where
- [ ] Terraform/Pulumi: blast radius for infrastructure changes
- [ ] AWS integration: EC2, RDS, Lambda, ECS, costs → graph
- [ ] GCP integration: GKE, Cloud Run, Cloud SQL → graph
- [ ] Extension SDK: let third parties build MCP tools on the graph
- [ ] Compliance reports: auto-generate SOC2/HIPAA incident evidence

**Success metric:** 50 paying teams, $50K MRR, 10K installs, 5 community extensions.

---

## Phase 5: Enterprise + Platform (Months 9-14)

**Goal: Enterprise contracts. $500K MRR.**

- [ ] SSO (SAML/OIDC) for enterprise customers
- [ ] RBAC: role-based access to graphs and tools
- [ ] Audit logs: who queried what, when
- [ ] On-premises deployment option (Helm chart, air-gapped)
- [ ] AWS Marketplace listing (buy with committed spend)
- [ ] Dedicated support tier ($5K/month)
- [ ] SLA: 15-minute response for critical issues
- [ ] SOC2 Type 2 certification for savants.cloud
- [ ] Multi-region deployment (US, EU, APAC)

**Success metric:** 10 enterprise customers, $500K MRR, 100K installs.

---

## Phase 6: Autonomous Agent (Months 15-24)

**Goal: Savants diagnoses AND fixes. $2M MRR.**

- [ ] Google Docs/Notion integration: connect docs to code + infra
- [ ] Google Calendar: on-call schedules, deploy freezes
- [ ] Slack deep integration: parse incident channels for decisions
- [ ] Jira/Linear: connect tickets to code changes to incidents
- [ ] Identity integration (Okta/Google Workspace): who has access to what
- [ ] Autonomous fix suggestion: detect issue → generate PR → request approval
- [ ] Autonomous deploy: if approved, trigger ArgoCD/Flux deployment
- [ ] Predictive: "this pod will OOM in 12 hours based on memory trend"
- [ ] Cost optimization: "these 5 deployments are idle, saving $2K/month"
- [ ] Full incident timeline: code change → deploy → incident → fix → postmortem

**Success metric:** 100 enterprise customers, $2M MRR, 500K installs.

---

## Phase 7: The Standard (Year 3+)

**Goal: Savants is how infrastructure is managed. $10M+ MRR.**

- [ ] Figma/Canva: design system connected to component graph
- [ ] Salesforce: customer impact analysis during incidents
- [ ] Financial: connect cloud spend to business metrics
- [ ] AI agent marketplace: community-built agents on the Savants platform
- [ ] Multi-cloud federation: unified view across AWS + GCP + Azure + on-prem
- [ ] Acquisition targets: buy specialized tools, integrate into the graph
- [ ] IPO preparation or strategic acquisition

---

## What you need to do RIGHT NOW

1. **Deploy savants.dev** (Cloudflare Pages, 30 minutes)
2. **Host binary at savants.sh** (Cloudflare Worker, 1 hour)
3. **Post on HN** (Tuesday or Wednesday, 9-10am ET)

Everything else follows from having users. The product is built. Ship it.

---

## The one question that determines everything

**Can you get 10 people to use Savants in a real incident within 60 days?**

If yes → the roadmap accelerates. Users tell you what to build next.
If no → the product needs to change. More features won't help.

The fastest path to 10 users: find SRE teams who had a bad incident
in the last month, run Savants against their cluster live, and show
them the root cause they already know (so they can verify accuracy).

---

*Generated by Savants. Last updated: 2026-04-11.*
