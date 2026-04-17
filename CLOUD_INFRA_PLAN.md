# Savants Cloud Infrastructure Plan

## Phase 1: Beta (0-100 customers, months 1-3)

Goal: ship fast, keep costs near zero, get customers hooked on the graph.

### Stack
- **Host**: astra (homelab, Tailscale) - already running
- **API Gateway**: Axum (Rust) - single binary, same style as savants-cli
- **Graph DB**: FalkorDB on k3s (already deployed on astra)
- **Metadata DB**: Postgres on astra (users, orgs, api_keys, usage records)
- **Billing**: Stripe (metered billing API)
- **DNS + TLS**: Cloudflare (savants.cloud subdomain)
- **Object storage**: MinIO (already deployed) for binary releases and backups
- **Monitoring**: Savants daemon (eat our own dog food)

### Services to build
1. `savants-api` - Axum server, auth + proxy to graph + billing meter
2. `savants-ingester` - per-tenant background workers for Slack/Jira/Sentry/GitHub
3. `savants-slack-bot` - single Slack app serving all workspaces
4. `savants-ci-webhook` - GitHub Actions webhook receiver
5. `savants-dashboard` - Astro site with API key management + usage view

### Multi-tenancy model
- One shared FalkorDB instance
- Each customer gets a separate graph (namespaced by `repo` property already in schema)
- API gateway enforces `X-Savants-Tenant-ID` on every query
- Existing graph schema already has `repo` prefix, so minimal changes

### Auth
- API keys: `sk_live_<random>` format, hashed in Postgres
- Slack OAuth for the bot
- GitHub App for CI webhooks
- No SSO yet (Phase 3)

### Usage metering
- Every API call logs: `(tenant_id, endpoint, timestamp, success)`
- Nightly job aggregates usage per tenant
- Stripe metered billing gets the daily total
- Customer sees usage in dashboard: queries this month, projected bill

## Phase 2: Paid launch (months 4-6)

### Add
- Stripe checkout integration (self-serve upgrade)
- Rate limiting per tenant (prevent runaway bills)
- Usage alerts (email when hitting thresholds)
- Multiple Slack workspaces per account

### Migrate when needed
- Move from astra homelab to AWS/GCP if traffic demands
- Likely EC2 for the API server, RDS for Postgres, self-hosted FalkorDB
- Cost projection: ~$500/mo infra at 100 paying customers, ~$3K/mo at 1000

## Phase 3: Enterprise (months 7-12)

### Add
- SSO (Google, Okta, SAML)
- RBAC and audit logs
- On-prem deployment option (same binary, customer runs it)
- Dedicated graph instances for enterprise customers
- SOC2 Type 1 prep

## Concrete next steps (this week)

1. [ ] DNS: point savants.cloud at Cloudflare
2. [ ] Register Slack app for the @savants bot
3. [ ] Register GitHub app for CI webhook
4. [ ] Build `savants-api` Axum skeleton with health check
5. [ ] Postgres schema: users, orgs, api_keys, usage_events
6. [ ] API key generation + auth middleware
7. [ ] Proxy single endpoint (diagnose-error) through API to local FalkorDB
8. [ ] Deploy to astra via existing helm pattern
9. [ ] Put up savants.cloud landing page with "request early access" form

## Early access grandfather policy

Early adopters (first 100 sign-ups) get:
- Free while in beta
- 50% off PAYG pricing for 12 months after paid launch
- Priority support via direct Slack channel
- Named in launch blog post (optional)

This creates a group of vocal advocates for the HN launch.

## Cost model (internal)

At 100 customers, paid launch:
- Infra: ~$500/mo (server, DB, Cloudflare)
- FalkorDB RAM: ~$200/mo (scales with graph size)
- Stripe fees: 2.9% + $0.30 per transaction
- Gross margin: ~90% at $5K MRR, ~95%+ at $50K MRR

## What NOT to build yet
- Web UI for graph queries (CLI + MCP are enough)
- Mobile app (no)
- GraphQL API (REST is enough)
- Kubernetes operator (on-prem is Phase 3)
- Multi-region (single region until we hit geographic customer concentration)
