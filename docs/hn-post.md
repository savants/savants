# HN Post Draft

## Title (must be < 80 chars)

Show HN: Savants – found root cause of 15 crashing K8s pods in 45 seconds

## Body

I built a CLI tool that connects your code, Kubernetes cluster, host metrics, and logs into one graph — then tells you what's wrong.

Last week my homelab cluster had 15 pods in CrashLoopBackOff. Instead of grepping through 7.5 million log lines across 94 pods, I ran:

    savants up

45 seconds later it told me: all 15 failures traced back to one DNS misconfiguration in CoreDNS. One ConfigMap edit fixed everything.

**How it works:**

Savants builds a knowledge graph with six layers:

1. **Code** — AST-level call graphs, function signatures, config keys (tree-sitter)
2. **K8s state** — real-time watch streams with 1.3s propagation (resourceVersion diffing)
3. **Log intelligence** — 3-tier pipeline: severity classifier → drain3 template extraction → graph nodes. 7.5M lines → 78 patterns
4. **Host** — CPU, memory, systemd units, Docker containers, kernel events, journal errors
5. **History** — git commits, co-change analysis, who-touched-what
6. **Cross-layer edges** — MENTIONS (log references a ConfigMap), CAUSED_BY (temporal correlation), READS (pod reads a config)

The cross-layer queries are the killer feature. No other tool can answer: "this pod crashed because this configmap was edited, which is read by this code path, changed in this commit."

**What it's not:**

- Not a Datadog replacement (no metrics storage, no dashboards)
- Not a log aggregator (logs are discarded after significance extraction)
- Not open source (closed source, but free forever for local use)

**Privacy:** No source code is stored — only metadata (function names, call relationships, config key names). ~450x compression ratio. Verifiable with a packet capture: zero outbound connections.

**Install:**

    curl -fsSL savants.sh | sh
    savants up

Works as an MCP server for Claude Code and Cursor (26 tools):

    savants mcp install

Then ask your AI: "What's wrong with my cluster?"

Site: https://savants.dev
Docs: https://savants.dev/docs/getting-started

---

## Notes for Miguel

**When to post:** Tuesday or Wednesday, 9-10am ET (peak HN traffic)

**First comment to post immediately after submission (prevents others from setting the narrative):**

Author here. A few things I learned building this:

1. K8s `resourceVersion` is the cleanest way to do diff-based ingestion. No generation counters needed — every object carries its own change indicator from etcd.

2. drain3 (the log template extraction library) is shockingly good at compressing millions of log lines into a few hundred patterns. The key insight: you don't need to store logs, you need to store the *templates*.

3. The MCP protocol made AI integration trivial. 26 tools, all queryable from Claude Code or Cursor. The graph is the knowledge base; the LLM is the interface.

4. The hardest part wasn't any single layer — it was getting the cross-layer edges right. Knowing that a LogEvent MENTIONS a ConfigMap that is READ by a Pod that runs code from a Function that was CHANGED in a Commit — that chain is what produces the "45 second root cause" result. Each layer alone is commodity; the connections are the product.

Happy to answer questions about the architecture, the drain3 pipeline, or the K8s watch implementation.
