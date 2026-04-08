# Live Infrastructure Layer (K8s + Cloud Providers)

**Status:** Settled design, decided 2026-04-08. The "runtime layer"
docs (`docs/runtime-layer-*.md`) are the foundation; this document
extends that design to cover **K8s + AWS + GCP + Azure as a unified
live infrastructure layer**, accessed via a single MCP tool surface
and federated under one MCP endpoint.

This is the architectural foundation for Pillar 3 of
`docs/competitive-defense.md`: build the layer competitors don't
want to build.

---

## The unified pitch

> **Mazkir is the structural memory layer for everything that runs
> your code — your repositories, your K8s clusters, your AWS
> accounts, your GCP projects, your Azure subscriptions. One graph
> per scope, federated under one MCP endpoint. One query language
> across all of them.**

That positioning competes with **nobody specifically** because no
existing tool does the cross-cutting join (see the table in
`docs/competitive-defense.md`).

---

## Sources of truth that get projected into the live infra layer

| Source | What we capture | Update mechanism | Update cadence |
|---|---|---|---|
| **K8s API** (any cluster) | Deployments, StatefulSets, DaemonSets, Pods, Services, Ingresses, ConfigMaps, Secrets (names only), HPAs, Jobs, CronJobs | In-cluster operator → watch API → delta events → cloud webhook | <1s freshness via watch |
| **AWS CloudTrail** | Lambda functions, EC2 instances, IAM roles & policies, S3 buckets, DynamoDB tables, RDS instances, ECS services, API Gateway, CloudFront, SQS, SNS, EventBridge, Step Functions, ECR repos | EventBridge rule → API destination → mazkir webhook (no in-account agent) | <5s freshness via event stream |
| **AWS inventory APIs** (optional, opt-in) | Full snapshots of resource state for "what currently exists" queries | Polled hourly via read-only IAM role | hourly |
| **GCP Cloud Audit Logs** | Cloud Run, Cloud Functions, GKE workloads, Pub/Sub, BigQuery datasets, IAM bindings, Cloud SQL, Storage buckets | Audit log → Pub/Sub → mazkir webhook | <5s |
| **Azure Activity Log** | App Service, Functions, AKS, Storage Accounts, RBAC, SQL Database, Cosmos DB | Activity log → Event Grid → mazkir webhook | <5s |
| **PagerDuty** | Incidents, oncall rotations, escalation policies, services | Webhook + REST API for snapshots | event-driven + hourly snapshot |
| **CI/CD systems** (optional) | Build events, deploy events, image SHAs, build commits | Webhooks (GitHub Actions, GitLab CI, Argo CD, Flux, Jenkins) | event-driven |

**The pattern is identical across all five environments:** something
in the customer's infrastructure produces a stream of events; an
event filter forwards the interesting ones to a Mazkir webhook
endpoint; the receiver writes a delta to the appropriate runtime
graph for that scope.

**No agents inside customer accounts for the MVP.** No long-running
binaries the customer has to operate. No scary "this thing is
running in our prod cluster" security review for the cloud-tier
customers. Just an event filter + a webhook URL. (The K8s case is
the exception — it does need an in-cluster operator to access the
watch API. That's a tradeoff for the deeper visibility.)

---

## Per-environment installation patterns

### K8s installation

```
$ kubectl apply -f https://app.mazkir.io/install/k8s/operator.yaml \
    --token-from-secret=mazkir-{org}-{cluster_name}
```

What it creates:
- `mazkir-system` namespace
- `mazkir-operator` service account
- ClusterRole with read-only access to the watched resource types
- ClusterRoleBinding
- Secret holding the scoped Mazkir token (cluster-scoped, can't read
  any other graphs)
- Deployment running the operator

The operator:
- Watches the K8s API for changes to: Deployment, StatefulSet,
  DaemonSet, Pod, Service, Ingress, ConfigMap, Secret (metadata only,
  not contents), HorizontalPodAutoscaler, Job, CronJob, Namespace
- For each meaningful event, sends a delta to
  `https://api.mazkir.io/ingest/k8s/{org_id}/{cluster_id}`
- Authenticates with the cluster-scoped token via HMAC headers
- Survives pod restarts via standard K8s mechanisms (the watch API
  resumes from the last resourceVersion)

### AWS installation (no in-account agent — the cleanest pattern)

```
$ mazkir cloud connect --provider aws --account 123456789012 \
                       --region us-east-1
```

Generates a CloudFormation template that creates:

1. **EventBridge rule** with a pattern matching ~150 interesting
   CloudTrail event types (filters out the 99% of routine
   GetObject/Describe* noise)
2. **EventBridge API destination** pointing at
   `https://api.mazkir.io/ingest/aws/{org_id}/{account_id}/{region}`
   with HMAC authentication
3. **EventBridge connection** holding the API key for the destination
4. **IAM role** for the API destination (scoped to invoke the
   destination only — no other permissions)
5. **SNS topic** for delivery failure notifications
6. All resources tagged `mazkir:org={org_id}` so the customer can
   audit/remove them as a single unit

The customer applies the template via the CloudFormation console
or `aws cloudformation create-stack`. **Within 60 seconds**, Mazkir
starts seeing every meaningful AWS event from that account.

**Optional Phase 2: read-only IAM role for inventory polling.** Some
customers want a fuller initial snapshot than CloudTrail provides
("show me every Lambda that currently exists, not just the ones
modified in the last 90 days"). For those:

```
$ mazkir cloud connect --provider aws --account 123456789012 \
                       --region us-east-1 \
                       --enable-inventory-polling
```

Generates an additional IAM role with `AWSReadOnlyAccess` (or a
narrower custom policy: just `lambda:List*`, `iam:Get*`, `s3:List*`,
etc.). Mazkir's pollers assume this role hourly to take inventory
snapshots. **Opt-in only** — the MVP works without it.

### GCP installation

Same pattern, different services:

```
$ mazkir cloud connect --provider gcp --project acme-prod
```

Generates a Terraform module (or `gcloud` command) that creates:
1. Cloud Audit Logs sink filtering to interesting events
2. Pub/Sub topic for the sink
3. Pub/Sub push subscription pointing at
   `https://api.mazkir.io/ingest/gcp/{org_id}/{project_id}` with HMAC
4. Service account with `pubsub.publisher` only (no other permissions)

### Azure installation

```
$ mazkir cloud connect --provider azure --subscription <sub_id>
```

Generates an ARM template (or `az` commands) that creates:
1. Activity Log alert rule filtering to interesting events
2. Event Grid subscription
3. Webhook endpoint configuration pointing at Mazkir's ingest URL

---

## The killer cross-environment queries

These are the queries that nobody else can answer well today, made
possible by joining all the runtime graphs under one federation
server:

### Q1: Full blast radius of a code change

> *"If I merge PR #847, what services in production could be
> affected?"*

Federation server queries:
1. Backend code graph → structural reach of changed functions
2. All K8s runtime graphs → which deployments contain those functions
3. All AWS Lambda runtime graphs → which Lambdas were built from
   commits in this branch
4. All GCP Cloud Run / Cloud Functions runtime graphs → same
5. PagerDuty edge for current oncall on each affected service

Returns: every affected production service across every cloud, with
deployment timestamps, current oncall, and recent incidents.

**Today this requires a SRE 30 minutes minimum.** Mazkir does it in
~400ms.

### Q2: Excess IAM permissions audit

> *"What IAM permissions does this Lambda actually need vs what it
> has?"*

Federation server queries:
1. Backend code graph → all `boto3` SDK calls in the source
2. AWS runtime graph → IAM policies attached to the Lambda's
   execution role
3. Cross-references to compute the diff

Returns: list of permissions in the role that are never used by the
code. **The principle-of-least-privilege answer every CISO wants.**

### Q3: S3 bucket blast radius

> *"What's the blast radius of deleting bucket payments-archive?"*

Federation server queries:
1. AWS runtime graph → which Lambdas, ECS tasks, EC2 instances
   reference this bucket (via IAM policies)
2. Code graphs → which source files contain `payments-archive` as
   a string literal
3. Joins them by service identity

Returns: every consumer with file:line locations. Today this is
**effectively impossible** without a manual audit.

### Q4: Dormant cost sinks

> *"Which Lambdas haven't been hit in 90 days but still cost money?"*

Federation server queries:
1. AWS runtime graph → all Lambdas in the account
2. Liveness fingerprints from OpenTelemetry traces → which ones
   have been invoked
3. AWS Cost Explorer reference (the actual cost number lives in
   AWS, we just reference by identity)

Returns: every Lambda with `last_invocation > 90d` plus its
estimated monthly cost.

### Q5: Cross-environment secrets audit

> *"Are any production secrets stored in dev S3 buckets?"*

Federation server queries:
1. AWS runtime graph for prod account → all secrets in Secrets Manager
2. AWS runtime graph for dev account → all S3 buckets and their
   IAM read permissions
3. Cross-account join to find any prod secret name that appears in
   a dev bucket's accessible objects

Returns: every cross-account leak. Today this is impossible without
custom tooling.

### Q6: Deployment fleet consistency

> *"Did this commit's deploy go to all 3 AWS regions and both K8s
> clusters?"*

Federation server queries:
1. Code graph → the commit Episode
2. All K8s runtime graphs → which deployments reference the image
   built from this commit
3. All AWS runtime graphs → which Lambdas reference the image built
   from this commit
4. Cross-references

Returns: yes/no per environment, with timestamps and deployer
identity for each.

---

## Federation topology

The full deployment topology after a customer has connected K8s +
AWS + GCP + Azure + Slack:

```
                         Federation Server
                         (single MCP endpoint
                          customers point Claude at)
                                   │
       ┌─────────┬──────────┬──────┴──────┬──────────┬──────────┐
       │         │          │             │          │          │
       ▼         ▼          ▼             ▼          ▼          ▼
   ┌──────┐ ┌──────┐ ┌──────────┐ ┌────────────┐ ┌────────┐ ┌─────────┐
   │ Code │ │ Code │ │ K8s prod │ │ AWS us-e-1 │ │ GCP    │ │ Slack   │
   │ back │ │ frnt │ │ -eu      │ │ + eu-west-1│ │ ml-prod│ │ acme    │
   └──────┘ └──────┘ └──────────┘ └────────────┘ └────────┘ └─────────┘
       │       │           │             │            │          │
       │       │           │             │            │          │
   GitHub   GitHub      In-cluster    EventBridge   Pub/Sub    Slack
   webhook  webhook     operator      → API dest   subscript  Events
                        (watch API)                            API
```

Each underlying graph:
- Has its own FalkorDB instance (or shared multi-tenant FalkorDB
  with strict namespace isolation)
- Exposes the same MCP tool surface internally
- Updates on its own cadence from its own source of truth
- Tags every node with `source_id` indicating the scope

The federation server:
- Holds the registry of all underlying graphs for a given org
- Routes incoming MCP queries to the right graphs based on the
  query type
- Issues parallel calls to multiple graphs when needed
- Joins results using stable node IDs (per
  `docs/federated-graph-architecture.md`)
- Returns one unified MCP response

**From Claude's perspective, there's still just one MCP endpoint.**
All the federation complexity is hidden.

---

## Per-environment node types

The schema for each underlying runtime graph follows the same
patterns, but each environment has its own node types:

### K8s runtime graph

```
Deployment, StatefulSet, DaemonSet, Pod, Service, Ingress,
ConfigMap (names only, content scrubbed), HPA, Job, CronJob,
Namespace, Image, Episode
```

### AWS runtime graph

```
LambdaFunction, EC2Instance, IAMRole, IAMPolicy, S3Bucket,
DynamoDBTable, RDSInstance, ECSService, ECSTaskDefinition,
APIGateway, CloudFrontDistribution, SQSQueue, SNSTopic,
EventBridgeRule, StepFunction, ECRRepository, Image, Episode
```

### GCP runtime graph

```
CloudRunService, CloudFunction, GKEDeployment, PubSubTopic,
BigQueryDataset, IAMBinding, CloudSQLInstance, StorageBucket,
Image, Episode
```

### Azure runtime graph

```
AppService, FunctionApp, AKSWorkload, StorageAccount,
RoleAssignment, SQLDatabase, CosmosDBAccount, Image, Episode
```

### Stable node ID format (cross-environment join)

Per `docs/federated-graph-architecture.md`:

```
{label}:{scope}:{path}:{name}
```

Examples:
- `Deployment:k8s/acme/prod-eu:payments-api`
- `LambdaFunction:aws/123456789012/us-east-1:payment-processor`
- `CloudRunService:gcp/acme-ml-prod:ml-inference-api`
- `AppService:azure/sub-abc123:payment-webhook`
- `Image:registry.acme.io/payments:v2.4.1`
- `Episode:backend:commit:def456`

The federation server joins on these stable IDs without needing the
underlying graphs to know about each other.

---

## Deployment architecture (Mazkir's side)

Per the `mazkir_python_rust_split.md` and the unit-economics work:

### Stage 1: MVP (months 0-12, <100 customers)

**One $60/month Hetzner dedicated server runs everything:**
- nginx (TLS termination)
- gunicorn × 4 Python workers (webhook receiver + Federation server +
  MCP server)
- FalkorDB (multi-tenant via graph names per scope)
- Daily backup cron to S3-compatible storage

Per-customer infrastructure cost: ~$0.60/month at 100 customers.

### Stage 2: Cloud tier scale-up (months 12-24, 100-1000 customers)

Either stay on Hetzner ($200/mo for primary + replica) or migrate
to AWS:

```
ALB + 2× ECS Fargate t4g.medium tasks (webhook + federation)
1× r6g.large EC2 with EBS gp3 (FalkorDB)
S3 for cold archive
Total: ~$215/month → $0.22/customer
```

### Stage 3: Real scale (year 2-3, 1000-10000 customers)

API Gateway + Lambda for ingest (now scale-to-zero matters), sharded
FalkorDB instances behind a router. ~$0.20/customer/month.

### Stage 4: Enterprise (year 2+)

Customers run the entire stack (federation server + per-scope
FalkorDB + MCP endpoint + webhook receiver) inside their own VPC.
Same code, different topology. Mazkir's cost: $0.

**Two install paths for Stage 4:**

**Path A — AWS / GCP / Azure Marketplace (the default):**

Customer goes to AWS Marketplace, searches "Mazkir," subscribes,
clicks "Deploy via CloudFormation." Within 10-15 minutes, AWS has
deployed the entire Mazkir stack into the customer's existing EKS
cluster. **Billing flows through AWS** — the customer's existing
AWS Enterprise Discount Program commit covers Mazkir, no new
vendor onboarding needed. Sales cycle drops from 6-9 months to
2-4 weeks.

GCP Marketplace and Azure Marketplace work identically with the
same one-click deploy pattern.

This is the **strategic GTM unlock** for Phase 4 because:

1. **Procurement bypass.** F500 customers buy through their existing
   AWS/GCP/Azure relationship, no new MSA required.
2. **AWS commit drawdown.** Marketplace purchases count against the
   customer's existing cloud commit, so it's "free money" from
   their finance team's perspective.
3. **AWS-co-marketed distribution.** Mazkir gets featured in the
   Marketplace catalog, AWS account managers recommend us, AWS
   reps get bonus credit for Marketplace deals.
4. **Trust signal.** Being in Marketplace is itself a credibility
   marker — AWS has reviewed the listing.
5. **Self-service deploy.** Eliminates the SE-led 1-2 day install
   process. Customer deploys themselves in 30 minutes.

The Marketplace listings unlock acquisition optionality too —
acquirers value Marketplace presence at a 1.5-2× multiplier on the
Marketplace-driven ARR because it proves enterprise sales motion
and is portable to a new brand.

**Effort to ship:** ~3-4 weeks for the first AWS Marketplace
listing (CloudFormation template + Marketplace Metering API
integration + listing approval). ~2 weeks each for GCP and Azure
on top. Total: ~7-8 weeks of engineering for all three marketplaces.

**Path B — SE-led Helm install (the fallback):**

For customers in regulated industries that can't use Marketplace
(some FedRAMP / DoD / EU sovereign-cloud requirements), a Mazkir
SE walks them through a manual Helm chart install:

```
$ helm repo add mazkir https://charts.mazkir.io
$ helm install mazkir mazkir/mazkir-enterprise \
    --namespace mazkir-system --create-namespace \
    --values values.customer.yaml
```

Same software, just deployed manually instead of via Marketplace
automation. ~1-2 days of SE time per install.

---

## API versioning is a hard requirement

Per `docs/api-versioning.md` (companion doc), every MCP tool call
accepts an `api_version` parameter and Mazkir maintains backwards
compatibility for at least 4 quarterly releases (~1 year).

The MCP tool surface across the live infrastructure layer is the
*platform contract*. Customers who build automations on top of
`cluster_state`, `diff_impact`, or `function_xray` need to pin
their version and upgrade on their schedule, not ours. Without
versioning, every Mazkir update breaks every customer integration.

This is non-negotiable for the live infra layer because:
1. Cloud provider APIs evolve constantly (AWS adds resource types
   every month)
2. Mazkir has to evolve its own schema to keep up
3. Customers have to be able to depend on us across version
   transitions
4. The standards-capture moat from `docs/competitive-defense.md`
   requires a stable API contract

---

## Implementation order

**Phase 3 (months 9-18) per `docs/roadmap-to-1b.md`** —
re-prioritized after the cloud-as-vertical insight:

1. **Months 9-11: AWS event ingestion** (CloudFormation template
   generator, webhook receiver, AWS event parser, Lambda + EC2 +
   IAM + S3 + RDS resource types — top 5 services that cover ~80%
   of customer use cases)
2. **Months 11-13: K8s operator** (the Helm chart, the watch API
   integration, the cluster-scoped token model)
3. **Months 13-14: GCP integration** (same pattern as AWS, different
   provider)
4. **Months 14-15: Azure integration** (same pattern, different
   provider)
5. **Months 15-16: Federation server** that joins source + K8s +
   AWS + GCP + Azure into one MCP endpoint
6. **Months 16-18: Liveness fingerprints** from OpenTelemetry trace
   ingestion (the killer Layer 4 feature per
   `docs/runtime-layer-design-principles.md`)

**Why AWS first instead of K8s first:** AWS is a bigger market,
needs no in-account agent (lower friction onboarding), and the
event-stream pattern is the same template that GCP and Azure can
reuse. K8s is harder (in-cluster operator, RBAC review at every
customer) and addresses a smaller customer base. Ship the easy big
market first.

---

## How this document gets used

- **For onboarding:** the customer story in
  `docs/onboarding-story.md` walks through this layer step by step.
  This doc is the architectural backstory.
- **For product decisions:** when a new resource type is proposed
  (e.g., "should we capture AWS Step Functions?"), check whether
  it serves one of the killer queries above. If not, defer.
- **For competitive defense:** this layer is **the moat** per
  `docs/competitive-defense.md`. Cursor / Glean / Sourcegraph won't
  build this. The runtime layer is uncopyable for at least 18
  months because the competitors who could build it have other
  priorities.
- **For roadmap conversations:** the implementation order above is
  the explicit sequence. Don't propose K8s first. Don't propose
  starting with GCP. AWS event-stream pattern, then K8s, then GCP/
  Azure as fast-followers.
