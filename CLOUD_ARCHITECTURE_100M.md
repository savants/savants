# Savants Cloud Architecture - 100M Users

## Design Principles
1. Stateless API tier - scale horizontally by adding pods
2. Per-tenant graph isolation - no noisy neighbor, no data leaks
3. Event-driven ingestion - decouple writes from reads
4. Edge-first - Cloudflare handles TLS, DDoS, rate limiting, caching
5. No single points of failure at any tier

## Architecture

```
                    Cloudflare Edge (TLS, WAF, rate limiting, caching)
                              |
                    Cloudflare Workers (optional: auth validation at edge)
                              |
                   ┌──────────┴──────────┐
                   │   API Gateway Tier    │
                   │  (Axum, stateless)    │
                   │  N pods, HPA scaled   │
                   └──────┬───────────────┘
                          |
            ┌─────────────┼─────────────┐
            |             |             |
     ┌──────┴──────┐ ┌───┴───┐  ┌──────┴──────┐
     │  Auth/IAM   │ │ Queue │  │   Billing   │
     │  (Postgres) │ │(NATS) │  │  (Stripe)   │
     └─────────────┘ └───┬───┘  └─────────────┘
                         |
              ┌──────────┼──────────┐
              |          |          |
       ┌──────┴───┐ ┌───┴────┐ ┌──┴──────┐
       │ Ingester  │ │Indexer │ │ Query   │
       │ Workers   │ │Workers │ │ Workers │
       │(Slack,Jira│ │(tree-  │ │(graph   │
       │ Sentry,GH)│ │sitter) │ │ reads)  │
       └──────┬────┘ └───┬────┘ └──┬──────┘
              |          |         |
              └──────────┼─────────┘
                         |
              ┌──────────┴──────────┐
              │   Graph Storage Tier │
              │                      │
              │  Option A: FalkorDB  │
              │  per-tenant instance │
              │                      │
              │  Option B: Apache AGE│
              │  (Postgres extension)│
              │  per-tenant schema   │
              │                      │
              │  Option C: Managed   │
              │  graph service       │
              └──────────────────────┘
```

## Tier-by-Tier Design

### Tier 1: Edge (Cloudflare)
- **TLS termination** - zero config, automatic certs
- **DDoS protection** - built-in, no extra cost
- **Rate limiting** - per-IP and per-API-key rules
- **Caching** - tool list, architecture_overview responses cached at edge (TTL 5min)
- **WAF** - block SQL injection in graph query params
- **Workers (optional)** - validate API keys at edge before hitting origin, reject bad requests in <1ms

At 100M users: Cloudflare handles this. Their edge network serves 50M+ requests/second globally. No infrastructure to manage.

### Tier 2: API Gateway (Axum pods)
- **Stateless** - no session state, no local storage
- **Horizontal Pod Autoscaler** - scale 2-200 pods based on CPU/requests
- **Auth** - validate API key (hash lookup in Postgres), attach tenant_id to request
- **Routing** - dispatch to the right graph for this tenant
- **Usage metering** - async log to NATS queue (not synchronous DB write)

At 100M users: 100M users don't mean 100M concurrent. Assume 5% daily active = 5M DAU. Each does ~50 tool calls/day = 250M calls/day = ~3,000 requests/second average, ~15,000 peak. A single Axum pod handles 50K req/s. So 1-5 pods at peak. HPA handles burst to 20 pods.

### Tier 3: Message Queue (NATS)
- **Decouple writes from reads** - ingestion events, usage events, reindex triggers go through NATS
- **Backpressure** - if graph storage is slow, queue absorbs the burst
- **Replay** - reprocess events if a worker crashes
- **Topics**: `ingest.slack.{org_id}`, `ingest.jira.{org_id}`, `usage.{org_id}`, `reindex.{org_id}`

At 100M users: NATS handles 10M+ messages/second on a single node. Clustered NATS handles more.

### Tier 4: Workers

**Ingester Workers** (Slack, Jira, Sentry, GitHub)
- Pull from external APIs on schedule (per-org cron)
- Write deltas to NATS
- Stateless - one worker per org, scaled by queue depth
- At 100M users: most orgs ingest hourly. 100K active orgs x 4 sources x 1/hour = 400K ingestion jobs/hour = ~110/second. Trivial.

**Indexer Workers** (tree-sitter code indexing)
- Triggered by webhook or reindex-diff API call
- Parse changed files, compute function signatures, imports, call sites
- Write graph deltas to NATS
- At 100M users: reindex on push. 100K active repos x 10 pushes/day = 1M reindexes/day = ~12/second. Each takes 1-5 seconds. 20 workers handle this.

**Query Workers** (graph reads - the paid intelligence tools)
- diagnose-error, pr-risk, radar run here
- Read from tenant's graph, compute the answer
- CPU-intensive (graph traversal) but short-lived (< 5 seconds)
- At 100M users: 3,000 intelligence queries/second peak. Each takes 1-3 seconds. Need ~5,000-9,000 concurrent worker slots. With 50 pods x 200 concurrent requests each = 10,000 slots. Scales with HPA.

### Tier 5: Graph Storage

This is the critical tier. Options:

**Option A: FalkorDB per-tenant instance**
- Each tenant gets their own FalkorDB instance
- Perfect isolation, no noisy neighbor
- Simple to reason about
- Problem: 100K tenants = 100K Redis processes. At 50MB RAM average = 5TB RAM. Expensive but doable on large instances.
- Cost: ~$15K/month for 5TB RAM on AWS (r6g.16xlarge instances)

**Option B: Apache AGE (Postgres extension) - RECOMMENDED**
- Graph queries as a Postgres extension
- Per-tenant schema within shared Postgres clusters
- Postgres handles replication, backups, connection pooling
- Can use managed Postgres (RDS, Cloud SQL, Neon)
- Each cluster handles ~1,000 tenants
- 100 clusters for 100K active tenants
- Cost: ~$10K/month for 100 RDS instances (db.r6g.large)

**Option C: Managed graph service**
- AWS Neptune, Google Cloud Spanner, or similar
- Fully managed, auto-scaling
- Most expensive, least control
- Cost: ~$30K+/month

**Recommendation: Start with FalkorDB (already works), migrate to Apache AGE at 1,000 tenants.** AGE gives you Postgres ecosystem (backups, replication, monitoring, managed services) while keeping Cypher query compatibility.

### Tier 6: Metadata (Postgres)
- Users, orgs, API keys, usage events, billing records
- Single Postgres cluster with read replicas
- At 100M users: 100M rows in users table, 1B+ rows in usage_events (partitioned by month)
- Managed Postgres (RDS/Cloud SQL) handles this trivially

### Tier 7: Auth/IAM
- API key validation: hash lookup in Postgres, cached in Redis/Valkey for 5 minutes
- Device auth (RFC 8628): for CLI login flow
- OAuth (Google, GitHub): for dashboard login
- At 100M users: API key cache hit rate >99%. Redis handles 1M+ lookups/second.

## Tenant Isolation Model

```
org_123 (Acme Corp, 60 engineers)
  ├── graph: acme_corp_prod (FalkorDB/AGE instance)
  │   ├── CodeFunction nodes (4,604)
  │   ├── SlackMessage nodes (7,873)
  │   ├── JiraTicket nodes (231)
  │   ├── Commit nodes (632)
  │   └── ... (total ~40K nodes)
  ├── api_keys: [sk_live_abc123, sk_live_def456]
  ├── agent_keys: [svt_agent_xyz789]
  ├── integrations:
  │   ├── slack: workspace T0ABC (bot token)
  │   ├── jira: site acme.atlassian.net
  │   ├── sentry: org acme-corp
  │   └── github: org acme-corp (app installation)
  └── billing: stripe_customer cus_XXX
```

Every API call is scoped to the org_id derived from the API key. The graph query includes `{repo: 'org_123_*'}` prefix or routes to the tenant's dedicated graph instance. No tenant can see another tenant's data.

## Scaling Milestones

### Phase 1: 0-1,000 tenants (current)
- Single astra machine
- Shared FalkorDB, tenant isolation via repo prefix
- Single Postgres for metadata
- Cloudflare Tunnel for ingress
- Cost: ~$0/month (homelab)

### Phase 2: 1,000-10,000 tenants
- Move to AWS/GCP
- 3 API pods behind ALB
- 5 FalkorDB instances (200 tenants each)
- Managed Postgres (RDS)
- NATS for async ingestion
- Cost: ~$2K/month

### Phase 3: 10,000-100,000 tenants
- Migrate to Apache AGE (Postgres-based graph)
- 20 Postgres clusters (5,000 tenants each)
- 10 API pods with HPA
- 20 worker pods for indexing/ingestion
- Cloudflare Workers for edge auth
- Cost: ~$15K/month

### Phase 4: 100,000-1,000,000 tenants
- 100 AGE clusters with read replicas
- 50 API pods
- 100 worker pods
- NATS cluster (3 nodes)
- Multi-region (US, EU, APAC)
- Cost: ~$50K/month

### Phase 5: 1,000,000-100,000,000 users
- Not tenants - USERS. 100M users across ~500K orgs.
- 500 AGE clusters across 3 regions
- 200 API pods per region
- 500 worker pods per region
- Cloudflare Workers handle auth + routing at edge
- Graph queries cached at edge for popular patterns (5min TTL)
- Cold storage tier for inactive orgs (S3/R2 + restore on demand)
- Cost: ~$500K/month. Revenue at this scale: ~$50M+/month.

## What NOT to Build Early
- Multi-region (single region until you have customers in 3+ time zones)
- Graph sharding (AGE handles single-tenant graphs up to 10M nodes)
- Custom auth provider (use Google/GitHub OAuth until enterprise needs SAML)
- Admin dashboard (API + Slack bot is enough until 1,000 tenants)
- Data export/portability (build when customers ask, not before)

## Critical Path to First External User
1. API key generation endpoint (POST /api/v1/org/keys)
2. Tenant graph creation on first reindex
3. Usage metering writing to Postgres
4. Stripe checkout for PAYG billing
5. savants login flow (device auth -> API key)

That's 5 endpoints. Everything else is already built.
