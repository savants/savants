# Savants Accuracy Benchmark - System Design

## Why This Is The Most Important Code In The Company

If the benchmark says 95%, customers trust the product.
If the benchmark says 85%, we know where to improve before shipping.
If the benchmark regresses from 95% to 90% after a code change, CI blocks the merge.

This test suite IS the product quality. Everything else is features.

## Design Principles

1. **Ground truth is sacred.** Every test case has a CONFIRMED root cause, not a guess. The root cause was verified by a human who deployed a fix and watched the error stop.

2. **The suite runs in CI in under 15 minutes.** If it takes an hour, nobody runs it. Fast feedback = fast iteration.

3. **No test case depends on external services.** No live Sentry API, no live Slack, no live Jira. Everything is self-contained snapshots. The test runs on a laptop with no internet.

4. **The holdout set is locked.** Once created, the holdout cases are never used for development. They are run exactly once before a release. If someone peeks at them and tunes the code, the entire holdout is contaminated and must be replaced.

5. **Every wrong answer becomes a test case.** When a customer reports a wrong diagnosis, that error + correct root cause is added to the suite. The suite grows monotonically.

## Architecture

```
tests/
  benchmark/
    README.md                    # This file
    run.sh                       # Entry point: runs all benchmarks
    score.py                     # Scoring engine

    cases/
      postmortems/               # From public incident reports
        pm-001.json
        pm-002.json
        ...
      github-bugs/               # From open source bug-fix pairs
        gh-001.json
        gh-002.json
        ...
      chaos/                     # From K8s chaos injection
        chaos-001.json
        chaos-002.json
        ...
      production/                # From real customer incidents
        prod-001.json
        prod-002.json
        ...
      regression/                # From reported wrong answers
        reg-001.json
        reg-002.json
        ...

    holdout/                     # NEVER look at these during development
      holdout-001.json           # Run only before release
      holdout-002.json
      ...

    fixtures/                    # Pre-built graph snapshots for each case
      pm-001.cypher              # Graph state at time of incident
      gh-001.cypher              # Graph state at time of bug
      ...

    chaos-harness/               # K8s chaos test automation
      manifests/                 # Broken K8s manifests to inject
      scenarios/                 # Scenario definitions
      runner.sh                  # Deploys, waits, diagnoses, scores

    harvest/                     # Scripts to collect new test cases
      github_bugs.py             # Harvests bug-fix pairs from GitHub
      postmortems.py             # Parses public postmortem blogs
      sentry_resolved.py         # Extracts resolved Sentry issues
```

## Test Case Format

Every test case is a self-contained JSON file:

```json
{
  "id": "pm-001",
  "source": "postmortem",
  "title": "Cloudflare DNS outage 2024-10-04",
  "category": "infrastructure",
  "subcategory": "dns",
  "difficulty": "hard",

  "error_signal": {
    "message": "SERVFAIL for all DNS queries, 502 errors across all services",
    "source": "monitoring",
    "timestamp": "2024-10-04T14:22:00Z"
  },

  "graph_fixture": "pm-001.cypher",

  "ground_truth": {
    "root_cause_file": "infrastructure/coredns/configmap.yaml",
    "root_cause_function": null,
    "root_cause_description": "CoreDNS forward directive pointing to unreachable upstream after network change",
    "root_cause_category": "infrastructure",
    "fix_description": "Changed forward . 10.0.0.1 to forward . 1.1.1.1 8.8.8.8",
    "fix_commit": "abc123",
    "fix_pr": "#456",
    "verified_by": "error_count_dropped_to_zero"
  },

  "expected_signals": {
    "must_mention": ["coredns", "forward", "upstream", "dns"],
    "must_not_mention": ["frontend", "application code"],
    "must_identify_category": "infrastructure",
    "must_trace_upstream": false
  },

  "metadata": {
    "added_date": "2026-04-15",
    "added_by": "benchmark-harvester",
    "public_reference": "https://blog.cloudflare.com/...",
    "confidence": "high"
  }
}
```

## Graph Fixtures

Each test case needs a graph state that represents what Savants would have seen at the time of the incident. This is a Cypher script that populates a fresh graph:

```cypher
// pm-001.cypher - Cloudflare DNS outage graph state
CREATE (:K8sPod {name: 'coredns-abc123', namespace: 'kube-system', status: 'Running'})
CREATE (:K8sConfigMap {name: 'coredns', namespace: 'kube-system', data_forward: '. 10.0.0.1'})
CREATE (:K8sEvent {type: 'Warning', reason: 'Unhealthy', message: 'Readiness probe failed', timestamp: 1696428120})
CREATE (:HostLogEvent {template_text: 'SERVFAIL resolving example.com', severity: 'ERROR', count: 15000})
CREATE (:SlackMessage {text: 'DNS is down across all services', channel_name: 'incidents', has_symptom: true, timestamp: 1696428120})
CREATE (:Commit {hash: 'def456', message: 'Update network config', author: 'ops-team', date: '2024-10-04T13:00:00Z'})

// Edges
MATCH (p:K8sPod {name: 'coredns-abc123'}), (cm:K8sConfigMap {name: 'coredns'}) CREATE (p)-[:READS]->(cm)
MATCH (c:Commit {hash: 'def456'}), (cm:K8sConfigMap {name: 'coredns'}) CREATE (c)-[:MODIFIED_FILE]->(cm)
MATCH (e:K8sEvent), (p:K8sPod {name: 'coredns-abc123'}) CREATE (p)-[:HAS_EVENT]->(e)
```

This fixture is loaded into a temporary graph before the diagnosis runs. After the test, the graph is wiped.

## Scoring Engine

The scoring engine is NOT a simple string match. It uses multiple signals:

```python
def score_diagnosis(diagnosis_output, ground_truth, expected_signals):
    score = 0
    max_score = 10
    details = []

    # 1. Root cause file identification (0-3 points)
    if ground_truth.root_cause_file in diagnosis_output:
        score += 3
        details.append("PASS: Correct root cause file")
    elif any(part in diagnosis_output for part in ground_truth.root_cause_file.split('/')):
        score += 1
        details.append("PARTIAL: Mentioned part of root cause path")
    else:
        details.append("FAIL: Root cause file not identified")

    # 2. Root cause function (0-2 points, skip if null)
    if ground_truth.root_cause_function:
        if ground_truth.root_cause_function in diagnosis_output:
            score += 2
            details.append("PASS: Correct root cause function")
        else:
            details.append("FAIL: Root cause function not identified")
    else:
        score += 2  # Full points if no function expected

    # 3. Category identification (0-2 points)
    if ground_truth.root_cause_category in diagnosis_output.lower():
        score += 2
        details.append("PASS: Correct category")

    # 4. Must-mention keywords (0-2 points)
    mentioned = sum(1 for kw in expected_signals.must_mention
                    if kw.lower() in diagnosis_output.lower())
    keyword_ratio = mentioned / len(expected_signals.must_mention)
    score += round(keyword_ratio * 2)

    # 5. Must-not-mention (0 or -1 points, penalty only)
    for bad_kw in expected_signals.must_not_mention:
        if bad_kw.lower() in diagnosis_output.lower():
            score -= 1
            details.append(f"PENALTY: Mentioned '{bad_kw}' (misleading)")

    # Normalize to 0-10
    score = max(0, min(10, score))

    # Classification
    if score >= 8:
        grade = "CORRECT"
    elif score >= 5:
        grade = "PARTIAL"
    elif score >= 3:
        grade = "DIRECTION"
    else:
        grade = "WRONG"

    return grade, score, details
```

Accuracy = count(CORRECT) / total cases

## K8s Chaos Test Scenarios

Each scenario is a self-contained test that:
1. Saves the current state of a namespace
2. Injects a known fault
3. Waits for Savants daemon to detect it (60 seconds max)
4. Runs diagnose-error
5. Scores the output against known ground truth
6. Restores the original state

### Scenario Definitions

```yaml
# chaos/scenarios/dns-failure.yaml
name: DNS Forward Misconfiguration
category: infrastructure
subcategory: dns
difficulty: medium

inject:
  type: configmap_edit
  namespace: kube-system
  resource: coredns
  field: Corefile
  original: "forward . /etc/resolv.conf"
  modified: "forward . 127.0.0.1"

wait_for:
  condition: pod_status
  namespace: default
  status: "CrashLoopBackOff|Error"
  timeout_seconds: 120

error_signal: "DNS resolution failing, pods unable to resolve service names"

ground_truth:
  root_cause_file: "kube-system/configmap/coredns"
  root_cause_description: "CoreDNS forward pointing to unreachable upstream 127.0.0.1"
  must_mention: ["coredns", "forward", "dns", "upstream"]
  category: "infrastructure"

restore:
  type: configmap_edit
  namespace: kube-system
  resource: coredns
  field: Corefile
  value: "forward . /etc/resolv.conf"
```

### Full Chaos Test Matrix (100 scenarios)

**Infrastructure (30 scenarios):**
- DNS: forward misconfiguration, missing DNS pods, DNS cache poisoning
- Network: NetworkPolicy deny-all, service port mismatch, ingress misconfiguration
- Storage: PVC full (50%, 80%, 95%, 100%), PVC deleted, storage class missing
- Certificates: TLS expired, TLS wrong domain, TLS self-signed rejected
- Node: node not-ready, node disk pressure, node memory pressure

**Application (30 scenarios):**
- CrashLoop: missing env var, missing secret, missing configmap, bad image tag
- OOM: memory limit too low (10Mi, 50Mi, 100Mi vs actual usage)
- Readiness: probe path wrong, probe port wrong, probe timeout too short
- Liveness: probe failing, restart loop, graceful shutdown timeout
- Scaling: HPA max too low, HPA metric missing, resource requests wrong

**Configuration (20 scenarios):**
- ConfigMap: wrong value, missing key, encoding error, YAML syntax error
- Secret: expired credential, wrong credential, missing secret
- RBAC: service account missing role, cluster role too restrictive
- Deployment: wrong image, wrong command, wrong args, wrong port

**Cross-layer (20 scenarios):**
- Code deploy causes pod crash (bad config in commit + K8s restart)
- Secret rotation breaks multiple services
- Config change cascades to dependent services
- Rolling update partially fails (mixed versions)

### Variation Generation

Each scenario has parameters that can be varied:
- Namespace (default, monitoring, app-specific)
- Resource names
- Timing (inject at different points in the monitoring cycle)
- Severity (gradual degradation vs instant failure)

30 base scenarios x 3-4 variations each = 100+ unique test cases

## GitHub Bug-Fix Harvester

Automated script that:
1. Searches GitHub for repos with >1000 stars, language: TypeScript
2. Finds closed issues labeled "bug" with a linked merged PR
3. For each pair: extracts issue body (error signal), PR diff (root cause)
4. Indexes the repo at the commit BEFORE the fix (point-in-time snapshot)
5. Packages as a test case with graph fixture

```python
# harvest/github_bugs.py (pseudocode)

TARGET_REPOS = [
    "vercel/next.js",
    "prisma/prisma",
    "nestjs/nest",
    "trpc/trpc",
    "vitejs/vite",
    "remix-run/remix",
    "honojs/hono",
    "drizzle-team/drizzle-orm",
    "t3-oss/create-t3-app",
    "calcom/cal.com",
]

for repo in TARGET_REPOS:
    issues = github.search_issues(repo, labels=["bug"], state="closed", linked_pr=True)
    for issue in issues:
        pr = issue.linked_pull_request
        if pr.state != "merged":
            continue

        # Extract root cause from PR
        diff = pr.diff()
        changed_files = diff.files
        if len(changed_files) > 10:
            continue  # Too complex for automated labeling

        # Build test case
        case = {
            "id": f"gh-{repo.split('/')[1]}-{issue.number}",
            "error_signal": {"message": issue.title + " " + issue.body[:500]},
            "ground_truth": {
                "root_cause_file": changed_files[0].path,
                "fix_commit": pr.merge_commit,
                "fix_pr": f"#{pr.number}"
            }
        }

        # Generate graph fixture by indexing repo at pre-fix commit
        pre_fix_commit = pr.base_commit
        fixture = generate_fixture(repo, pre_fix_commit)

        save_test_case(case, fixture)
```

Expected yield: 20-30 high-quality cases per repo = 200-300 total

## Postmortem Harvester

Parse structured postmortem blogs to extract:
- Timeline (what happened when)
- Root cause (the confirmed fix)
- Affected systems (for graph fixture construction)

Sources with structured formats:
- https://github.com/danluu/post-mortems (300+ links)
- https://github.com/upgundecha/howtheysre (company SRE practices)
- PagerDuty incident reports (structured JSON format)

Expected yield: 100-150 cases from public sources

## Continuous Integration

```yaml
# .github/workflows/accuracy.yml
name: Accuracy Benchmark

on:
  pull_request:
    paths:
      - 'savants-cli/src/mcp/server.rs'  # Diagnosis logic
      - 'savants-cli/src/code_index.rs'   # Indexer
      - 'savants-cli/src/knowledge.rs'    # Knowledge engine

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build Savants
        run: cd savants-cli && cargo build --release

      - name: Start graph engine
        run: savants config init && sleep 3

      - name: Run development benchmark
        run: |
          ./tests/benchmark/run.sh --set development
          # Fails if accuracy drops below 93% (2% buffer below 95% target)

      - name: Upload results
        uses: actions/upload-artifact@v4
        with:
          name: benchmark-results
          path: tests/benchmark/results/

  # Only runs on release tags, not on every PR
  holdout:
    if: startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-latest
    steps:
      - name: Run holdout benchmark
        run: |
          ./tests/benchmark/run.sh --set holdout
          # Fails if accuracy below 95%

      - name: Publish accuracy badge
        run: |
          # Updates the accuracy badge on the README
          echo "ACCURACY=$(cat tests/benchmark/results/holdout-accuracy.txt)" >> $GITHUB_ENV
```

## Anti-Gaming Measures

The benchmark must be resistant to gaming (intentionally or accidentally):

1. **Holdout contamination detection:** Hash the holdout directory. If any file changes, CI fails with "holdout contaminated, regenerate."

2. **Overfitting detection:** Track per-case accuracy over time. If a previously-failing case suddenly passes after a code change that doesn't logically relate to it, flag for review.

3. **Category balance enforcement:** If accuracy on "infrastructure" cases is 99% but "cross-layer" cases is 80%, report both numbers. Don't let easy categories inflate the aggregate.

4. **Adversarial cases:** Include cases designed to trick the system:
   - Error message that mentions function A but the real cause is function B
   - Frontend crash whose root cause is a backend config change in a different repo
   - Error that looks like a code bug but is actually a network issue
   - Multiple simultaneous errors where only one is the root cause

5. **Freshness requirement:** At least 10% of the development set must be replaced every quarter with new cases from new repos. This prevents the system from memorizing a fixed dataset.

## Success Criteria for Launch

| Metric | Target | Minimum | Measured on |
|--------|--------|---------|-------------|
| Strict accuracy (CORRECT) | 95% | 93% | Holdout set (80+ cases) |
| Useful accuracy (CORRECT + PARTIAL) | 98% | 96% | Holdout set |
| Category accuracy | 99% | 97% | Holdout set |
| Infrastructure scenarios | 95% | 90% | Chaos test suite |
| Cross-layer scenarios | 90% | 85% | Chaos test suite |
| p99 diagnosis time | < 5 seconds | < 10 seconds | All cases |
| False positive rate | < 5% | < 8% | Must-not-mention violations |

All targets must be met simultaneously. If strict accuracy is 96% but cross-layer is 82%, the product does not ship.
