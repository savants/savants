/**
 * Linear tools - replaces the Linear MCP server.
 *
 * 12 core tools: search, CRUD issues, projects, cycles, comments.
 * Graph-enriched: issues matched to code graph functions.
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

async function getLinearKey(db: Env["DB"], orgId: string): Promise<string | null> {
  const row = await getIntegration(db, orgId, "linear");
  if (!row) return null;
  const creds = JSON.parse(row.credentials || "{}");
  return creds.api_key || null;
}

async function linearQuery(apiKey: string, query: string, variables?: Record<string, unknown>): Promise<any> {
  const res = await fetch("https://api.linear.app/graphql", {
    method: "POST",
    headers: { Authorization: apiKey, "Content-Type": "application/json" },
    body: JSON.stringify({ query, variables }),
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) return null;
  const data = await res.json<any>();
  return data?.data;
}

// ── search_linear_issues ──
async function searchIssues(db: Env["DB"], orgId: string, input: { query: string }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };
  const data = await linearQuery(key, `
    query($q: String!) {
      issueSearch(query: $q, first: 15) {
        nodes { id identifier title state { name } assignee { name } priority priorityLabel
          team { name } createdAt updatedAt url labels { nodes { name } } }
      }
    }
  `, { q: input.query });
  if (!data) return { results: [], count: 0 };
  return {
    results: data.issueSearch.nodes.map((i: any) => ({
      id: i.identifier, title: i.title, status: i.state?.name, assignee: i.assignee?.name,
      priority: i.priorityLabel, team: i.team?.name, url: i.url,
      labels: i.labels?.nodes?.map((l: any) => l.name) || [],
      created_at: i.createdAt, updated_at: i.updatedAt,
    })),
    count: data.issueSearch.nodes.length,
  };
}

// ── get_linear_issue ──
async function getIssue(db: Env["DB"], orgId: string, input: { issue_id: string }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };
  const data = await linearQuery(key, `
    query($id: String!) {
      issue(id: $id) { id identifier title description state { name } assignee { name email }
        priority priorityLabel team { name } project { name } cycle { name number }
        createdAt updatedAt completedAt url estimate
        labels { nodes { name } }
        comments { nodes { body user { name } createdAt } }
        relations { nodes { type relatedIssue { identifier title state { name } } } }
      }
    }
  `, { id: input.issue_id });
  if (!data?.issue) return { error: "Issue not found" };
  const i = data.issue;

  // Graph enrichment: match issue title/description to code functions
  let graph = null;
  const project = await db.prepare(
    "SELECT id FROM projects WHERE org_id = ?1 ORDER BY updated_at DESC LIMIT 1"
  ).bind(orgId).first<{ id: string }>();
  if (project && i.title) {
    const words = i.title.split(/\s+/).filter((w: string) => w.length > 4).slice(0, 3);
    for (const word of words) {
      const node = await db.prepare(
        "SELECT id, name, file_path FROM graph_nodes WHERE project_id = ?1 AND name LIKE ?2 AND type = 'function' LIMIT 1"
      ).bind(project.id, `%${word}%`).first<any>();
      if (node) {
        const callers = await db.prepare(
          "SELECT COUNT(*) as c FROM graph_edges WHERE target_node = ?1 AND type = 'CALLS'"
        ).bind(node.id).first<{ c: number }>();
        graph = { function: node.name, file: node.file_path, callers: callers?.c || 0 };
        break;
      }
    }
  }

  return {
    id: i.identifier, title: i.title, description: i.description?.slice(0, 500),
    status: i.state?.name, assignee: i.assignee?.name, assignee_email: i.assignee?.email,
    priority: i.priorityLabel, team: i.team?.name, project: i.project?.name,
    cycle: i.cycle ? `${i.cycle.name} (#${i.cycle.number})` : null,
    estimate: i.estimate, url: i.url,
    labels: i.labels?.nodes?.map((l: any) => l.name) || [],
    comments: i.comments?.nodes?.slice(0, 5)?.map((c: any) => ({
      author: c.user?.name, body: c.body?.slice(0, 200), created_at: c.createdAt,
    })) || [],
    relations: i.relations?.nodes?.map((r: any) => ({
      type: r.type, issue: r.relatedIssue?.identifier, title: r.relatedIssue?.title,
      status: r.relatedIssue?.state?.name,
    })) || [],
    created_at: i.createdAt, updated_at: i.updatedAt, completed_at: i.completedAt,
    graph: graph || undefined,
  };
}

// ── create_linear_issue ──
async function createIssue(db: Env["DB"], orgId: string, input: { title: string; description?: string; team_id?: string; assignee_id?: string; priority?: number }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };

  // If no team_id, get the first team
  let teamId = input.team_id;
  if (!teamId) {
    const teams = await linearQuery(key, `{ teams { nodes { id name } } }`);
    teamId = teams?.teams?.nodes?.[0]?.id;
  }
  if (!teamId) return { error: "No team found" };

  const data = await linearQuery(key, `
    mutation($input: IssueCreateInput!) {
      issueCreate(input: $input) { issue { id identifier title url state { name } } success }
    }
  `, { input: { title: input.title, description: input.description, teamId, assigneeId: input.assignee_id, priority: input.priority } });

  if (!data?.issueCreate?.success) return { error: "Failed to create issue" };
  const i = data.issueCreate.issue;
  return { created: true, id: i.identifier, title: i.title, status: i.state?.name, url: i.url };
}

// ── update_linear_issue ──
async function updateIssue(db: Env["DB"], orgId: string, input: { issue_id: string; status?: string; assignee_id?: string; priority?: number; title?: string }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };

  // Resolve status name to state ID
  let stateId: string | undefined;
  if (input.status) {
    const states = await linearQuery(key, `{ workflowStates { nodes { id name } } }`);
    const match = states?.workflowStates?.nodes?.find((s: any) =>
      s.name.toLowerCase() === input.status!.toLowerCase()
    );
    stateId = match?.id;
  }

  const updateInput: any = {};
  if (stateId) updateInput.stateId = stateId;
  if (input.assignee_id) updateInput.assigneeId = input.assignee_id;
  if (input.priority !== undefined) updateInput.priority = input.priority;
  if (input.title) updateInput.title = input.title;

  const data = await linearQuery(key, `
    mutation($id: String!, $input: IssueUpdateInput!) {
      issueUpdate(id: $id, input: $input) { issue { identifier title state { name } assignee { name } } success }
    }
  `, { id: input.issue_id, input: updateInput });

  if (!data?.issueUpdate?.success) return { error: "Failed to update" };
  const i = data.issueUpdate.issue;
  return { updated: true, id: i.identifier, title: i.title, status: i.state?.name, assignee: i.assignee?.name };
}

// ── add_linear_comment ──
async function addComment(db: Env["DB"], orgId: string, input: { issue_id: string; body: string }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };
  const data = await linearQuery(key, `
    mutation($input: CommentCreateInput!) {
      commentCreate(input: $input) { comment { id body } success }
    }
  `, { input: { issueId: input.issue_id, body: input.body } });
  return { posted: data?.commentCreate?.success || false };
}

// ── list_linear_projects ──
async function listProjects(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };
  const data = await linearQuery(key, `
    { projects(first: 20, orderBy: updatedAt) {
      nodes { id name state slugId progress startDate targetDate
        lead { name } teams { nodes { name } } }
    } }
  `);
  if (!data) return { results: [] };
  return {
    results: data.projects.nodes.map((p: any) => ({
      id: p.id, name: p.name, status: p.state, progress: Math.round((p.progress || 0) * 100),
      lead: p.lead?.name, teams: p.teams?.nodes?.map((t: any) => t.name) || [],
      start_date: p.startDate, target_date: p.targetDate,
    })),
  };
}

// ── list_linear_teams ──
async function listTeams(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };
  const data = await linearQuery(key, `
    { teams { nodes { id name key description members { nodes { name email } } } } }
  `);
  if (!data) return { results: [] };
  return {
    results: data.teams.nodes.map((t: any) => ({
      id: t.id, name: t.name, key: t.key,
      members: t.members?.nodes?.map((m: any) => m.name) || [],
    })),
  };
}

// ── get_linear_cycle ──
async function getActiveCycle(db: Env["DB"], orgId: string, input: { team_id?: string }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };
  const data = await linearQuery(key, `
    { cycles(filter: { isActive: { eq: true } }, first: 5) {
      nodes { id name number startsAt endsAt progress completedAt
        issues { nodes { identifier title state { name } assignee { name } priority } }
      }
    } }
  `);
  if (!data?.cycles?.nodes?.length) return { cycle: null };
  const c = data.cycles.nodes[0];
  return {
    name: c.name, number: c.number, progress: Math.round((c.progress || 0) * 100),
    starts_at: c.startsAt, ends_at: c.endsAt,
    issues: c.issues?.nodes?.map((i: any) => ({
      id: i.identifier, title: i.title, status: i.state?.name,
      assignee: i.assignee?.name, priority: i.priority,
    })) || [],
  };
}

// ── list_linear_issues (by team/project/status) ──
async function listIssues(db: Env["DB"], orgId: string, input: { team?: string; status?: string; assignee?: string }): Promise<any> {
  const key = await getLinearKey(db, orgId);
  if (!key) return { error: "Linear not connected" };

  let filter = "";
  const filters = [];
  if (input.team) filters.push(`team: { name: { eq: "${input.team}" } }`);
  if (input.status) filters.push(`state: { name: { eq: "${input.status}" } }`);
  if (input.assignee) filters.push(`assignee: { name: { eq: "${input.assignee}" } }`);
  if (filters.length) filter = `filter: { ${filters.join(", ")} },`;

  const data = await linearQuery(key, `
    { issues(${filter} first: 20, orderBy: updatedAt) {
      nodes { identifier title state { name } assignee { name } priority priorityLabel
        team { name } createdAt updatedAt url }
    } }
  `);
  if (!data) return { results: [] };
  return {
    results: data.issues.nodes.map((i: any) => ({
      id: i.identifier, title: i.title, status: i.state?.name, assignee: i.assignee?.name,
      priority: i.priorityLabel, team: i.team?.name, url: i.url, updated_at: i.updatedAt,
    })),
  };
}

// ── Dispatcher ──
export const LINEAR_TOOL_NAMES = [
  "search_linear_issues", "get_linear_issue", "create_linear_issue",
  "update_linear_issue", "add_linear_comment", "list_linear_projects",
  "list_linear_teams", "get_active_cycle", "list_linear_issues",
];

export async function executeLinearTool(
  db: Env["DB"], orgId: string, tool: string, input: Record<string, unknown>
): Promise<any> {
  switch (tool) {
    case "search_linear_issues": return searchIssues(db, orgId, input as any);
    case "get_linear_issue": return getIssue(db, orgId, input as any);
    case "create_linear_issue": return createIssue(db, orgId, input as any);
    case "update_linear_issue": return updateIssue(db, orgId, input as any);
    case "add_linear_comment": return addComment(db, orgId, input as any);
    case "list_linear_projects": return listProjects(db, orgId, input as any);
    case "list_linear_teams": return listTeams(db, orgId, input as any);
    case "get_active_cycle": return getActiveCycle(db, orgId, input as any);
    case "list_linear_issues": return listIssues(db, orgId, input as any);
    default: return { error: `Unknown linear tool: ${tool}` };
  }
}
