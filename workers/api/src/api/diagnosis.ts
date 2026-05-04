/**
 * Standalone diagnosis engine.
 * Uses whatever data sources are available:
 *   1. Error message + stack trace (always available)
 *   2. Sentry event details (if integration configured)
 *   3. Code graph (if graph proxy available)
 *   4. K8s state (if cluster connected)
 *
 * Gracefully degrades - uses what it has.
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

interface DiagnosisInput {
  error_message: string;
  file_path?: string;
  sentry_event_id?: string;
  sentry_project?: string;
}

interface DiagnosisResult {
  root_cause: string;
  file: string | null;
  line: number | null;
  function: string | null;
  call_chain: string[];
  suggested_fix: string;
  severity: "critical" | "error" | "warning";
  confidence: number;
  sources_used: string[];
  sentry_context?: {
    breadcrumbs: string[];
    tags: Record<string, string>;
    user: string | null;
  };
  graph_context?: {
    callers: string[];
    importers: string[];
    blast_radius: number;
  };
  tickets?: Array<{
    id: string;
    title: string;
    status?: string;
    assignee?: string;
    url?: string;
    priority?: string | number;
  }>;
}

export async function diagnoseError(
  env: Env,
  orgId: string,
  input: DiagnosisInput
): Promise<DiagnosisResult> {
  const sources: string[] = [];
  let sentryData: any = null;
  let graphData: any = null;

  // ── Source 1: Parse the error message (always available) ──
  sources.push("error_message");
  const parsed = parseErrorMessage(input.error_message);

  // ── Source 1b: Detect if this is an infra problem (not code) ──
  const infraKeywords = ["pod", "node", "cluster", "dns", "tunnel", "timeout", "connection",
    "interface", "drops", "tailscale", "cloudflared", "coredns", "wifi", "ethernet",
    "memory", "disk", "cpu", "load", "systemd", "service", "k8s", "kubernetes",
    "restarts", "crashloop", "oom", "evict", "unhealthy", "probe", "certificate"];
  const lowerMsg = input.error_message.toLowerCase();
  const isInfra = infraKeywords.filter(k => lowerMsg.includes(k)).length >= 2;

  let infraData: any = null;

  if (isInfra) {
    // Query agent for current host + k8s state
    try {
      // Get recent agent findings from audit log
      const findings = await env.DB.prepare(`
        SELECT metadata, created_at FROM audit_log
        WHERE org_id = ?1 AND action = 'agent.notify'
        ORDER BY created_at DESC LIMIT 20
      `).bind(orgId).all();

      // Get online agents
      const agents = await env.DB.prepare(
        "SELECT name, hostname, os, capabilities, last_heartbeat, status FROM agents WHERE org_id = ?1 AND status = 'online'"
      ).bind(orgId).all();

      // Get recent k8s events from graph
      const k8sEvents = await env.DB.prepare(`
        SELECT type, title, severity, occurred_at FROM graph_events
        WHERE project_id IN (SELECT id FROM projects WHERE org_id = ?1)
          AND occurred_at > ?2
        ORDER BY occurred_at DESC LIMIT 20
      `).bind(orgId, Math.floor(Date.now() / 1000) - 7200).all();

      // Get pods with high restarts
      const unhealthyPods = await env.DB.prepare(`
        SELECT name, metadata FROM graph_nodes
        WHERE project_id IN (SELECT id FROM projects WHERE org_id = ?1)
          AND type = 'k8s_pod'
          AND CAST(json_extract(metadata, '$.restarts') AS INTEGER) > 10
        ORDER BY CAST(json_extract(metadata, '$.restarts') AS INTEGER) DESC
        LIMIT 10
      `).bind(orgId).all();

      const agentFindings = (findings.results as any[]).map(r => {
        try { return JSON.parse(r.metadata || "{}"); } catch { return {}; }
      }).filter(f => f.title);

      if (agentFindings.length > 0 || (agents.results as any[]).length > 0 || (k8sEvents.results as any[]).length > 0) {
        sources.push("agent_findings");
        infraData = {
          agents: (agents.results as any[]).map(a => ({
            name: a.name, os: a.os, status: a.status,
            capabilities: JSON.parse(a.capabilities || "[]"),
          })),
          recent_findings: agentFindings.slice(0, 10).map(f => ({
            severity: f.severity, category: f.category, title: f.title,
            message: f.message,
          })),
          k8s_events: (k8sEvents.results as any[]).map(e => ({
            type: e.type, title: e.title, severity: e.severity,
          })),
          unhealthy_pods: (unhealthyPods.results as any[]).map(p => {
            let meta: any = {};
            try { meta = JSON.parse(p.metadata || "{}"); } catch {}
            return { name: p.name, restarts: meta.restarts, status: meta.status };
          }),
        };
      }
    } catch {
      // Agent data unavailable
    }
  }

  // ── Source 2: Sentry enrichment (if integration exists) ──
  try {
    const sentryIntegration = await getIntegration(env.DB, orgId, "sentry");
    if (sentryIntegration) {
      const creds = JSON.parse(sentryIntegration.credentials);
      const config = JSON.parse(sentryIntegration.config);

      if (creds.auth_token) {
        // Try to fetch event details if we have event ID + project
        const project = input.sentry_project || config.project_slugs?.[0];
        if (input.sentry_event_id && project) {
          sentryData = await fetchSentryEvent(
            creds.auth_token,
            config.org_slug,
            project,
            input.sentry_event_id
          );
          if (sentryData) sources.push("sentry_event");
        }

        // If no specific event, search recent events matching the error
        if (!sentryData) {
          sentryData = await searchSentryEvents(
            creds.auth_token,
            config.org_slug,
            input.error_message
          );
          if (sentryData) sources.push("sentry_search");
        }
      }
    }
  } catch {
    // Sentry unavailable - continue without it
  }

  // ── Source 2b: Linear ticket search ──
  let ticketData: any = null;
  try {
    const linearIntegration = await getIntegration(env.DB, orgId, "linear");
    if (linearIntegration) {
      const creds = JSON.parse(linearIntegration.credentials || "{}");
      if (creds.api_key) {
        const searchTerms = input.error_message.slice(0, 80);
        const res = await fetch("https://api.linear.app/graphql", {
          method: "POST",
          headers: { Authorization: creds.api_key, "Content-Type": "application/json" },
          body: JSON.stringify({
            query: `query { issueSearch(query: "${searchTerms.replace(/"/g, '\\"')}", first: 3) { nodes { id identifier title state { name } assignee { name } url priority } } }`,
          }),
          signal: AbortSignal.timeout(5000),
        });
        if (res.ok) {
          const data = await res.json<any>();
          const issues = data?.data?.issueSearch?.nodes || [];
          if (issues.length > 0) {
            sources.push("linear");
            ticketData = issues.map((i: any) => ({
              id: i.identifier,
              title: i.title,
              status: i.state?.name,
              assignee: i.assignee?.name,
              url: i.url,
              priority: i.priority,
            }));
          }
        }
      }
    }
  } catch {}

  // ── Source 2c: Jira ticket search ──
  try {
    const jiraIntegration = await getIntegration(env.DB, orgId, "jira");
    if (jiraIntegration && !ticketData) {
      const creds = JSON.parse(jiraIntegration.credentials || "{}");
      const config = JSON.parse(jiraIntegration.config || "{}");
      if (creds.email && creds.api_token && config.domain) {
        const auth64 = btoa(`${creds.email}:${creds.api_token}`);
        const jql = encodeURIComponent(`text ~ "${input.error_message.slice(0, 60).replace(/"/g, '\\"')}" ORDER BY updated DESC`);
        const res = await fetch(
          `https://${config.domain}/rest/api/3/search?jql=${jql}&maxResults=3&fields=summary,status,assignee,priority`,
          {
            headers: { Authorization: `Basic ${auth64}` },
            signal: AbortSignal.timeout(5000),
          }
        );
        if (res.ok) {
          const data = await res.json<any>();
          if (data.issues?.length > 0) {
            sources.push("jira");
            ticketData = data.issues.map((i: any) => ({
              id: i.key,
              title: i.fields?.summary,
              status: i.fields?.status?.name,
              assignee: i.fields?.assignee?.displayName,
              priority: i.fields?.priority?.name,
            }));
          }
        }
      }
    }
  } catch {}

  // ── Source 3: Code graph from D1 (if function/file found) ──
  if (parsed.function || parsed.file) {
    try {
      // Find the function node in the graph
      let nodeQuery = "SELECT * FROM graph_nodes WHERE 1=1";
      const nodeParams: unknown[] = [];

      if (parsed.function) {
        nodeQuery += ` AND name = ?${nodeParams.length + 1}`;
        nodeParams.push(parsed.function);
      }
      if (parsed.file) {
        // Match file path (could be full path like /app/server/... or relative)
        const fileName = parsed.file.split("/").pop() || parsed.file;
        nodeQuery += ` AND file_path LIKE ?${nodeParams.length + 1}`;
        nodeParams.push(`%${fileName}`);
      }
      nodeQuery += " LIMIT 1";

      const node = await env.DB.prepare(nodeQuery).bind(...nodeParams).first<{
        id: string; name: string; type: string; file_path: string; line_start: number; project_id: string;
      }>();

      if (node) {
        sources.push("code_graph");

        // Get callers (who calls this function?)
        const callers = await env.DB.prepare(`
          SELECT n.name, n.type, n.file_path, n.line_start
          FROM graph_edges e
          JOIN graph_nodes n ON n.id = e.source_node
          WHERE e.target_node = ?1 AND e.type = 'CALLS'
          ORDER BY n.name LIMIT 10
        `).bind(node.id).all();

        // Get recent events on this node
        const events = await env.DB.prepare(`
          SELECT type, title, severity, occurred_at
          FROM graph_events
          WHERE node_id = ?1
          ORDER BY occurred_at DESC LIMIT 5
        `).bind(node.id).all();

        // Get what this function calls (downstream)
        const callees = await env.DB.prepare(`
          SELECT n.name, n.type, n.file_path
          FROM graph_edges e
          JOIN graph_nodes n ON n.id = e.target_node
          WHERE e.source_node = ?1 AND e.type = 'CALLS'
          ORDER BY n.name LIMIT 10
        `).bind(node.id).all();

        graphData = {
          node: {
            name: node.name,
            type: node.type,
            file: node.file_path,
            line: node.line_start,
          },
          callers: (callers.results as unknown as any[]).map((r) =>
            `${r.name}() at ${r.file_path || "?"}:${r.line_start || "?"}`
          ),
          callees: (callees.results as unknown as any[]).map((r) =>
            `${r.name}() at ${r.file_path || "?"}`
          ),
          recent_events: (events.results as unknown as any[]).map((r) =>
            `[${r.severity}] ${r.title}`
          ),
          blast_radius: (callers.results?.length || 0) + (callees.results?.length || 0),
        };
      }
    } catch {
      // Graph query failed - continue without it
    }
  }

  // ── Build diagnosis from all available sources ──
  return buildDiagnosis(parsed, sentryData, graphData, ticketData, infraData, isInfra, sources);
}

function parseErrorMessage(msg: string): {
  type: string;
  message: string;
  file: string | null;
  line: number | null;
  function: string | null;
  call_chain: string[];
} {
  const lines = msg.split("\n").map((l) => l.trim());
  let type = "Error";
  let message = msg;
  let file: string | null = null;
  let line: number | null = null;
  let fn: string | null = null;
  const call_chain: string[] = [];

  // Extract error type: "TypeName: message"
  const typeMatch = msg.match(/^(\w+Error|\w+Exception):\s*(.*)/);
  if (typeMatch) {
    type = typeMatch[1];
    message = typeMatch[2];
  }

  // Extract file:line from patterns like "at file.ts:123" or "in file.ts:123"
  const fileMatch = msg.match(/(?:at|in)\s+([^\s:]+\.[a-z]+):(\d+)/i);
  if (fileMatch) {
    file = fileMatch[1];
    line = parseInt(fileMatch[2]);
  }

  // Extract function name from "in functionName()" or "at functionName"
  const fnMatch = msg.match(/(?:at|in)\s+(\w+)\(\)/);
  if (fnMatch) {
    fn = fnMatch[1];
  }

  // Build call chain from "at X -> at Y -> at Z" or stack trace lines
  const stackLines = lines.filter(
    (l) => l.match(/^\s*(at\s+|->|→)/) || l.match(/\w+\(\)/)
  );
  for (const sl of stackLines) {
    const fnName = sl.match(/(?:at\s+)?(\w+(?:\.\w+)*)\s*\(/)?.[1];
    if (fnName) call_chain.push(fnName);
  }

  // Also extract from "X() -> Y() -> Z()" patterns
  const chainMatch = msg.match(/(\w+\(\)(?:\s*(?:->|→|at)\s*\w+\(\))+)/);
  if (chainMatch && call_chain.length === 0) {
    const fns = chainMatch[0].match(/\w+(?=\(\))/g);
    if (fns) call_chain.push(...fns);
  }

  return { type, message, file, line, function: fn, call_chain };
}

async function fetchSentryEvent(
  token: string,
  org: string,
  project: string,
  eventId: string
): Promise<any | null> {
  try {
    const res = await fetch(
      `https://sentry.io/api/0/projects/${org}/${project}/events/${eventId}/`,
      {
        headers: { Authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(5000),
      }
    );
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

async function searchSentryEvents(
  token: string,
  org: string,
  errorMessage: string
): Promise<any | null> {
  const headers = { Authorization: `Bearer ${token}`, "Content-Type": "application/json" };

  // Step 1: Search issues by error message (this is what Sentry MCP does)
  try {
    // Use the error title/type as search - Sentry matches on issue title
    // Too many terms = zero results. Extract the most distinctive part.
    let searchTerms = "";
    // Try to get the error type (e.g. "LLMParseError", "AxiosError", "TypeError")
    const typeMatch = errorMessage.match(/^(\w+Error|\w+Exception|Error)/);
    if (typeMatch) {
      searchTerms = typeMatch[1];
    }
    // If the error has a bracketed context like [ATS Push], use that
    const bracketMatch = errorMessage.match(/\[([^\]]+)\]/);
    if (bracketMatch) {
      searchTerms = searchTerms ? `${searchTerms} ${bracketMatch[1]}` : bracketMatch[1];
    }
    // Fallback: first 3 words over 4 chars
    if (!searchTerms) {
      const stopWords = new Set(["with", "status", "code", "error", "failed", "from", "that", "this"]);
      searchTerms = errorMessage
        .replace(/['":\[\]()]/g, " ")
        .split(/\s+/)
        .filter(w => w.length > 4 && !stopWords.has(w.toLowerCase()))
        .slice(0, 2)
        .join(" ");
    }

    const query = encodeURIComponent(`is:unresolved ${searchTerms}`);
    const issuesRes = await fetch(
      `https://sentry.io/api/0/organizations/${org}/issues/?query=${query}&per_page=5&sort=date`,
      { headers, signal: AbortSignal.timeout(8000) }
    );

    if (issuesRes.ok) {
      const issues = (await issuesRes.json()) as any[];
      if (issues.length > 0) {
        const issue = issues[0];

        // Step 2: Get the latest event for this issue (has stack trace, breadcrumbs)
        try {
          const eventRes = await fetch(
            `https://sentry.io/api/0/organizations/${org}/issues/${issue.id}/events/latest/`,
            { headers, signal: AbortSignal.timeout(5000) }
          );

          if (eventRes.ok) {
            const event = (await eventRes.json()) as any;
            return {
              issue_id: issue.id,
              title: issue.title,
              culprit: issue.culprit,
              first_seen: issue.firstSeen,
              last_seen: issue.lastSeen,
              count: issue.count,
              level: issue.level,
              // From the event
              event_id: event.eventID,
              tags: event.tags?.map((t: any) => `${t.key}=${t.value}`) || [],
              breadcrumbs: event.entries
                ?.find((e: any) => e.type === "breadcrumbs")
                ?.data?.values?.slice(-10)
                ?.map((b: any) => `[${b.category}] ${b.message || b.data?.url || ""}`)
                || [],
              stack_trace: event.entries
                ?.find((e: any) => e.type === "exception")
                ?.data?.values?.[0]?.stacktrace?.frames?.slice(-5)
                ?.map((f: any) => `${f.filename}:${f.lineNo} in ${f.function}`)
                || [],
              platform: event.platform,
              release: event.release?.version || event.tags?.find((t: any) => t.key === "release")?.value,
            };
          }
        } catch {
          // Return issue data without event details
          return {
            issue_id: issue.id,
            title: issue.title,
            culprit: issue.culprit,
            count: issue.count,
            last_seen: issue.lastSeen,
          };
        }
      }
    }
  } catch {
    // Search failed
  }

  // Fallback: try discover events endpoint
  try {
    const query = encodeURIComponent(errorMessage.slice(0, 100));
    const res = await fetch(
      `https://sentry.io/api/0/organizations/${org}/events/?query=${query}&per_page=1&full=true`,
      { headers, signal: AbortSignal.timeout(5000) }
    );
    if (!res.ok) return null;
    const events = (await res.json()) as any[];
    return events?.[0] ?? null;
  } catch {
    return null;
  }
}

function buildDiagnosis(
  parsed: ReturnType<typeof parseErrorMessage>,
  sentryData: any | null,
  graphData: any | null,
  ticketData: any | null,
  infraData: any | null,
  isInfra: boolean,
  sources: string[]
): DiagnosisResult {
  let rootCause = "";
  let file = parsed.file;
  let line = parsed.line;
  let fn = parsed.function;
  let callChain = parsed.call_chain;
  let suggestedFix = "";
  let severity: "critical" | "error" | "warning" = "error";
  let confidence = 0.5;
  let sentryContext: DiagnosisResult["sentry_context"] = undefined;
  let graphContext: DiagnosisResult["graph_context"] = undefined;

  // ── Enrich from Sentry ──
  if (sentryData) {
    confidence += 0.2;

    // Extract better stack trace
    const entries = sentryData.entries || [];
    for (const entry of entries) {
      if (entry.type === "exception") {
        const values = entry.data?.values || [];
        for (const exc of values) {
          const frames = exc.stacktrace?.frames || [];
          const appFrames = frames.filter((f: any) => f.inApp);
          if (appFrames.length > 0) {
            const topFrame = appFrames[appFrames.length - 1];
            file = topFrame.filename || file;
            line = topFrame.lineNo || line;
            fn = topFrame.function || fn;
            callChain = appFrames.map(
              (f: any) => `${f.function || "?"}() at ${f.filename || "?"}:${f.lineNo || "?"}`
            );
          }
        }
      }

      if (entry.type === "breadcrumbs") {
        const crumbs = entry.data?.values || [];
        sentryContext = {
          breadcrumbs: crumbs.slice(-5).map(
            (b: any) => `[${b.category || "?"}] ${b.message || JSON.stringify(b.data || {})}`
          ),
          tags: {},
          user: null,
        };
      }
    }

    // Tags
    const tags = sentryData.tags || [];
    if (sentryContext && Array.isArray(tags)) {
      for (const t of tags) {
        if (t.key && t.value) sentryContext.tags[t.key] = t.value;
      }
    }

    // User
    if (sentryData.user && sentryContext) {
      sentryContext.user =
        sentryData.user.email || sentryData.user.username || sentryData.user.ip_address || null;
    }
  }

  // ── Enrich from code graph ──
  if (graphData?.node) {
    confidence += 0.15;

    // Use graph data for more precise file/line
    if (graphData.node.file) file = graphData.node.file;
    if (graphData.node.line) line = graphData.node.line;
    if (graphData.node.name) fn = graphData.node.name;

    graphContext = {
      callers: graphData.callers || [],
      importers: graphData.callees || [],
      blast_radius: graphData.blast_radius || 0,
    };

    // Add callers to the call chain
    if (graphData.callers?.length > 0 && callChain.length === 0) {
      callChain = graphData.callers;
    }

    // Add recent events to the diagnosis
    if (graphData.recent_events?.length > 0) {
      rootCause += `\n\nRecent events on this function: ${graphData.recent_events.join(", ")}`;
    }
  }

  // ── Enrich from infrastructure data ──
  if (isInfra && infraData) {
    confidence += 0.2;

    // Build infra context into the diagnosis
    const findings = infraData.recent_findings || [];
    const agents = infraData.agents || [];
    const k8sEvents = infraData.k8s_events || [];

    if (findings.length > 0) {
      rootCause += "\n\nAgent findings:\n" + findings.map((f: any) =>
        `[${f.severity}] ${f.title}: ${f.message}`
      ).join("\n");
    }

    if (agents.length > 0) {
      rootCause += "\n\nAgents: " + agents.map((a: any) =>
        `${a.name} (${a.status}, ${a.capabilities?.length || 0} capabilities)`
      ).join(", ");
    }

    if (k8sEvents.length > 0) {
      rootCause += "\n\nRecent K8s events:\n" + k8sEvents.map((e: any) =>
        `[${e.severity}] ${e.title}`
      ).join("\n");
    }

    if (infraData.unhealthy_pods?.length > 0) {
      rootCause += "\n\nUnhealthy pods: " + infraData.unhealthy_pods.map((p: any) =>
        `${p.name} (${p.restarts} restarts)`
      ).join(", ");
    }
  }

  // ── Generate diagnosis based on error patterns ──
  const errorType = parsed.type;
  const errorMsg = parsed.message;

  // ── Infrastructure patterns ──
  if (isInfra) {
    const msg = errorMsg.toLowerCase();

    if (msg.includes("dns") || msg.includes("nameserver") || msg.includes("resolve")) {
      rootCause = `DNS resolution failure detected. ${rootCause}`;
      suggestedFix = "1. Check /etc/resolv.conf for nameserver configuration.\n2. Verify CoreDNS is running and its upstream forwarders are reachable.\n3. Add fallback DNS servers (1.1.1.1, 8.8.8.8) to CoreDNS forward config.\n4. If using Tailscale MagicDNS as sole nameserver, add non-Tailscale fallbacks.";
      severity = "critical";
      confidence += 0.15;
    } else if (msg.includes("tunnel") || msg.includes("cloudflared") || msg.includes("tls handshake")) {
      rootCause = `Network tunnel connectivity failure. ${rootCause}`;
      suggestedFix = "1. Check DNS resolution from within the cluster (CoreDNS upstream).\n2. Verify the host network interface is stable (WiFi power-save, packet drops).\n3. Check cloudflared pod logs for connection/reconnection patterns.\n4. If using WiFi, disable power management: iw dev <iface> set power_save off.";
      severity = "critical";
      confidence += 0.15;
    } else if (msg.includes("tx_drops") || msg.includes("rx_drops") || msg.includes("packet")) {
      rootCause = `Network packet loss on interface. ${rootCause}`;
      suggestedFix = "1. Check interface error counters: ip -s link show.\n2. If WiFi, check power-save mode and signal quality.\n3. If VPN (tailscale), check coordination server reachability.\n4. Consider switching to wired ethernet for server workloads.";
      severity = "warning";
      confidence += 0.1;
    } else if (msg.includes("restart") || msg.includes("crashloop") || msg.includes("oom")) {
      rootCause = `Pod instability - high restart count indicates recurring failures. ${rootCause}`;
      suggestedFix = "1. Check pod logs for the crash reason: kubectl logs <pod> --previous.\n2. Check resource limits - OOMKilled means the pod needs more memory.\n3. Check liveness/readiness probes - may be too aggressive.\n4. Check if a dependency (database, external service) is intermittently unavailable.";
      severity = "critical";
      confidence += 0.1;
    } else if (msg.includes("disk") || msg.includes("storage") || msg.includes("volume")) {
      rootCause = `Storage issue detected. ${rootCause}`;
      suggestedFix = "1. Check disk usage: df -h.\n2. Identify large files/directories: du -sh /* | sort -rh.\n3. Check if PVCs are bound and have available space.\n4. Consider enabling log rotation or cleaning old data.";
      severity = "warning";
      confidence += 0.1;
    } else if (msg.includes("memory") || msg.includes("load") || msg.includes("cpu")) {
      rootCause = `Resource pressure detected. ${rootCause}`;
      suggestedFix = "1. Check host resource usage: top, free -h.\n2. Identify resource-hungry pods: kubectl top pods --all-namespaces.\n3. Set resource limits on pods that don't have them.\n4. Consider scaling up the node or evicting non-critical workloads.";
      severity = "warning";
      confidence += 0.1;
    } else {
      rootCause = `Infrastructure issue detected. ${rootCause}`;
      suggestedFix = "1. Check agent findings for related alerts.\n2. Review pod logs for the affected services.\n3. Check host health (memory, disk, network).\n4. Review recent deployments or configuration changes.";
      confidence += 0.05;
    }
  } else if (errorType.includes("Prisma") || errorMsg.includes("Foreign key")) {
    rootCause = `Database foreign key constraint violation. The code at ${file || "?"}:${line || "?"} in ${fn || "?"}() tries to create a record referencing a row that doesn't exist in the parent table.`;
    suggestedFix = `1. Add a null check before the database write: verify the referenced ID exists with a findUnique() call.\n2. Wrap the create() in a try/catch so the error handler doesn't itself crash.\n3. Consider using connectOrCreate instead of a hard foreign key reference.`;
    severity = "error";
    confidence += 0.15;
  } else if (errorType.includes("LLMValidation") || errorMsg.includes("schema validation")) {
    rootCause = `LLM output does not match the expected schema. The AI model returned data that fails validation at ${file || "?"}:${line || "?"}.`;
    suggestedFix = `1. Add a fallback/retry with a more explicit prompt when validation fails.\n2. Use a default value for fields the LLM gets wrong (e.g., extract email from input data instead of LLM output).\n3. Consider using structured output mode (JSON mode) if available.`;
    severity = "warning";
    confidence += 0.1;
  } else if (errorType.includes("TypeError") || errorMsg.includes("Cannot read prop")) {
    const propMatch = errorMsg.match(/property '(\w+)' of (undefined|null)/);
    const prop = propMatch?.[1] || "unknown";
    rootCause = `Null reference: accessing property '${prop}' on undefined/null at ${file || "?"}:${line || "?"}.`;
    suggestedFix = `Add optional chaining (?.) or a null guard before accessing '${prop}'.`;
    severity = "error";
    confidence += 0.1;
  } else if (errorType.includes("ConnectionError") || errorMsg.includes("ECONNREFUSED")) {
    rootCause = `Service connection failure at ${file || "?"}:${line || "?"}. The downstream service is unreachable.`;
    suggestedFix = `1. Check if the target service is running and healthy.\n2. Add retry logic with exponential backoff.\n3. Add a circuit breaker to fail fast.`;
    severity = "critical";
  } else {
    rootCause = `${errorType} at ${file || "?"}:${line || "?"} in ${fn || "?"}(): ${errorMsg.slice(0, 200)}`;
    suggestedFix = `Review the code at the error location and add appropriate error handling.`;
  }

  // Cascade analysis
  if (callChain.length > 1) {
    rootCause += `\n\nCall chain: ${callChain.join(" → ")}`;
    if (callChain.length > 3) {
      rootCause += `\n\nThis error propagates through ${callChain.length} frames. The originating call is ${callChain[callChain.length - 1]}.`;
    }
  }

  // ── Enrich from tickets (Linear/Jira) ──
  if (ticketData && ticketData.length > 0) {
    confidence += 0.05;
    const ticket = ticketData[0];
    rootCause += `\n\nTracked: ${ticket.id} "${ticket.title}" (${ticket.status}${ticket.assignee ? ", assigned to " + ticket.assignee : ""})`;
  }

  // ── Enrich from Sentry search results (new format with stack_trace/breadcrumbs) ──
  if (sentryData?.stack_trace?.length > 0 && !sentryContext) {
    confidence += 0.15;
    sentryContext = {
      breadcrumbs: sentryData.breadcrumbs || [],
      tags: {},
      user: null,
    };
    // Use Sentry stack trace for file/line
    const topFrame = sentryData.stack_trace[sentryData.stack_trace.length - 1];
    if (topFrame) {
      const match = topFrame.match(/(.+):(\d+) in (.+)/);
      if (match) {
        file = file || match[1];
        line = line || parseInt(match[2]);
        fn = fn || match[3];
      }
    }
    callChain = callChain.length > 0 ? callChain : sentryData.stack_trace;
    if (sentryData.count) {
      rootCause += `\n\nSentry: ${sentryData.count} occurrences. First seen: ${sentryData.first_seen}. Last seen: ${sentryData.last_seen}.`;
    }
    if (sentryData.release) {
      rootCause += ` Release: ${sentryData.release}.`;
    }
  }

  return {
    root_cause: rootCause,
    file,
    line,
    function: fn,
    call_chain: callChain,
    suggested_fix: suggestedFix,
    severity,
    confidence: Math.min(confidence, 1),
    sources_used: sources,
    sentry_context: sentryContext,
    graph_context: graphContext,
    tickets: ticketData || undefined,
  };
}
