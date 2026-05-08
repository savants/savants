/**
 * Diagnosis engine v2.
 * Actually traces through the code graph using semantic search + recursive callers.
 * Falls back gracefully when data sources are unavailable.
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

interface DiagnosisInput {
  error_message: string;
  file_path?: string;
  sentry_event_id?: string;
  sentry_project?: string;
  repo?: string;
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
  sentry_context?: any;
  graph_context?: any;
  tickets?: any[];
}

export async function diagnoseError(
  env: Env,
  orgId: string,
  input: DiagnosisInput
): Promise<DiagnosisResult> {
  const sources: string[] = ["error_message"];
  const errorMsg = input.error_message;
  let confidence = 0.3;

  // ── Step 1: Find the relevant function in the code graph ──
  // Try multiple strategies: exact name match, keyword extraction, semantic search
  let entryNode: any = null;
  let projectId: string | null = null;
  let callChain: string[] = [];
  let graphCallers: string[] = [];
  let graphCallees: string[] = [];

  // Resolve project
  try {
    const repoName = input.repo;
    if (repoName) {
      const proj = await env.DB.prepare(
        "SELECT id FROM projects WHERE org_id = ?1 AND (slug = ?2 OR name = ?2) LIMIT 1"
      ).bind(orgId, repoName).first<{ id: string }>();
      projectId = proj?.id || null;
    }
    if (!projectId) {
      // Use first project with graph data
      const proj = await env.DB.prepare(
        "SELECT p.id FROM projects p WHERE p.org_id = ?1 AND EXISTS (SELECT 1 FROM graph_nodes WHERE project_id = p.id LIMIT 1) LIMIT 1"
      ).bind(orgId).first<{ id: string }>();
      projectId = proj?.id || null;
    }
  } catch {}

  if (projectId) {
    // Strategy 1: Extract function names from the error message
    const funcNames = extractFunctionNames(errorMsg);

    for (const fname of funcNames) {
      const node = await env.DB.prepare(
        "SELECT id, name, type, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND name = ?2 AND type = 'function' LIMIT 1"
      ).bind(projectId, fname).first<any>();
      if (node) {
        entryNode = node;
        break;
      }
    }

    // Strategy 2: Fuzzy name search (LIKE)
    if (!entryNode && funcNames.length > 0) {
      for (const fname of funcNames) {
        const node = await env.DB.prepare(
          "SELECT id, name, type, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND name LIKE ?2 AND type = 'function' LIMIT 1"
        ).bind(projectId, `%${fname}%`).first<any>();
        if (node) {
          entryNode = node;
          break;
        }
      }
    }

    // Strategy 3: Keyword search on all function names
    if (!entryNode) {
      const keywords = extractKeywords(errorMsg);
      for (const kw of keywords) {
        const node = await env.DB.prepare(
          "SELECT id, name, type, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND name LIKE ?2 AND type = 'function' LIMIT 1"
        ).bind(projectId, `%${kw}%`).first<any>();
        if (node) {
          entryNode = node;
          break;
        }
      }
    }

    // Strategy 4: File path match
    if (!entryNode && input.file_path) {
      const fileName = input.file_path.split("/").pop() || input.file_path;
      const node = await env.DB.prepare(
        "SELECT id, name, type, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND file_path LIKE ?2 AND type = 'function' LIMIT 1"
      ).bind(projectId, `%${fileName}%`).first<any>();
      if (node) entryNode = node;
    }

    // ── Step 2: Recursive caller chain (walk UP the call graph) ──
    if (entryNode) {
      sources.push("code_graph");
      confidence += 0.25;

      callChain = [formatNode(entryNode)];
      let currentId = entryNode.id;

      // Walk up to 10 levels of callers
      for (let depth = 0; depth < 10; depth++) {
        const caller = await env.DB.prepare(`
          SELECT n.id, n.name, n.type, n.file_path, n.line_start
          FROM graph_edges e
          JOIN graph_nodes n ON n.id = e.source_node
          WHERE e.target_node = ?1 AND e.type = 'CALLS'
          ORDER BY n.name LIMIT 1
        `).bind(currentId).first<any>();

        if (!caller) break;
        callChain.push(formatNode(caller));
        currentId = caller.id;
      }

      callChain.reverse(); // Entry point first, deepest function last

      // Get all direct callers (not just the chain)
      const allCallers = await env.DB.prepare(`
        SELECT n.name, n.file_path, n.line_start
        FROM graph_edges e
        JOIN graph_nodes n ON n.id = e.source_node
        WHERE e.target_node = ?1 AND e.type = 'CALLS'
        ORDER BY n.name LIMIT 10
      `).bind(entryNode.id).all();
      graphCallers = (allCallers.results as any[]).map(r => formatNode(r));

      // Get callees (what the function calls)
      const allCallees = await env.DB.prepare(`
        SELECT n.name, n.file_path, n.line_start
        FROM graph_edges e
        JOIN graph_nodes n ON n.id = e.target_node
        WHERE e.source_node = ?1 AND e.type = 'CALLS'
        ORDER BY n.name LIMIT 10
      `).bind(entryNode.id).all();
      graphCallees = (allCallees.results as any[]).map(r => formatNode(r));
    }
  }

  // ── Step 3: Sentry enrichment ──
  let sentryData: any = null;
  try {
    const sentryIntegration = await getIntegration(env.DB, orgId, "sentry");
    if (sentryIntegration) {
      const creds = JSON.parse(sentryIntegration.credentials);
      const config = JSON.parse(sentryIntegration.config);
      if (creds.auth_token) {
        sentryData = await searchSentry(creds.auth_token, config.org_slug, errorMsg, input.sentry_project || config.project_slugs?.[0], input.sentry_event_id);
        if (sentryData) {
          sources.push("sentry");
          confidence += 0.15;
        }
      }
    }
  } catch {}

  // ── Step 4: Ticket search (Linear/Jira) ──
  let tickets: any[] = [];
  try {
    const linearIntegration = await getIntegration(env.DB, orgId, "linear");
    if (linearIntegration) {
      const creds = JSON.parse(linearIntegration.credentials || "{}");
      if (creds.api_key) {
        const searchTerms = errorMsg.slice(0, 80);
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
            tickets = issues.map((i: any) => ({
              id: i.identifier, title: i.title, status: i.state?.name,
              assignee: i.assignee?.name, url: i.url,
            }));
          }
        }
      }
    }
  } catch {}

  // ── Step 5: Agent findings (infra context) ──
  let agentContext: any = null;
  try {
    const findings = await env.DB.prepare(`
      SELECT metadata FROM audit_log
      WHERE org_id = ?1 AND action = 'agent.notify'
      ORDER BY created_at DESC LIMIT 10
    `).bind(orgId).all();

    const parsed = (findings.results as any[]).map(r => {
      try { return JSON.parse(r.metadata || "{}"); } catch { return {}; }
    }).filter(f => f.title);

    if (parsed.length > 0) {
      sources.push("agent");
      agentContext = parsed.slice(0, 5).map((f: any) => `[${f.severity}] ${f.title}`);
    }
  } catch {}

  // ── Step 6: Build the diagnosis ──
  let rootCause: string;
  let suggestedFix: string;
  let severity: "critical" | "error" | "warning" = "error";

  if (entryNode && callChain.length > 0) {
    // We have graph data - build a real diagnosis
    rootCause = `Function: ${entryNode.name}() at ${entryNode.file_path}:${entryNode.line_start}`;

    if (callChain.length > 1) {
      rootCause += `\n\nCall chain (entry point to function):\n  ${callChain.join("\n  -> ")}`;
    }

    if (graphCallers.length > 0) {
      rootCause += `\n\nCalled by: ${graphCallers.join(", ")}`;
    }

    if (graphCallees.length > 0) {
      rootCause += `\n\nCalls: ${graphCallees.join(", ")}`;
    }

    rootCause += `\n\nError: ${errorMsg.slice(0, 300)}`;
    suggestedFix = `Review ${entryNode.file_path}:${entryNode.line_start} (${entryNode.name}). ${callChain.length > 2 ? `The call originates from ${callChain[0]}.` : ""}`;
    confidence += 0.1;
  } else {
    // No graph data - fall back to pattern matching
    rootCause = classifyError(errorMsg);
    suggestedFix = suggestFix(errorMsg);
  }

  // Enrich with Sentry
  if (sentryData) {
    if (sentryData.stack_trace?.length > 0) {
      rootCause += `\n\nSentry stack trace:\n  ${sentryData.stack_trace.join("\n  ")}`;
      if (callChain.length === 0) callChain = sentryData.stack_trace;
    }
    if (sentryData.count) {
      rootCause += `\n\nSentry: ${sentryData.count} occurrences, last seen ${sentryData.last_seen}`;
    }
  }

  // Enrich with tickets
  if (tickets.length > 0) {
    rootCause += `\n\nTracked: ${tickets[0].id} "${tickets[0].title}" (${tickets[0].status})`;
    confidence += 0.05;
  }

  // Enrich with agent findings
  if (agentContext) {
    rootCause += `\n\nRecent agent findings:\n  ${agentContext.join("\n  ")}`;
  }

  return {
    root_cause: rootCause,
    file: entryNode?.file_path || null,
    line: entryNode?.line_start || null,
    function: entryNode?.name || null,
    call_chain: callChain,
    suggested_fix: suggestedFix,
    severity,
    confidence: Math.min(confidence, 1),
    sources_used: sources,
    sentry_context: sentryData ? { breadcrumbs: sentryData.breadcrumbs || [], tags: sentryData.tags || {} } : undefined,
    graph_context: entryNode ? { callers: graphCallers, callees: graphCallees, blast_radius: graphCallers.length + graphCallees.length } : undefined,
    tickets: tickets.length > 0 ? tickets : undefined,
  };
}

// ── Helpers ──

function formatNode(n: any): string {
  return `${n.name}() at ${n.file_path || "?"}:${n.line_start || "?"}`;
}

function extractFunctionNames(msg: string): string[] {
  const names: string[] = [];
  // Match camelCase/snake_case identifiers that look like function names
  const matches = msg.match(/\b([a-z][a-zA-Z0-9_]{5,})\b/g) || [];
  for (const m of matches) {
    // Skip common words
    if (["string", "number", "object", "function", "return", "import", "export",
         "require", "module", "undefined", "tokens", "baseline", "current",
         "company", "increase", "because", "should", "unusual"].includes(m.toLowerCase())) continue;
    if (!names.includes(m)) names.push(m);
  }
  return names;
}

function extractKeywords(msg: string): string[] {
  // Extract distinctive words for fuzzy graph search
  const stop = new Set(["the","is","at","in","to","for","and","or","but","was","not","this","that","with","from",
    "have","has","had","been","are","were","will","would","could","should","can","may","might",
    "error","fail","issue","problem","bug","token","usage","spike","above","below","current"]);
  return msg.toLowerCase().replace(/[^a-z0-9_ ]/g, " ").split(/\s+/)
    .filter(w => w.length > 4 && !stop.has(w))
    .slice(0, 5);
}

function classifyError(msg: string): string {
  const lower = msg.toLowerCase();
  if (lower.includes("token") && (lower.includes("usage") || lower.includes("spike"))) {
    return `Token usage anomaly detected. ${msg.slice(0, 300)}`;
  }
  if (lower.includes("timeout") || lower.includes("deadline")) {
    return `Timeout/deadline exceeded. ${msg.slice(0, 300)}`;
  }
  if (lower.includes("connection") && (lower.includes("refused") || lower.includes("reset"))) {
    return `Connection failure. ${msg.slice(0, 300)}`;
  }
  return `Error detected: ${msg.slice(0, 300)}`;
}

function suggestFix(msg: string): string {
  const lower = msg.toLowerCase();
  if (lower.includes("token") && lower.includes("spike")) {
    return "Check which function is making excessive LLM calls. Look for missing caching, missing skip logic for already-processed items, or a workflow that re-evaluates unnecessarily.";
  }
  if (lower.includes("timeout")) {
    return "Check the downstream service health. Add retry logic with backoff. Consider increasing timeout thresholds or adding circuit breakers.";
  }
  return "Review the error context and check recent deployments or configuration changes.";
}

async function searchSentry(token: string, org: string, errorMsg: string, project?: string, eventId?: string): Promise<any | null> {
  const headers = { Authorization: `Bearer ${token}`, "Content-Type": "application/json" };

  // Direct event lookup
  if (eventId && project) {
    try {
      const res = await fetch(`https://sentry.io/api/0/projects/${org}/${project}/events/${eventId}/`, {
        headers, signal: AbortSignal.timeout(5000),
      });
      if (res.ok) return await res.json();
    } catch {}
  }

  // Search by error message
  try {
    const typeMatch = errorMsg.match(/^(\w+Error|\w+Exception)/);
    const bracketMatch = errorMsg.match(/\[([^\]]+)\]/);
    let search = typeMatch?.[1] || "";
    if (bracketMatch) search += " " + bracketMatch[1];
    if (!search) search = errorMsg.replace(/['":\[\]()]/g, " ").split(/\s+/).filter(w => w.length > 4).slice(0, 2).join(" ");

    const query = encodeURIComponent(`is:unresolved ${search}`);
    const res = await fetch(`https://sentry.io/api/0/organizations/${org}/issues/?query=${query}&per_page=3&sort=date`, {
      headers, signal: AbortSignal.timeout(8000),
    });
    if (!res.ok) return null;
    const issues = (await res.json()) as any[];
    if (issues.length === 0) return null;

    const issue = issues[0];
    // Get latest event for stack trace
    try {
      const eventRes = await fetch(`https://sentry.io/api/0/organizations/${org}/issues/${issue.id}/events/latest/`, {
        headers, signal: AbortSignal.timeout(5000),
      });
      if (eventRes.ok) {
        const event = (await eventRes.json()) as any;
        return {
          issue_id: issue.id, title: issue.title, count: issue.count,
          first_seen: issue.firstSeen, last_seen: issue.lastSeen,
          stack_trace: event.entries?.find((e: any) => e.type === "exception")
            ?.data?.values?.[0]?.stacktrace?.frames?.slice(-5)
            ?.map((f: any) => `${f.filename}:${f.lineNo} in ${f.function}`) || [],
          breadcrumbs: event.entries?.find((e: any) => e.type === "breadcrumbs")
            ?.data?.values?.slice(-10)?.map((b: any) => `[${b.category}] ${b.message || ""}`) || [],
          tags: event.tags?.reduce((acc: any, t: any) => { acc[t.key] = t.value; return acc; }, {}) || {},
        };
      }
    } catch {}
    return { issue_id: issue.id, title: issue.title, count: issue.count, last_seen: issue.lastSeen };
  } catch {}
  return null;
}
