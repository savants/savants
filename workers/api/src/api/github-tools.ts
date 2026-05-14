/**
 * GitHub tools - replaces the GitHub MCP server.
 *
 * 17 core tools covering issues, PRs, commits, CI, and security.
 * Every response enriched with code graph context (blast radius, callers).
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

interface GitHubCreds {
  token: string;
}

async function getGitHubCreds(db: Env["DB"], orgId: string): Promise<GitHubCreds | null> {
  const row = await getIntegration(db, orgId, "github");
  if (!row) return null;
  const creds = JSON.parse(row.credentials || "{}");
  return creds.token || creds.access_token ? { token: creds.token || creds.access_token } : null;
}

async function ghGet(token: string, url: string): Promise<any> {
  const res = await fetch(url, {
    headers: { Authorization: `Bearer ${token}`, "User-Agent": "Savants", Accept: "application/vnd.github.v3+json" },
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) return null;
  return res.json();
}

async function ghPost(token: string, url: string, body: any): Promise<any> {
  const res = await fetch(url, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}`, "User-Agent": "Savants", "Content-Type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  return { ok: res.ok, status: res.status, data: res.ok ? await res.json() : await res.text() };
}

async function ghPatch(token: string, url: string, body: any): Promise<any> {
  const res = await fetch(url, {
    method: "PATCH",
    headers: { Authorization: `Bearer ${token}`, "User-Agent": "Savants", "Content-Type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  return { ok: res.ok, status: res.status, data: res.ok ? await res.json() : await res.text() };
}

async function ghPut(token: string, url: string, body: any): Promise<any> {
  const res = await fetch(url, {
    method: "PUT",
    headers: { Authorization: `Bearer ${token}`, "User-Agent": "Savants", "Content-Type": "application/json" },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(10000),
  });
  return { ok: res.ok, status: res.status, data: res.ok ? await res.json() : await res.text() };
}

// Graph enrichment for file lists (PRs, commits)
async function enrichFiles(db: Env["DB"], orgId: string, files: string[]): Promise<any> {
  const project = await db.prepare(
    "SELECT id FROM projects WHERE org_id = ?1 ORDER BY updated_at DESC LIMIT 1"
  ).bind(orgId).first<{ id: string }>();
  if (!project || !files.length) return null;

  let totalBlast = 0;
  const riskyFunctions: any[] = [];

  for (const file of files.slice(0, 10)) {
    const nodes = await db.prepare(
      "SELECT id, name FROM graph_nodes WHERE project_id = ?1 AND file_path LIKE ?2 AND type = 'function' LIMIT 5"
    ).bind(project.id, `%${file.split("/").pop()}`).all();

    for (const node of nodes.results as any[]) {
      const callers = await db.prepare(
        "SELECT COUNT(*) as c FROM graph_edges WHERE target_node = ?1 AND type = 'CALLS'"
      ).bind(node.id).first<{ c: number }>();
      const count = callers?.c || 0;
      totalBlast += count;
      if (count > 0) {
        riskyFunctions.push({ name: node.name, file, callers: count });
      }
    }
  }

  if (!riskyFunctions.length) return null;

  riskyFunctions.sort((a, b) => b.callers - a.callers);
  return {
    total_blast_radius: totalBlast,
    risk: totalBlast > 20 ? "high" : totalBlast > 5 ? "medium" : "low",
    risky_functions: riskyFunctions.slice(0, 10),
  };
}

// ── search_github_issues ──
async function searchIssues(db: Env["DB"], orgId: string, input: { query: string; repo?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  let q = input.query;
  if (input.repo) q += ` repo:${input.repo}`;
  const data = await ghGet(creds.token, `https://api.github.com/search/issues?q=${encodeURIComponent(q)}&per_page=10&sort=updated`);
  if (!data) return { results: [], count: 0 };
  return {
    results: data.items?.map((i: any) => ({
      number: i.number, title: i.title, state: i.state, user: i.user?.login,
      labels: i.labels?.map((l: any) => l.name), created_at: i.created_at,
      updated_at: i.updated_at, url: i.html_url, repo: i.repository_url?.split("/").slice(-2).join("/"),
    })) || [],
    count: data.total_count,
  };
}

// ── search_github_code ──
async function searchCode(db: Env["DB"], orgId: string, input: { query: string; repo?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  let q = input.query;
  if (input.repo) q += ` repo:${input.repo}`;
  const data = await ghGet(creds.token, `https://api.github.com/search/code?q=${encodeURIComponent(q)}&per_page=10`);
  if (!data) return { results: [], count: 0 };
  return {
    results: data.items?.map((i: any) => ({
      name: i.name, path: i.path, repo: i.repository?.full_name, url: i.html_url,
    })) || [],
    count: data.total_count,
  };
}

// ── search_github_prs ──
async function searchPRs(db: Env["DB"], orgId: string, input: { query: string; repo?: string; author?: string; state?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };

  // Build proper GitHub search query
  let q = `is:pr ${input.query}`;
  if (input.repo) q += ` repo:${input.repo}`;
  if (input.author) q += ` author:${input.author}`;
  if (input.state) q += ` is:${input.state}`;

  const data = await ghGet(creds.token, `https://api.github.com/search/issues?q=${encodeURIComponent(q)}&per_page=20&sort=updated`);
  if (!data) return { results: [], count: 0 };

  // Enrich with PR details (additions/deletions) for each result
  const results = [];
  for (const i of (data.items || []).slice(0, 15)) {
    const pr: any = {
      number: i.number, title: i.title, state: i.state, user: i.user?.login,
      draft: i.draft, created_at: i.created_at, closed_at: i.closed_at,
      url: i.html_url, labels: i.labels?.map((l: any) => l.name),
    };

    // Fetch PR stats (additions/deletions) from the pulls API
    if (input.repo) {
      const prDetail = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/pulls/${i.number}`);
      if (prDetail) {
        pr.additions = prDetail.additions;
        pr.deletions = prDetail.deletions;
        pr.changed_files = prDetail.changed_files;
        pr.merged = prDetail.merged;
        pr.merged_at = prDetail.merged_at;
      }
    }
    results.push(pr);
  }

  return { results, count: data.total_count };
}

// ── list_github_issues ──
async function listIssues(db: Env["DB"], orgId: string, input: { repo: string; state?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const state = input.state || "open";
  const data = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/issues?state=${state}&per_page=15&sort=updated`);
  if (!data) return { results: [] };
  return {
    results: (data as any[]).filter((i: any) => !i.pull_request).map((i: any) => ({
      number: i.number, title: i.title, state: i.state, user: i.user?.login,
      labels: i.labels?.map((l: any) => l.name), assignee: i.assignee?.login,
      created_at: i.created_at, updated_at: i.updated_at,
    })),
  };
}

// ── list_github_prs ──
async function listPRs(db: Env["DB"], orgId: string, input: { repo: string; state?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const state = input.state || "open";
  const data = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/pulls?state=${state}&per_page=15&sort=updated`);
  if (!data) return { results: [] };

  const prs = (data as any[]).map((p: any) => ({
    number: p.number, title: p.title, state: p.state, user: p.user?.login,
    draft: p.draft, base: p.base?.ref, head: p.head?.ref,
    created_at: p.created_at, updated_at: p.updated_at,
    additions: p.additions, deletions: p.deletions, changed_files: p.changed_files,
  }));

  return { results: prs };
}

// ── get_github_commit ──
async function getCommit(db: Env["DB"], orgId: string, input: { repo: string; sha: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const data = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/commits/${input.sha}`);
  if (!data) return { error: "Commit not found" };

  const files = data.files?.map((f: any) => f.filename) || [];
  const graph = await enrichFiles(db, orgId, files);

  return {
    sha: data.sha, message: data.commit?.message, author: data.commit?.author?.name,
    date: data.commit?.author?.date, additions: data.stats?.additions, deletions: data.stats?.deletions,
    files_changed: files,
    graph: graph || undefined,
  };
}

// ── list_github_commits ──
async function listCommits(db: Env["DB"], orgId: string, input: { repo: string; branch?: string; author?: string; since?: string; until?: string; per_page?: number }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  let url = `https://api.github.com/repos/${input.repo}/commits?per_page=${input.per_page || 30}`;
  if (input.branch) url += `&sha=${input.branch}`;
  if (input.author) url += `&author=${encodeURIComponent(input.author)}`;
  if (input.since) url += `&since=${input.since}`;
  if (input.until) url += `&until=${input.until}`;
  const data = await ghGet(creds.token, url);
  if (!data) return { results: [] };
  return {
    results: (data as any[]).map((c: any) => ({
      sha: c.sha?.slice(0, 7), message: c.commit?.message?.split("\n")[0],
      author: c.commit?.author?.name, date: c.commit?.author?.date,
    })),
    count: (data as any[]).length,
  };
}

// ── get_github_file ──
async function getFile(db: Env["DB"], orgId: string, input: { repo: string; path: string; branch?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  let url = `https://api.github.com/repos/${input.repo}/contents/${input.path}`;
  if (input.branch) url += `?ref=${input.branch}`;
  const data = await ghGet(creds.token, url);
  if (!data) return { error: "File not found" };
  const content = data.encoding === "base64" ? atob(data.content) : data.content;
  return { path: data.path, size: data.size, content: content?.slice(0, 10000) };
}

// ── add_github_comment ──
async function addComment(db: Env["DB"], orgId: string, input: { repo: string; issue_number: number; body: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const res = await ghPost(creds.token, `https://api.github.com/repos/${input.repo}/issues/${input.issue_number}/comments`, { body: input.body });
  return { posted: res.ok, url: res.data?.html_url };
}

// ── create_github_pr ──
async function createPR(db: Env["DB"], orgId: string, input: { repo: string; title: string; body?: string; head: string; base: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const res = await ghPost(creds.token, `https://api.github.com/repos/${input.repo}/pulls`, {
    title: input.title, body: input.body || "", head: input.head, base: input.base,
  });
  return { created: res.ok, number: res.data?.number, url: res.data?.html_url };
}

// ── merge_github_pr ──
async function mergePR(db: Env["DB"], orgId: string, input: { repo: string; pull_number: number; method?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const res = await ghPut(creds.token, `https://api.github.com/repos/${input.repo}/pulls/${input.pull_number}/merge`, {
    merge_method: input.method || "squash",
  });
  return { merged: res.ok, message: res.data?.message || res.data };
}

// ── list_github_actions ──
async function listActions(db: Env["DB"], orgId: string, input: { repo: string; status?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  let url = `https://api.github.com/repos/${input.repo}/actions/runs?per_page=10`;
  if (input.status) url += `&status=${input.status}`;
  const data = await ghGet(creds.token, url);
  if (!data) return { results: [] };
  return {
    results: data.workflow_runs?.map((r: any) => ({
      id: r.id, name: r.name, status: r.status, conclusion: r.conclusion,
      branch: r.head_branch, commit: r.head_sha?.slice(0, 7),
      created_at: r.created_at, url: r.html_url,
    })) || [],
  };
}

// ── get_github_action_run ──
async function getActionRun(db: Env["DB"], orgId: string, input: { repo: string; run_id: number }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const data = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/actions/runs/${input.run_id}`);
  if (!data) return { error: "Run not found" };
  return {
    id: data.id, name: data.name, status: data.status, conclusion: data.conclusion,
    branch: data.head_branch, commit: data.head_sha?.slice(0, 7), message: data.head_commit?.message,
    created_at: data.created_at, updated_at: data.updated_at, url: data.html_url,
    duration_sec: data.run_started_at && data.updated_at
      ? Math.round((new Date(data.updated_at).getTime() - new Date(data.run_started_at).getTime()) / 1000) : null,
  };
}

// ── get_github_action_logs ──
async function getActionLogs(db: Env["DB"], orgId: string, input: { repo: string; run_id: number }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };

  // Get jobs for this run
  const jobs = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/actions/runs/${input.run_id}/jobs`);
  if (!jobs?.jobs) return { error: "No jobs found" };

  const failedJobs = jobs.jobs.filter((j: any) => j.conclusion === "failure");
  const results = [];

  for (const job of failedJobs.slice(0, 3)) {
    const failedSteps = job.steps?.filter((s: any) => s.conclusion === "failure")
      ?.map((s: any) => ({ name: s.name, number: s.number })) || [];
    results.push({
      job_name: job.name, conclusion: job.conclusion,
      failed_steps: failedSteps,
    });
  }

  return { failed_jobs: results, total_jobs: jobs.jobs.length };
}

// ── list_github_releases ──
async function listReleases(db: Env["DB"], orgId: string, input: { repo: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const data = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/releases?per_page=10`);
  if (!data) return { results: [] };
  return {
    results: (data as any[]).map((r: any) => ({
      tag: r.tag_name, name: r.name, draft: r.draft, prerelease: r.prerelease,
      published_at: r.published_at, author: r.author?.login,
    })),
  };
}

// ── list_code_scanning_alerts ──
async function listCodeAlerts(db: Env["DB"], orgId: string, input: { repo: string; state?: string }): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };
  const state = input.state || "open";
  const data = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/code-scanning/alerts?state=${state}&per_page=15`);
  if (!data || !Array.isArray(data)) return { results: [] };

  const alerts = (data as any[]).map((a: any) => ({
    number: a.number, rule: a.rule?.id, severity: a.rule?.severity,
    description: a.rule?.description, state: a.state,
    file: a.most_recent_instance?.location?.path,
    line: a.most_recent_instance?.location?.start_line,
    url: a.html_url,
  }));

  // Graph enrichment
  const files = alerts.map(a => a.file).filter(Boolean);
  const graph = await enrichFiles(db, orgId, files);

  return { results: alerts, graph: graph || undefined };
}

// ── developer_report: cross-reference GitHub PRs with Jira story points ──
async function developerReport(db: Env["DB"], orgId: string, input: {
  author: string; repo: string; since?: string; until?: string;
}): Promise<any> {
  const creds = await getGitHubCreds(db, orgId);
  if (!creds) return { error: "GitHub not connected" };

  const author = input.author;
  const since = input.since || new Date(Date.now() - 30 * 86400000).toISOString().slice(0, 10);
  const until = input.until || new Date().toISOString().slice(0, 10);

  // 1. Get all merged PRs by this author in the date range
  const q = `is:pr is:merged repo:${input.repo} author:${author} merged:${since}..${until}`;
  const searchData = await ghGet(creds.token,
    `https://api.github.com/search/issues?q=${encodeURIComponent(q)}&per_page=50&sort=created`);

  const prs: any[] = [];
  const ticketIds = new Set<string>();

  for (const item of (searchData?.items || [])) {
    // Get PR details (additions/deletions)
    const detail = await ghGet(creds.token, `https://api.github.com/repos/${input.repo}/pulls/${item.number}`);

    // Extract Jira ticket ID from PR title or branch name
    const ticketMatch = (item.title || "").match(/([A-Z]+-\d+)/);
    const ticketId = ticketMatch?.[1] || null;
    if (ticketId) ticketIds.add(ticketId);

    prs.push({
      number: item.number,
      title: item.title,
      ticket: ticketId,
      created_at: item.created_at,
      merged_at: detail?.merged_at || item.closed_at,
      additions: detail?.additions || 0,
      deletions: detail?.deletions || 0,
      changed_files: detail?.changed_files || 0,
      lines_total: (detail?.additions || 0) + (detail?.deletions || 0),
    });
  }

  // 2. Get Jira story points for each ticket
  const ticketDetails: Record<string, { points: number | null; summary: string; status: string }> = {};
  const jiraIntegration = await getIntegration(db, orgId, "jira");
  if (jiraIntegration && ticketIds.size > 0) {
    const jiraCreds = JSON.parse(jiraIntegration.credentials || "{}");
    const jiraConfig = JSON.parse(jiraIntegration.config || "{}");
    if (jiraCreds.email && jiraCreds.api_token && jiraConfig.domain) {
      const auth64 = btoa(`${jiraCreds.email}:${jiraCreds.api_token}`);
      for (const ticketId of ticketIds) {
        try {
          const res = await fetch(
            `https://${jiraConfig.domain}/rest/api/3/issue/${ticketId}?fields=summary,status,story_points,customfield_10016`,
            { headers: { Authorization: `Basic ${auth64}` }, signal: AbortSignal.timeout(5000) }
          );
          if (res.ok) {
            const issue = await res.json() as any;
            ticketDetails[ticketId] = {
              points: issue.fields?.story_points || issue.fields?.customfield_10016 || null,
              summary: issue.fields?.summary || "",
              status: issue.fields?.status?.name || "",
            };
          }
        } catch {}
      }
    }
  }

  // Also try Linear if Jira didn't find anything
  if (Object.keys(ticketDetails).length === 0 && ticketIds.size > 0) {
    const linearIntegration = await getIntegration(db, orgId, "linear");
    if (linearIntegration) {
      const linearCreds = JSON.parse(linearIntegration.credentials || "{}");
      if (linearCreds.api_key) {
        for (const ticketId of ticketIds) {
          try {
            const res = await fetch("https://api.linear.app/graphql", {
              method: "POST",
              headers: { Authorization: linearCreds.api_key, "Content-Type": "application/json" },
              body: JSON.stringify({
                query: `query { issueSearch(query: "${ticketId}", first: 1) { nodes { identifier title state { name } estimate } } }`,
              }),
              signal: AbortSignal.timeout(5000),
            });
            if (res.ok) {
              const data = await res.json() as any;
              const issue = data?.data?.issueSearch?.nodes?.[0];
              if (issue) {
                ticketDetails[ticketId] = {
                  points: issue.estimate || null,
                  summary: issue.title || "",
                  status: issue.state?.name || "",
                };
              }
            }
          } catch {}
        }
      }
    }
  }

  // 3. Build the report
  let totalAdditions = 0, totalDeletions = 0, totalFiles = 0, totalPoints = 0;
  const prDetails = prs.map(pr => {
    totalAdditions += pr.additions;
    totalDeletions += pr.deletions;
    totalFiles += pr.changed_files;
    const ticket = pr.ticket ? ticketDetails[pr.ticket] : null;
    const points = ticket?.points || null;
    if (points) totalPoints += points;
    return {
      ...pr,
      story_points: points,
      ticket_summary: ticket?.summary,
      ticket_status: ticket?.status,
      lines_per_point: points ? Math.round(pr.lines_total / points) : null,
    };
  });

  // Sort by lines_total descending
  prDetails.sort((a, b) => b.lines_total - a.lines_total);

  const avgLinesPerPR = prs.length > 0 ? Math.round((totalAdditions + totalDeletions) / prs.length) : 0;
  const avgLinesPerPoint = totalPoints > 0 ? Math.round((totalAdditions + totalDeletions) / totalPoints) : null;

  return {
    author,
    period: { since, until },
    summary: {
      total_prs: prs.length,
      total_additions: totalAdditions,
      total_deletions: totalDeletions,
      total_lines: totalAdditions + totalDeletions,
      total_files_changed: totalFiles,
      total_story_points: totalPoints || "unknown",
      avg_lines_per_pr: avgLinesPerPR,
      avg_lines_per_point: avgLinesPerPoint,
      tickets_found: ticketIds.size,
      tickets_with_points: Object.values(ticketDetails).filter(t => t.points).length,
    },
    prs: prDetails,
  };
}

// ── Dispatcher ──

export const GITHUB_TOOL_NAMES = [
  "search_github_issues", "search_github_code", "search_github_prs",
  "list_github_issues", "list_github_prs", "get_github_commit",
  "list_github_commits", "get_github_file", "add_github_comment",
  "create_github_pr", "merge_github_pr", "list_github_actions",
  "get_github_action_run", "get_github_action_logs",
  "list_github_releases", "list_code_scanning_alerts",
  "developer_report",
];

export async function executeGitHubTool(
  db: Env["DB"], orgId: string, tool: string, input: Record<string, unknown>
): Promise<any> {
  switch (tool) {
    case "search_github_issues": return searchIssues(db, orgId, input as any);
    case "search_github_code": return searchCode(db, orgId, input as any);
    case "search_github_prs": return searchPRs(db, orgId, input as any);
    case "list_github_issues": return listIssues(db, orgId, input as any);
    case "list_github_prs": return listPRs(db, orgId, input as any);
    case "get_github_commit": return getCommit(db, orgId, input as any);
    case "list_github_commits": return listCommits(db, orgId, input as any);
    case "get_github_file": return getFile(db, orgId, input as any);
    case "add_github_comment": return addComment(db, orgId, input as any);
    case "create_github_pr": return createPR(db, orgId, input as any);
    case "merge_github_pr": return mergePR(db, orgId, input as any);
    case "list_github_actions": return listActions(db, orgId, input as any);
    case "get_github_action_run": return getActionRun(db, orgId, input as any);
    case "get_github_action_logs": return getActionLogs(db, orgId, input as any);
    case "list_github_releases": return listReleases(db, orgId, input as any);
    case "list_code_scanning_alerts": return listCodeAlerts(db, orgId, input as any);
    case "developer_report": return developerReport(db, orgId, input as any);
    default: return { error: `Unknown github tool: ${tool}` };
  }
}
