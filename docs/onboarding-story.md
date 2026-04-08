# The Mazkir Onboarding Story

**Status:** Settled product narrative, decided 2026-04-08. This is
both the customer-facing onboarding doc *and* the product spec for
what we have to build. **Every command in this doc must be a command
we actually ship.** If a command appears here that doesn't exist yet,
that's a roadmap item, not a doc bug.

This document walks through the complete customer journey from
"never heard of Mazkir" to "$338K/year enterprise contract." It
covers four expansion moments:

1. **Solo developer** installs the free local tier (Day 1)
2. **Team upgrade** to the cloud tier with auto-indexing (Day 7)
3. **Multi-environment federation** — adding K8s clusters, AWS
   accounts, GCP projects, Slack (Days 14-60)
4. **Enterprise upgrade** with on-prem deployment + Confidential
   Compute (Day 180)

Each stage shows the actual commands the customer runs, what happens
behind the scenes, and the architecture at that moment.

---

## Cast (fictional but realistic Acme Corp)

| Persona | Role | Trigger to adopt Mazkir |
|---|---|---|
| **Sarah** | Staff engineer, payments team | Frustrated debugging an incident, wants better Claude context |
| **Bob** | DevOps lead | Notices 8 devs each running their own local index, wants team consistency |
| **Carol** | SRE manager | Hears about it during an incident, wants the runtime layer |
| **Dave** | CISO | Compliance gate for the enterprise upgrade |

**Acme Corp setup:** 100 engineers, 3 repos (backend Python+FastAPI,
frontend Next.js+TS, data-pipeline Python+Prefect), 2 K8s clusters
(prod-eu, prod-us), AWS production account in us-east-1 + eu-west-1,
1 GCP project for ML workloads, Slack workspace, Linear, GitHub
Enterprise.

---

## Day 1, 14:00 — Sarah hears about Mazkir

Sarah is debugging a production incident. She's asking Claude in
Cursor: *"what calls payment_handler in our codebase?"* Claude
runs grep, returns 47 raw text matches — half are comments, half
are in tests, none are clearly the actual callers. She has to
manually filter for 20 minutes before she finds the 4 real callers.

She remembers seeing Mazkir on Hacker News two weeks ago. *"Structural
code intelligence with MCP integration for AI agents — local-first,
free, closed source."* She decides to try it.

---

## Day 1, 14:05 — Install the local tier

```
$ curl -fsSL https://get.mazkir.io | sh
Installing Mazkir 1.0.0 for linux-x86_64...
✓ Downloaded mazkir binary (28 MB)
✓ Installed to /usr/local/bin/mazkir
✓ Bundled FalkorDB module + redis-server installed to ~/.mazkir/bin/
✓ Running mazkir doctor...

Mazkir doctor
==========================================
Index backend:    not running yet (will start on first use)
Module:           ✓ /home/sarah/.mazkir/bin/falkordb.so
Backend host:     ✓ /home/sarah/.mazkir/bin/falkordb-server-bundled
libgomp:          ✓ found

All checks passed. Ready to index your first repo.

Run: mazkir init /path/to/your/repo
```

**Total elapsed time: 30 seconds.** No Docker. No package manager
dance. No FalkorDB knowledge required. The bundled binaries handle
everything.

> **Behind the scenes:** the curl-installer detects platform
> (linux/macos/windows × x86_64/arm64), downloads the right tarball
> from GitHub releases, extracts to `~/.mazkir/`, symlinks the CLI
> into `/usr/local/bin/`, and runs `mazkir doctor` to verify health.
> If anything's missing it tells the user how to fix it. NixOS gets
> special-cased to use the system `redis-server` because the bundled
> ELF can't run there without nix-ld.

---

## Day 1, 14:06 — First index

```
$ mazkir init ~/work/acme/backend
Indexing /home/sarah/work/acme/backend...
✓ Started backend on port 16379
Code graph:   1247 files, 8423 functions, 1567 classes, 22158 edges
Config:       34 files, 2890 keys
Env vars:     89 references
Symbol refs:  412 string-literal edges
Decorators:   1834 (including 47 @app.route, 12 @workflow.defn,
              156 @pytest.fixture, 89 @dataclass)
Walking last 300 commits for Layer 2 (history)...
History:      300 episodes, 18234 CHANGES edges
Bookmark saved at a3f5b8c1
```

12 seconds for a 1247-file repo. Sarah immediately runs the question
that started this:

```
$ mazkir ask payment_handler
Found 1 definition:
  [Function] payment_handler  (src/payments/handlers.py:142)

Direct callers (4):
  process_order             (src/orders/processing.py:88)
  retry_failed_payment      (src/cron/retry.py:23)
  test_payment_flow         (tests/test_payments.py:147)
  benchmark_charge_throughput (benchmarks/payment_perf.py:34)

String-literal references (2):
  setup_handlers            (src/registry/handler_registry.py:14)
  load_handler_by_name      (src/registry/dispatch.py:67)

Decorators: ['app.route', 'metrics.timed']
Methods of class: (none — top-level function)

Liveness: not yet available (no production trace data connected)
```

**The "oh shit" moment.** That's the answer she spent 20 minutes
manually filtering for, returned in 8 milliseconds. Plus two
registry-dispatch references that grep would have missed entirely
(because they're string keys looked up by `dispatch.py`'s registry,
not literal function calls).

---

## Day 1, 14:10 — Wire up Claude Code (or Cursor / Continue)

```
$ mazkir mcp install --client claude-code
Detected Claude Code at ~/.claude.json
✓ Wrote MCP server config:
    server name: mazkir
    command:     /usr/local/bin/mazkir serve
    env:         { MAZKIR_GRAPH: backend }

Restart Claude Code (or run /mcp reload) to activate.
After activation, Claude will have access to 16 structural query tools:
  function_xray, find_references_structured, decorated_with,
  diff_impact, search_code, resolves_to, impact_analysis,
  dependency_chain, co_change_partners, coupling_check,
  pre_change_warning, risk_score, recall_history,
  community_summary, graph_stats, advanced_graph_query

Need help configuring? mazkir help mcp
```

She restarts Claude Code. Asks Claude the same `payment_handler`
question. **Claude calls `function_xray` directly via MCP**, gets
the structured answer in <100ms, and gives Sarah a thoughtful
analysis instead of a wall of grep matches. The cost difference:
~5,000 fewer tokens per query because the MCP returns structured
data instead of raw lines that Claude has to parse.

---

## Day 1, 14:30 — Sarah Slacks her team

> **Sarah:** y'all need to try this thing called Mazkir. It made
> Claude actually useful for understanding our codebase.
> `curl get.mazkir.io | sh` then `mazkir init <repo>`. Free.

By end of day, 8 people on her team have it installed. Each one
has their own local index. Each one re-indexes manually whenever
they remember to.

---

## Day 7 — Bob (DevOps lead) decides this needs to be a team thing

Bob notices 8 developers are independently indexing the same repo
on their laptops. **Stupid. Wasteful. Stale.** Each laptop has a
different bookmark and they all see slightly different graphs.

He upgrades to the team cloud tier:

```
$ mazkir login
Opening browser to https://app.mazkir.io/login...
✓ Authenticated as bob@acme.com via Google SSO
✓ Created org "acme" (you are admin)
✓ Detected 8 existing local users on @acme.com domain — invite them?
  [y/N] y
✓ Sent invites to: sarah@, alice@, carol@, dave@, eve@, frank@, grace@, henry@
```

Then he sets up the cloud-hosted graph for the backend repo:

```
$ mazkir cloud connect --repo ~/work/acme/backend
This will:
  • Upload your local graph to the cloud (one-time, ~12 MB)
  • Configure GitHub webhook for auto-indexing on every push
  • Switch your local CLI to query the shared cloud graph instead
  • Enable team-wide access with org SSO
  • Cost: $20/dev/month after the 15-day trial

Continue? [y/N] y

✓ Uploaded graph (12 MB in 3.2s)
✓ Created repo binding: acme/backend → mazkir-acme-backend
✓ GitHub webhook (paste this URL into your repo settings):
    https://api.mazkir.io/webhooks/github/abc123def456
✓ Or auto-install via the GitHub CLI:
    mazkir cloud github install acme/backend
✓ Switched local mazkir CLI to cloud mode

Your team can now query the same always-fresh graph via:
  mazkir ask <symbol>            # CLI (queries the cloud)
  Claude Code → function_xray    # MCP (queries the cloud)
  https://app.mazkir.io/acme     # Web UI
```

Bob runs `mazkir cloud github install acme/backend` and the GitHub
CLI configures the webhook automatically. **From now on, every push
to the repo triggers a sub-second incremental re-index.** The shared
team graph is always fresh.

He repeats for the other two repos (`frontend` and `data-pipeline`).
5 minutes total per repo.

He sends his team a magic link: `mazkir login --org acme`. Every
developer on the team joins, the local CLI auto-detects the cloud
config, and from that moment on every `mazkir ask`, every Claude
Code MCP call, and every Cursor query hits the same shared graph.
No more stale local indexes. No more disagreeing answers.

> **Behind the scenes (federation server is now in play):**
> The acme org now has 3 underlying graphs in the federation: backend,
> frontend, data-pipeline. The federation server holds a registry
> mapping `(acme, backend) → backend.graph.acme.mazkir.io`. When a
> developer runs `mazkir ask payment_handler` and the function exists
> in multiple repos, the federation server queries all of them in
> parallel and joins the results, tagging each row with which repo
> it came from. Stable node IDs (`Function:backend:src/payments/...`)
> make the join unambiguous.

---

## Day 14 — Carol (SRE manager) hears about it during an incident

A cron job in production is failing every 15 minutes. Nobody knows
why. Carol pages the on-call. The on-call asks Claude (via Mazkir)
*"what calls retry_failed_payment and what does it actually do?"* —
gets the answer in seconds, finds the bug in 10 minutes instead of
the 2-hour grep-and-cross-reference investigation it would have been.

Post-incident, Carol asks Bob: **"can this thing also tell us what's
running in our K8s clusters?"**

That's the unlock moment for Layer 4 (the runtime layer).

---

## Day 21 — Carol installs the K8s integration

She gets a single command from the Mazkir docs:

```
$ kubectl apply -f https://app.mazkir.io/install/k8s/operator.yaml \
    --token-from-secret=mazkir-acme-prod-eu

namespace/mazkir-system created
serviceaccount/mazkir-operator created
clusterrole.rbac.authorization.k8s.io/mazkir-operator created
clusterrolebinding.rbac.authorization.k8s.io/mazkir-operator created
secret/mazkir-token created
deployment.apps/mazkir-operator created

$ kubectl -n mazkir-system logs deployment/mazkir-operator -f
[INFO] Mazkir K8s operator 1.0.0 starting
[INFO] Authenticating to api.mazkir.io as cluster prod-eu (org acme)
[INFO] Watching: deployments, statefulsets, daemonsets, pods,
       configmaps, secrets, services, ingresses
[INFO] Initial sync: 47 deployments, 142 pods, 23 configmaps,
       8 services, 12 secrets
[INFO] Synced cluster state to mazkir cloud in 3.2s
[INFO] Watching for changes via K8s watch API...
```

> **Behind the scenes:** the operator uses a scoped token that
> identifies it as `cluster=prod-eu, org=acme`. The token is bound
> to the cluster scope only — it cannot read or write any other
> graphs. The operator's RBAC is read-only on the K8s API
> (`get/list/watch` on the watched resource types). It does not
> need write permission anywhere. It runs as a regular Deployment
> in the `mazkir-system` namespace and survives pod restarts via
> standard K8s mechanisms.

She repeats for `prod-us`. Now Mazkir's federation server has 5
underlying graphs for acme:

```
acme org graphs:
  ├─ Code:    backend
  ├─ Code:    frontend
  ├─ Code:    data-pipeline
  ├─ Runtime: k8s/prod-eu
  └─ Runtime: k8s/prod-us
```

She asks Claude:

> *"what's currently deployed for the payments service in prod-eu?"*

Claude calls a single MCP tool (`cluster_state`). Behind the scenes,
the federation server queries the prod-eu runtime graph + the
backend code graph + joins them by image SHA. Returns:

```
payments service in prod-eu:
  Image:        registry.acme.io/payments:v2.4.1
  Image digest: sha256:abc123def456...
  Built from:   commit def456 by alice@acme.com on 2026-04-08 08:14
  Replicas:     12 (12 ready, 12 available)
  Last deploy:  6 hours ago via Argo CD (alice@)
  Health:       all pods running, 0 restarts in last hour

  Recent incidents touching this code path:
    PD-12345  2 days ago, affected /payments/* for 3 min,
              root-caused to commit abc123

  Owned by:       payments-platform team
  Current oncall: @bob (PagerDuty rotation: payments-primary)
  Slack channel:  #payments-platform

  What changed in the last deploy (v2.4.0 → v2.4.1):
    8 functions modified
    1 new function: handle_partial_refund
    1 removed: legacy_charge_v1 (was test-only, no prod callers)
    Config keys touched: payments.timeout_ms (30000 → 45000)
```

Carol's response: *"ok this is the thing we've been missing for
five years."* This was 8-15 separate `kubectl` / `git` / `argocd` /
`pagerduty` round-trips before Mazkir. Now it's one MCP call.

---

## Day 28 — Adding AWS

Bob extends the integration to AWS. There's no agent to deploy in
their AWS account — just an EventBridge rule and an API destination.

```
$ mazkir cloud connect --provider aws --account 123456789012 \
                       --region us-east-1
This will generate a CloudFormation template that creates:
  • EventBridge rule (filters CloudTrail events to interesting ones:
    deploys, IAM changes, S3 bucket policy changes, RDS modifications,
    Lambda updates, ConfigMap-equivalents, etc.)
  • EventBridge API destination → POST https://api.mazkir.io/ingest/aws
  • IAM role for the API destination (scoped to invoke the destination only)
  • SNS topic for delivery failures
  • All resources tagged: mazkir:org=acme, mazkir:scope=aws/123456789012/us-east-1

Generating template... ✓
Apply via the CloudFormation console:
  https://console.aws.amazon.com/cloudformation/home?region=us-east-1
  #/stacks/quickcreate?templateURL=https://app.mazkir.io/aws/install/abc123

Or via the AWS CLI:
  aws cloudformation create-stack --stack-name mazkir-ingest \
    --template-url https://app.mazkir.io/aws/install/abc123 \
    --capabilities CAPABILITY_NAMED_IAM \
    --region us-east-1
```

Bob runs the CloudFormation. Within 60 seconds, the stack creates
the EventBridge rule + API destination. **No Lambda functions in
acme's account.** No persistent compute they have to operate. Just
an event forwarder + a webhook URL.

Within 5 minutes, Mazkir starts seeing every meaningful AWS event
from that account. After ~10 minutes the AWS runtime graph for
acme has every Lambda function, every IAM role, every S3 bucket,
every DynamoDB table, every RDS instance, every CloudFront
distribution, every API Gateway endpoint that exists in the account.

> **Behind the scenes:** the EventBridge rule pattern filters
> CloudTrail events to the ~150 event types that actually matter
> (CreateFunction, UpdateFunctionCode, PutBucketPolicy, etc. — not
> the millions of routine GetObject calls). The API destination
> POSTs each filtered event as a JSON document to
> `https://api.mazkir.io/ingest/aws/{org_id}` with HMAC
> authentication. The Mazkir webhook receiver validates the HMAC,
> identifies the org from the URL, and writes a delta to the
> appropriate AWS runtime graph for that scope.
>
> For deeper inventory (e.g., "what's the current set of all Lambdas
> in this account, including ones that haven't changed lately"),
> the customer can optionally grant a read-only IAM role
> (`AWSReadOnlyAccess` or a custom policy) and Mazkir polls the
> inventory APIs once per hour. This is opt-in. The MVP works
> without it because the event stream catches all subsequent
> changes.

He repeats for `eu-west-1`. Now acme has 7 graphs:

```
acme org graphs:
  ├─ Code:    backend
  ├─ Code:    frontend
  ├─ Code:    data-pipeline
  ├─ Runtime: k8s/prod-eu
  ├─ Runtime: k8s/prod-us
  ├─ Runtime: aws/123456789012/us-east-1
  └─ Runtime: aws/123456789012/eu-west-1
```

Bob does GCP next week using the same pattern (Cloud Audit Logs →
Pub/Sub → API destination). 8 graphs.

Carol asks Claude:

> *"are any of our Lambda functions running code that's been changed
> in the last week?"*

The federation server hits the AWS runtime graphs to find Lambdas,
joins each Lambda's image-source-commit with the backend code graph's
recent commit history, returns: *"4 Lambdas are running images built
from commits in the last week. Here they are with commit messages,
deployers, deployment times, and the functions that changed."*

That's a query that takes a SRE 30 minutes minimum today. Mazkir
does it in ~200ms.

---

## Day 35 — The federated query that proves the value

Carol is reviewing PR #847. It changes a function deep in the
payments code. She wants to know the full blast radius before
approving. She asks Claude:

> *"If I merge PR #847, what services in production could be
> affected? Include K8s + Lambda + ECS, tell me which environments,
> and tell me which oncall to ping."*

Claude calls `diff_impact` on the federation server. **Behind the
scenes (this is the entire moat in one query):**

1. Federation server hits the **backend code graph** to compute
   structural reach of the changed functions in PR #847
2. Hits the **prod-eu K8s runtime graph** for currently-deployed
   services containing those functions → finds 1 deployment
   (`payments-api`)
3. Hits the **prod-us K8s runtime graph** → finds 1 deployment
   (`payments-api`)
4. Hits the **AWS us-east-1 runtime graph** → finds 2 Lambdas
   (`payment-processor`, `payment-retry-handler`)
5. Hits the **AWS eu-west-1 runtime graph** → finds 1 Lambda
   (`payment-eu-backup`)
6. Cross-references each affected service with PagerDuty for
   current oncall via the cached oncall edge
7. Returns one unified response in ~400ms

Carol gets:

```
Diff impact for PR #847 (payment_handler refactor):

Currently deployed services that contain functions touched by this PR:

  K8s:
    payments-api in prod-eu  (deployed 6h ago, last incident PD-12345 2d ago)
    payments-api in prod-us  (deployed 6h ago, healthy)

  AWS Lambda:
    payment-processor       (us-east-1, deployed 4d ago)
    payment-retry-handler   (us-east-1, deployed 1d ago)
    payment-eu-backup       (eu-west-1, deployed 14d ago, STALE)

Owned by:        payments-platform team
Current oncall:  @bob (PagerDuty rotation: payments-primary)

⚠ HIGH RISK to merge:
  - touches 5 production deployments across 4 regions
  - one related incident (PD-12345) in the last week
  - payment-eu-backup is 14 days behind main branch — out of sync risk
  - test coverage: 67% of changed lines (below team threshold of 80%)
  - liveness: payment_handler hit by 12,847 prod requests in the last hour
    (NOT a dormant code path — actively serving traffic)

Suggested action:
  1. Ping @bob on Slack (#payments-platform) before merging
  2. Deploy to prod-eu first, monitor for 30 min, then prod-us
  3. Update payment-eu-backup to latest after merge
  4. Add test coverage for handle_partial_refund (new function in this PR)
```

**Six different systems queried, one unified answer, in under half
a second.** Carol now uses this on every PR review. Bob refuses to
approve merges to payments without the diff_impact output attached.
The team's senior engineers start using it pre-emptively before
they push: *"let me check if this is safe."*

**This is the moment Mazkir becomes load-bearing for the team.**

---

## Day 60 — Slack integration (the knowledge layer kicks in)

Bob adds the Slack integration:

```
$ mazkir cloud connect --integration slack
Opening browser to authorize Mazkir for acme.slack.com via OAuth...
✓ Authorized by bob@acme.com
✓ Permissions: read-only access to public channels + channels you join
✓ Indexing the last 90 days of #engineering, #incidents,
  #payments-platform, #data-platform, #frontend (5 channels)
✓ Synced 12,847 messages
✓ Found 89 references to function names in your code graph
✓ Found 23 references to deployment events in your runtime graphs
✓ Linked 14 incident channel discussions to PagerDuty incidents
```

> **Behind the scenes:** the Slack integration runs as a webhook
> receiver in Mazkir's cloud (or in customer VPC for the enterprise
> tier). It receives Slack events for the channels the customer
> authorized, runs them through the same string-literal symbol
> matching the parser uses for code, and creates `MENTIONS_SYMBOL`
> edges from `SlackMessage` nodes to `Function`/`Class`/`Deployment`
> nodes. Now the graph contains structural code + history + runtime
> + conversational context, all joined.

Now when Carol asks *"did anyone discuss the payment_handler refactor
in Slack?"* — Mazkir cross-references function names against
indexed Slack messages and returns the relevant threads with links.
The Stephen/HandleTsCoinTransfer pattern from earlier in this
project, but done automatically and continuously.

acme org now has 9 graphs:

```
acme org graphs:
  ├─ Code:        backend
  ├─ Code:        frontend
  ├─ Code:        data-pipeline
  ├─ Runtime:     k8s/prod-eu
  ├─ Runtime:     k8s/prod-us
  ├─ Runtime:     aws/123456789012/us-east-1
  ├─ Runtime:     aws/123456789012/eu-west-1
  ├─ Runtime:     gcp/acme-ml-prod
  └─ Knowledge:   slack/acme
```

Coming next week: Linear (tickets), Notion (docs), Granola (meeting
transcripts).

---

## Day 90 — Steady state, daily use

Mazkir is invisible but always-on. The acme team uses it every day
without thinking about it:

| Workflow | Mazkir's role |
|---|---|
| **Every PR** | A GitHub Action calls `mazkir diff-impact` and posts the structured blast radius as a PR comment |
| **Every Claude Code session** | All 16 MCP tools available, so Claude has structural awareness without prompting |
| **Every Cursor query** | Same MCP integration |
| **Every incident** | A Mazkir Slack bot auto-posts "what changed in the last hour that touches the affected service" the moment a `#incidents` channel is created |
| **Every new hire** | First-day onboarding uses `mazkir explain <service>` to generate auto-doc from the graph |
| **Every architecture review** | Starts with `mazkir architecture summary --org acme` — the auto-generated org-wide service map |
| **Every refactor planning** | `mazkir impact <function>` to scope the work before any code is written |
| **Every config change** | `mazkir config-impact <key>` to find every service reading that config |
| **Every deprecation** | `mazkir liveness <function>` to verify nothing in production is still calling it |

**Cost:** $20/dev/month × 100 devs = $2,000/month for acme. Mazkir's
infrastructure cost to serve acme: ~$0.20/customer/month (per the
scaling math in `docs/scaling-and-throughput.md`).

**Margin contribution from acme alone:** ~$23,800/year × ~99% gross
margin = $23,560/year.

---

## Day 180 — Enterprise upgrade conversation

Dave (CISO) reviews Mazkir for SOC 2 compliance as part of acme's
annual vendor security review. He's concerned about source code
metadata leaving acme's network. Bob walks him through:

1. **Metadata-not-source story** (per `docs/strategy-and-business-model.md`)
   — Mazkir stores function names and call edges, not function
   bodies. The 450× compression ratio means even if Mazkir's
   database is breached, the attacker gets a graph of "function
   `gen_handle_block` calls function `populate_default_setup`" —
   not the actual source code.
2. **Secret scrubber** (per `docs/strategy-and-business-model.md`,
   section 5) — Mazkir scrubs known secret patterns at ingest and
   stores only fingerprints, never the secret values themselves.
3. **No PII** beyond git author email, which is hashable on
   request for GDPR compliance.
4. **Empirical verification** — Dave runs `tcpdump` against the
   local Mazkir and sees zero outbound traffic from the local tier.
   Trust comes from packet captures, not from claims.

Dave is partially convinced but wants belt-and-suspenders for the
production graph that includes their AWS account state. He requests
the enterprise tier with full self-hosting.

```
$ mazkir cloud upgrade --plan enterprise
This will:
  • Schedule a deployment call with Mazkir's solutions engineer
  • Migrate your graphs from us-east-1 cloud to a self-hosted instance
    in your own AWS / k8s / on-prem
  • Enable Confidential Compute (Nitro Enclave) mode for the
    federation server (requires a dedicated EC2 instance with
    Nitro Enclaves enabled)
  • Add SAML SSO via Okta / Azure AD / Google Workspace
  • Add audit log streaming to your SIEM (Splunk, Datadog, Elastic)
  • Increase price to ~$50K/year base + $40/dev/month
  • Provide dedicated Slack channel for support

Continue? [y/N] y
✓ Created enterprise inquiry ticket. SE will reach out within
  1 business day.
```

**Two install paths for the enterprise tier:**

### Path A: AWS Marketplace (the default, ~30 min, no SE)

Bob goes to AWS Marketplace, searches "Mazkir," subscribes, and
clicks "Deploy via CloudFormation." Within 10-15 minutes, AWS has
deployed the entire Mazkir stack into acme's existing EKS cluster:

- Mazkir Helm release in `mazkir-system` namespace
- IAM role with least-privilege scoping for AWS event ingestion
- EventBridge rule + API destination targeting the in-VPC Mazkir
  webhook (`mazkir.acme.internal`)
- S3 bucket for cold archive (per `docs/runtime-layer-retention-and-gc.md`)
- Secrets Manager entry for the Mazkir license token
- Application Load Balancer for the internal MCP endpoint

**Billing flows through AWS** — acme's existing AWS Enterprise
Discount Program commit covers the Mazkir cost. No new vendor
onboarding, no new MSA, no new payment terms. The enterprise sales
cycle that would have taken 6-9 months collapses to 2-4 weeks.

(GCP and Azure Marketplace work identically — same pattern, same
one-click deploy, just for customers running on different clouds.)

### Path B: SE-led Helm install (the fallback for customers who can't use Marketplace)

For customers in regulated industries that can't use Marketplace
(some FedRAMP / DoD customers, some EU sovereign-cloud requirements),
a Mazkir SE schedules a call and walks Bob through deploying the
Helm chart manually:

```
$ helm repo add mazkir https://charts.mazkir.io
$ helm repo update
$ helm install mazkir mazkir/mazkir-enterprise \
    --namespace mazkir-system \
    --create-namespace \
    --values values.acme.yaml
```

Either way, the whole Mazkir stack — federation server, per-scope
FalkorDB instances, MCP endpoint, web UI, billing webhook receiver,
audit log writer — now lives **inside acme's EKS cluster**. CloudTrail
events still POST to a Mazkir webhook, but that webhook is now
`mazkir.acme.internal`, not `api.mazkir.io`. **No data ever leaves
acme's network.**

The federation server is the same software they were using on
mazkir.io's hosted version. Same MCP tool surface. Same query
syntax. The only thing that changed is where it runs.

> **Behind the scenes:** the enterprise installer creates separate
> FalkorDB pods for each scope (one per repo, one per cluster, one
> per AWS account). Each pod has its own PVC for persistence. The
> federation server sits in front of them, holding the registry of
> which scope lives where. The MCP endpoint is exposed via an
> internal Service that acme's network policies restrict to the
> developer subnet only.
>
> The Confidential Compute mode runs the federation server inside
> a Nitro Enclave on a dedicated `m5.metal` instance. The enclave
> has no network access except via vsock to the host, and its
> attestation document is verified by every customer query before
> the query is allowed to proceed. This is the highest paranoia
> level — most customers don't enable it, but it's available for
> the ones that need it.

Dave signs off. Acme expands Mazkir to all 6 of their engineering
teams. **Total revenue: $50K/yr base + $40 × 600 devs/month = $338K/year ARR
from one customer.**

---

## What just happened (the architecture in one diagram)

After Day 180, acme's Mazkir deployment looks like this:

```
                  acme.internal (acme's own AWS VPC)
                  ┌─────────────────────────────────────────┐
                  │                                         │
                  │  Mazkir Federation Server               │
                  │  (Nitro Enclave on m5.metal,            │
                  │   exposes MCP endpoint to acme's        │
                  │   internal network only)                │
                  │                                         │
                  └────────┬────────────────────────────┬───┘
                           │                            │
       ┌─────────┬─────────┼─────────┬──────────────────┼──────────┐
       │         │         │         │                  │          │
       ▼         ▼         ▼         ▼                  ▼          ▼
  ┌──────┐ ┌──────┐ ┌────────┐ ┌────────┐ ┌────────────┐ ┌────────┐
  │Code  │ │Code  │ │Runtime │ │Runtime │ │AWS Runtime │ │Slack   │
  │backnd│ │frnt  │ │k8s prod│ │k8s prod│ │us-east-1   │ │acme    │
  │      │ │      │ │ -eu    │ │ -us    │ │+ eu-west-1 │ │        │
  └──────┘ └──────┘ └────────┘ └────────┘ └────────────┘ └────────┘
                          │          │
                          │          │ (each k8s graph is fed by
                          ▼          ▼  the operator running in that
                    [in-cluster Mazkir   cluster, via watch API)
                     operator pods]

  All graphs live INSIDE acme's network.
  No data ever crosses acme.internal → public internet.
  Federation server is the single MCP endpoint Claude/Cursor talk to.
```

Same architecture as the cloud-tier multi-tenant deployment, just
relocated into the customer's own network. **Same software, same
contracts, same MCP tool surface.** Customers can develop against
the local tier and seamlessly upgrade through team → enterprise
without changing any code in their Claude Code config or their
GitHub Actions.

---

## The progression in numbers

| Day | Stage | Acme spent | Acme's value |
|---|---|---|---|
| 1 | Local install | $0 | Sarah saved 20 min on one query |
| 7 | Team cloud | $20/dev/month | 100 devs × ~5 queries/day, never stale |
| 21 | + K8s integration | same | "what's running" answered in <500ms |
| 28 | + AWS integration | same | Lambda + IAM + S3 visibility |
| 35 | First federated diff_impact | same | PR review time cut from 30 min to 30 sec |
| 60 | + Slack integration | $50/dev/month (Business) | Conversational context joined to code |
| 180 | Enterprise upgrade | $338K/year | On-prem, Nitro Enclave, SOC 2 ready |

**Revenue ramp from a single customer:** $0 → $24K/year → $338K/year
in 6 months. Multiply by 50 customers per year on the same trajectory
and the Phase 4 revenue target ($60M ARR) is reachable.

---

## What this story commits us to building

Every command in this doc is a product spec. Here's the explicit
build list, organized by phase per `docs/roadmap-to-1b.md`:

### Phase 0 (now → month 1) — Foundation
- [ ] `curl get.mazkir.io | sh` installer
- [ ] `mazkir doctor` (already exists, needs polish)
- [ ] `mazkir init <repo>` (already exists)
- [ ] `mazkir ask <symbol>` (already exists)
- [ ] `mazkir mcp install --client claude-code` (one-command MCP setup)
- [ ] PyPI wheel with bundled binaries (already in `pyproject.toml`)
- [ ] Per-platform release tarballs on GitHub releases

### Phase 1 (months 1-4) — Wedge
- [ ] `mazkir login` (OAuth via Google/GitHub)
- [ ] `mazkir cloud connect --repo <path>` — uploads local graph,
      configures GitHub webhook
- [ ] `mazkir cloud github install <org/repo>` — automated webhook setup
- [ ] Cloud receiver: `https://api.mazkir.io/webhooks/github/{token}`
- [ ] Web UI at `app.mazkir.io` for browsing the shared graph
- [ ] Stripe billing integration with $20/dev/month and 15-day trial
- [ ] Tauri desktop launcher (the legibility breakthrough)

### Phase 2 (months 4-9) — Cloud tier MVP
- [ ] Multi-tenant federation server with per-org graph routing
- [ ] Per-repo code graphs in the federation
- [ ] GitHub webhook auto-indexing on push (incremental update via existing builder)
- [ ] Team SSO (Google, GitHub, Microsoft)
- [ ] `mazkir cloud connect --integration slack`
- [ ] Slack OAuth flow + channel ingestion + MENTIONS_SYMBOL edges

### Phase 3 (months 9-18) — Live infrastructure layer
- [ ] **K8s operator** with cluster-scoped tokens
- [ ] `kubectl apply -f https://app.mazkir.io/install/k8s/operator.yaml`
- [ ] K8s watch API → delta events → cluster runtime graph
- [ ] **AWS integration** via EventBridge API destinations (no in-account agent)
- [ ] CloudFormation template generator: `mazkir cloud connect --provider aws`
- [ ] Optional read-only IAM role for deeper inventory polling
- [ ] **GCP integration** via Cloud Audit Logs → Pub/Sub → API destination
- [ ] **Azure integration** via Activity Log → Event Grid → API destination
- [ ] `cluster_state` MCP tool (the killer query)
- [ ] Federated `diff_impact` that joins code + K8s + AWS + GCP + Azure runtime graphs
- [ ] Liveness fingerprints from OpenTelemetry trace ingestion

### Phase 4 (months 18-30) — Enterprise
- [ ] Helm chart for self-hosted: `helm install mazkir mazkir/mazkir-enterprise`
- [ ] **AWS Marketplace listing** (Container product + CloudFormation template)
- [ ] **GCP Marketplace listing** (Container product + Deployment Manager template)
- [ ] **Azure Marketplace listing** (Container product + ARM template)
- [ ] Marketplace Metering API integration (usage reporting → billing)
- [ ] Confidential Compute mode (Nitro Enclave)
- [ ] SAML SSO (Okta, Azure AD, Google Workspace)
- [ ] SCIM provisioning
- [ ] Audit log streaming to Splunk / Datadog / Elastic
- [ ] SOC 2 Type II + ISO 27001 certifications
- [ ] Linear / Jira / Notion / Confluence integrations
- [ ] PagerDuty integration for ownership/oncall edges
- [ ] Granola / Otter / Zoom meeting transcript integration
- [ ] `mazkir cloud upgrade --plan enterprise` flow + SE workflow

---

## How to use this document

- **For onboarding new customers:** walk them through the linear
  story above, customized to their stack
- **For product decisions:** every command has to be real and
  shippable; if a stakeholder proposes an alternative onboarding
  path, check whether it's already in this doc
- **For roadmap prioritization:** the build list at the bottom is
  the explicit ordering — Phase 0 commands have to ship before
  Phase 1, and so on
- **For competitive defense:** the multi-environment federation
  story (Day 35 onwards) is what no other tool does. Cursor /
  Glean / Sourcegraph cannot answer the federated `diff_impact`
  query. That's the moat in concrete user-story form.

If a feature gets proposed that doesn't fit anywhere in this
narrative — push back. The story is the contract for what Mazkir
is. Adding things outside the story dilutes the product.
