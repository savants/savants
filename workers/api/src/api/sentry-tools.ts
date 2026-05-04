/**
 * Sentry tools - replaces the Sentry MCP server.
 *
 * Users connect Sentry once via `savants connect sentry`.
 * These tools query the Sentry API using stored credentials.
 * Combined with the code graph, they provide richer context than
 * Sentry MCP alone.
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

interface SentryCreds {
  auth_token: string;
  org_slug: string;
}

async function getSentryCreds(db: Env["DB"], orgId: string): Promise<SentryCreds | null> {
  const row = await getIntegration(db, orgId, "sentry");
  if (!row) return null;
  const creds = JSON.parse(row.credentials || "{}");
  const config = JSON.parse(row.config || "{}");
  if (!creds.auth_token) return null;
  return { auth_token: creds.auth_token, org_slug: config.org_slug };
}

// ── Graph enrichment: match stack trace to code graph ──
async function enrichWithGraph(
  db: Env["DB"],
  orgId: string,
  stackFrames: Array<{ file?: string; function?: string; line?: number }>
): Promise<{
  enriched_frames: Array<{
    function: string;
    file: string;
    line: number;
    callers: string[];
    blast_radius: number;
    authored_by?: string;
    has_tests: boolean;
  }>;
  total_blast_radius: number;
}> {
  if (!stackFrames.length) return { enriched_frames: [], total_blast_radius: 0 };

  // Find the project for this org (most recently updated)
  const project = await db.prepare(
    "SELECT id FROM projects WHERE org_id = ?1 ORDER BY updated_at DESC LIMIT 1"
  ).bind(orgId).first<{ id: string }>();
  if (!project) return { enriched_frames: [], total_blast_radius: 0 };

  const enriched = [];
  let totalBlast = 0;

  for (const frame of stackFrames.slice(-5)) { // Top 5 frames
    if (!frame.function) continue;

    // Find in graph
    const node = await db.prepare(
      "SELECT id, name, file_path, line_start, metadata FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
    ).bind(project.id, frame.function).first<{ id: string; name: string; file_path: string; line_start: number; metadata: string }>();

    if (!node) continue;

    // Get callers
    const callers = await db.prepare(`
      SELECT n.name, n.file_path FROM graph_edges e
      JOIN graph_nodes n ON n.id = e.source_node
      WHERE e.target_node = ?1 AND e.type = 'CALLS' LIMIT 10
    `).bind(node.id).all();

    // Blast radius via recursive CTE
    const blast = await db.prepare(`
      WITH RECURSIVE br(node_id, depth) AS (
        SELECT ?1, 0
        UNION
        SELECT e.source_node, br.depth + 1
        FROM graph_edges e JOIN br ON br.node_id = e.target_node
        WHERE e.type = 'CALLS' AND br.depth < 3
      )
      SELECT COUNT(DISTINCT node_id) - 1 as c FROM br
    `).bind(node.id).first<{ c: number }>();

    const blastCount = blast?.c || 0;
    totalBlast += blastCount;

    enriched.push({
      function: node.name,
      file: node.file_path || frame.file || "?",
      line: node.line_start || frame.line || 0,
      callers: (callers.results as any[]).map(c => `${c.name}() in ${c.file_path}`),
      blast_radius: blastCount,
      has_tests: false, // Would need test file detection
    });
  }

  return { enriched_frames: enriched, total_blast_radius: totalBlast };
}

async function sentryGet(token: string, url: string): Promise<any> {
  const res = await fetch(url, {
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) return null;
  return res.json();
}

// ── get_sentry_issue ──
export async function getSentryIssue(db: Env["DB"], orgId: string, input: { issue_id: string }): Promise<any> {
  const creds = await getSentryCreds(db, orgId);
  if (!creds) return { error: "Sentry not connected. Run: savants connect sentry" };

  const issue = await sentryGet(creds.auth_token,
    `https://sentry.io/api/0/organizations/${creds.org_slug}/issues/${input.issue_id}/`
  );
  if (!issue) return { error: "Issue not found" };

  // Get latest event for stack trace
  const event = await sentryGet(creds.auth_token,
    `https://sentry.io/api/0/organizations/${creds.org_slug}/issues/${input.issue_id}/events/latest/`
  );

  const stackFrames = event?.entries
    ?.find((e: any) => e.type === "exception")
    ?.data?.values?.[0]?.stacktrace?.frames
    ?.filter((f: any) => f.inApp)
    ?.slice(-10)
    ?.map((f: any) => ({ file: f.filename, function: f.function, line: f.lineNo }))
    || [];

  const breadcrumbs = event?.entries
    ?.find((e: any) => e.type === "breadcrumbs")
    ?.data?.values?.slice(-10)
    ?.map((b: any) => ({ category: b.category, message: b.message, level: b.level }))
    || [];

  // Graph enrichment: match stack trace to code graph
  const graphContext = await enrichWithGraph(db, orgId, stackFrames);

  return {
    id: issue.id,
    short_id: issue.shortId,
    title: issue.title,
    culprit: issue.culprit,
    level: issue.level,
    status: issue.status,
    count: issue.count,
    first_seen: issue.firstSeen,
    last_seen: issue.lastSeen,
    assigned_to: issue.assignedTo?.name || null,
    platform: issue.platform,
    project: issue.project?.slug,
    stack_trace: stackFrames,
    breadcrumbs,
    tags: event?.tags?.filter((t: any) => ["environment", "release", "browser", "os", "server_name"].includes(t.key))
      ?.map((t: any) => ({ [t.key]: t.value })) || [],
    // Graph context: what Sentry MCP can't give you
    graph: graphContext.enriched_frames.length > 0 ? {
      matched_functions: graphContext.enriched_frames,
      total_blast_radius: graphContext.total_blast_radius,
      risk: graphContext.total_blast_radius > 10 ? "high" : graphContext.total_blast_radius > 3 ? "medium" : "low",
    } : undefined,
  };
}

// ── search_sentry_issues ──
export async function searchSentryIssues(db: Env["DB"], orgId: string, input: { query: string; project?: string }): Promise<any> {
  const creds = await getSentryCreds(db, orgId);
  if (!creds) return { error: "Sentry not connected" };

  let url = `https://sentry.io/api/0/organizations/${creds.org_slug}/issues/?query=${encodeURIComponent(input.query)}&per_page=10&sort=date`;
  if (input.project) {
    url += `&project=${input.project}`;
  }

  const issues = await sentryGet(creds.auth_token, url);
  if (!issues || !Array.isArray(issues)) return { results: [], count: 0 };

  // For each issue, try to match culprit to code graph
  const project = await db.prepare(
    "SELECT id FROM projects WHERE org_id = ?1 ORDER BY updated_at DESC LIMIT 1"
  ).bind(orgId).first<{ id: string }>();

  const results = [];
  for (const i of issues) {
    const result: any = {
      id: i.id,
      short_id: i.shortId,
      title: i.title,
      culprit: i.culprit,
      level: i.level,
      status: i.status,
      count: i.count,
      first_seen: i.firstSeen,
      last_seen: i.lastSeen,
      assigned_to: i.assignedTo?.name || null,
      project: i.project?.slug,
    };

    // Try to match culprit to graph node (lightweight - just callers count)
    if (project && i.culprit) {
      const funcName = i.culprit.split(/[./]/).pop()?.replace(/[()]/g, "");
      if (funcName) {
        const node = await db.prepare(
          "SELECT id FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
        ).bind(project.id, funcName).first<{ id: string }>();
        if (node) {
          const callerCount = await db.prepare(
            "SELECT COUNT(*) as c FROM graph_edges WHERE target_node = ?1 AND type = 'CALLS'"
          ).bind(node.id).first<{ c: number }>();
          result.graph = {
            function_in_graph: true,
            callers: callerCount?.c || 0,
            risk: (callerCount?.c || 0) > 5 ? "high" : "low",
          };
        }
      }
    }

    results.push(result);
  }

  return { results, count: results.length };
}

// ── search_sentry_events ──
export async function searchSentryEvents(db: Env["DB"], orgId: string, input: { query: string; project?: string }): Promise<any> {
  const creds = await getSentryCreds(db, orgId);
  if (!creds) return { error: "Sentry not connected" };

  let url = `https://sentry.io/api/0/organizations/${creds.org_slug}/events/?query=${encodeURIComponent(input.query)}&per_page=10&field=title&field=event.type&field=project&field=timestamp&field=id`;
  const events = await sentryGet(creds.auth_token, url);
  if (!events?.data) return { results: [], count: 0 };

  return {
    results: events.data.map((e: any) => ({
      id: e.id,
      title: e.title,
      type: e["event.type"],
      project: e.project,
      timestamp: e.timestamp,
    })),
    count: events.data.length,
  };
}

// ── update_sentry_issue ──
export async function updateSentryIssue(db: Env["DB"], orgId: string, input: { issue_id: string; status?: string; assigned_to?: string }): Promise<any> {
  const creds = await getSentryCreds(db, orgId);
  if (!creds) return { error: "Sentry not connected" };

  const body: any = {};
  if (input.status) body.status = input.status; // resolved, unresolved, ignored
  if (input.assigned_to) body.assignedTo = input.assigned_to;

  const res = await fetch(
    `https://sentry.io/api/0/organizations/${creds.org_slug}/issues/${input.issue_id}/`,
    {
      method: "PUT",
      headers: { Authorization: `Bearer ${creds.auth_token}`, "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(10000),
    }
  );

  if (!res.ok) return { error: `Failed to update: ${res.status}` };
  const updated = await res.json<any>();
  return { updated: true, id: updated.id, status: updated.status, assigned_to: updated.assignedTo?.name };
}

// ── find_sentry_releases ──
export async function findSentryReleases(db: Env["DB"], orgId: string, input: { project?: string }): Promise<any> {
  const creds = await getSentryCreds(db, orgId);
  if (!creds) return { error: "Sentry not connected" };

  let url = `https://sentry.io/api/0/organizations/${creds.org_slug}/releases/?per_page=10&sort=date`;
  if (input.project) {
    url += `&project=${input.project}`;
  }

  const releases = await sentryGet(creds.auth_token, url);
  if (!releases || !Array.isArray(releases)) return { results: [], count: 0 };

  return {
    results: releases.map((r: any) => ({
      version: r.version,
      short_version: r.shortVersion,
      date_released: r.dateReleased,
      date_created: r.dateCreated,
      new_groups: r.newGroups,
      project: r.projects?.[0]?.slug,
      commit_count: r.commitCount,
      last_deploy: r.lastDeploy?.dateFinished,
    })),
    count: releases.length,
  };
}

// ── get_sentry_issue_tags ──
export async function getSentryIssueTags(db: Env["DB"], orgId: string, input: { issue_id: string; tag: string }): Promise<any> {
  const creds = await getSentryCreds(db, orgId);
  if (!creds) return { error: "Sentry not connected" };

  const values = await sentryGet(creds.auth_token,
    `https://sentry.io/api/0/organizations/${creds.org_slug}/issues/${input.issue_id}/tags/${input.tag}/values/?per_page=10`
  );

  if (!values || !Array.isArray(values)) return { tag: input.tag, values: [] };

  return {
    tag: input.tag,
    values: values.map((v: any) => ({
      value: v.value,
      count: v.count,
      percentage: v.percentage,
      first_seen: v.firstSeen,
      last_seen: v.lastSeen,
    })),
  };
}

// ── Dispatcher ──
export const SENTRY_TOOL_NAMES = [
  "get_sentry_issue", "search_sentry_issues", "search_sentry_events",
  "update_sentry_issue", "find_sentry_releases", "get_sentry_issue_tags",
];

export async function executeSentryTool(
  db: Env["DB"],
  orgId: string,
  tool: string,
  input: Record<string, unknown>
): Promise<any> {
  switch (tool) {
    case "get_sentry_issue":
      return getSentryIssue(db, orgId, input as any);
    case "search_sentry_issues":
      return searchSentryIssues(db, orgId, input as any);
    case "search_sentry_events":
      return searchSentryEvents(db, orgId, input as any);
    case "update_sentry_issue":
      return updateSentryIssue(db, orgId, input as any);
    case "find_sentry_releases":
      return findSentryReleases(db, orgId, input as any);
    case "get_sentry_issue_tags":
      return getSentryIssueTags(db, orgId, input as any);
    default:
      return { error: `Unknown sentry tool: ${tool}` };
  }
}
