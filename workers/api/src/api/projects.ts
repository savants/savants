import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const projects = new Hono<HonoEnv>();

// ─── Source types ────────────────────────────────────────────────────────────
// Each source is isolated per project. No cross-project leakage.
//
// source_type values:
//   "github_repo"    - config: { owner, repo, branch, full_name }
//   "k8s_cluster"    - config: { cluster_name, namespace, context }
//   "k8s_namespace"  - config: { cluster_name, namespace }
//   "sentry_project" - config: { org_slug, project_slug }
//   "slack_channel"  - config: { team_id, channel_id, channel_name }
//   "local_machine"  - config: { hostname, agent_id, last_ip }

// GET /api/v1/projects - List all projects for the org
projects.get("/", async (c) => {
  const auth = c.get("auth");

  const result = await c.env.DB
    .prepare(`
      SELECT p.*,
        (SELECT COUNT(*) FROM project_members pm WHERE pm.project_id = p.id) as member_count,
        (SELECT COUNT(*) FROM project_sources ps WHERE ps.project_id = p.id AND ps.enabled = 1) as source_count
      FROM projects p
      WHERE p.org_id = ?1
      ORDER BY p.created_at DESC
    `)
    .bind(auth.orgId)
    .all();

  return c.json({ projects: result.results });
});

// POST /api/v1/projects - Create a project
projects.post("/", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ name: string; description?: string }>();

  if (!body.name) {
    return c.json({ error: "invalid_request", message: "name is required", status: 400 }, 400);
  }

  const slug = body.name.toLowerCase().replace(/[^a-z0-9-]/g, "-").replace(/-+/g, "-");
  const id = crypto.randomUUID();

  await c.env.DB
    .prepare("INSERT INTO projects (id, org_id, name, slug, description) VALUES (?1, ?2, ?3, ?4, ?5)")
    .bind(id, auth.orgId, body.name, slug, body.description || null)
    .run();

  // Add creator as project owner
  await c.env.DB
    .prepare("INSERT INTO project_members (id, project_id, user_id, role) VALUES (?1, ?2, ?3, ?4)")
    .bind(crypto.randomUUID(), id, auth.userId, "owner")
    .run();

  return c.json({ id, name: body.name, slug, created: true }, 201);
});

// GET /api/v1/projects/:id - Get project details with all sources
projects.get("/:id", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("id");

  const project = await c.env.DB
    .prepare("SELECT * FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(projectId, auth.orgId)
    .first();

  if (!project) {
    return c.json({ error: "not_found", message: "Project not found", status: 404 }, 404);
  }

  const sources = await c.env.DB
    .prepare("SELECT * FROM project_sources WHERE project_id = ?1 ORDER BY source_type, created_at")
    .bind(projectId)
    .all();

  const members = await c.env.DB
    .prepare(`
      SELECT pm.*, u.name, u.email, u.avatar_url
      FROM project_members pm
      JOIN users u ON u.id = pm.user_id
      WHERE pm.project_id = ?1
    `)
    .bind(projectId)
    .all();

  return c.json({
    project,
    sources: (sources.results as unknown as any[]).map((s) => ({
      ...s,
      config: JSON.parse(s.config || "{}"),
    })),
    members: members.results,
  });
});

// DELETE /api/v1/projects/:id - Delete a project
projects.delete("/:id", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("id");

  const result = await c.env.DB
    .prepare("DELETE FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(projectId, auth.orgId)
    .run();

  if (!result.meta.changes) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  return c.json({ deleted: true }, 200);
});

// ─── Project Sources ─────────────────────────────────────────────────────────

// POST /api/v1/projects/:id/sources - Add a data source to a project
projects.post("/:id/sources", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("id");

  // Verify project belongs to org
  const project = await c.env.DB
    .prepare("SELECT id FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(projectId, auth.orgId)
    .first();

  if (!project) {
    return c.json({ error: "not_found", message: "Project not found", status: 404 }, 404);
  }

  const body = await c.req.json<{
    source_type: string;
    config: Record<string, unknown>;
  }>();

  const validTypes = ["github_repo", "k8s_cluster", "k8s_namespace", "sentry_project", "slack_channel", "local_machine"];
  if (!validTypes.includes(body.source_type)) {
    return c.json({ error: "invalid_type", message: `Valid types: ${validTypes.join(", ")}`, status: 400 }, 400);
  }

  const id = crypto.randomUUID();
  await c.env.DB
    .prepare("INSERT INTO project_sources (id, project_id, source_type, source_config) VALUES (?1, ?2, ?3, ?4)")
    .bind(id, projectId, body.source_type, JSON.stringify(body.config))
    .run();

  return c.json({ id, source_type: body.source_type, config: body.config, created: true }, 201);
});

// DELETE /api/v1/projects/:id/sources/:sourceId - Remove a source
projects.delete("/:id/sources/:sourceId", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("id");
  const sourceId = c.req.param("sourceId");

  // Verify project belongs to org
  const project = await c.env.DB
    .prepare("SELECT id FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(projectId, auth.orgId)
    .first();

  if (!project) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  await c.env.DB
    .prepare("DELETE FROM project_sources WHERE id = ?1 AND project_id = ?2")
    .bind(sourceId, projectId)
    .run();

  return c.json({ deleted: true });
});

// ─── Project Members ─────────────────────────────────────────────────────────

// POST /api/v1/projects/:id/members - Add a member to a project
projects.post("/:id/members", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("id");
  const body = await c.req.json<{ user_id?: string; email?: string; role?: string }>();

  const project = await c.env.DB
    .prepare("SELECT id FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(projectId, auth.orgId)
    .first();

  if (!project) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  let userId = body.user_id;
  if (!userId && body.email) {
    const user = await c.env.DB
      .prepare("SELECT id FROM users WHERE email = ?1")
      .bind(body.email)
      .first<{ id: string }>();
    userId = user?.id;
  }

  if (!userId) {
    return c.json({ error: "user_not_found", message: "User not found. They must sign up first.", status: 404 }, 404);
  }

  // Verify user is in the org
  const membership = await c.env.DB
    .prepare("SELECT id FROM memberships WHERE user_id = ?1 AND org_id = ?2")
    .bind(userId, auth.orgId)
    .first();

  if (!membership) {
    return c.json({ error: "not_org_member", message: "User must be a member of the organization first.", status: 403 }, 403);
  }

  await c.env.DB
    .prepare("INSERT OR REPLACE INTO project_members (id, project_id, user_id, role) VALUES (?1, ?2, ?3, ?4)")
    .bind(crypto.randomUUID(), projectId, userId, body.role || "member")
    .run();

  return c.json({ added: true, user_id: userId, project_id: projectId });
});

// DELETE /api/v1/projects/:id/members/:userId - Remove a member
projects.delete("/:id/members/:userId", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("id");
  const userId = c.req.param("userId");

  await c.env.DB
    .prepare("DELETE FROM project_members WHERE project_id = ?1 AND user_id = ?2")
    .bind(projectId, userId)
    .run();

  return c.json({ removed: true });
});

// ─── GitHub Repos (fetch available repos for adding to project) ──────────────

// GET /api/v1/projects/github/repos - List repos the user has access to
projects.get("/github/repos", async (c) => {
  const auth = c.get("auth");

  // Get the GitHub integration for this org
  const integration = await c.env.DB
    .prepare("SELECT credentials FROM integrations WHERE org_id = ?1 AND type = ?2")
    .bind(auth.orgId, "github")
    .first<{ credentials: string }>();

  if (!integration) {
    return c.json({ error: "not_connected", message: "GitHub not connected. Sign in with GitHub first.", status: 404 }, 404);
  }

  const creds = JSON.parse(integration.credentials);
  const token = creds.access_token;

  if (!token) {
    return c.json({ error: "no_token", message: "GitHub token not available", status: 400 }, 400);
  }

  // Fetch repos from GitHub
  const repos: Array<{ full_name: string; name: string; owner: string; private: boolean; language: string | null; default_branch: string }> = [];

  // Fetch user repos (first 100)
  const res = await fetch("https://api.github.com/user/repos?per_page=100&sort=updated&type=all", {
    headers: {
      Authorization: `Bearer ${token}`,
      "User-Agent": "Savants-Cloud-API",
      Accept: "application/vnd.github.v3+json",
    },
  });

  if (res.ok) {
    const data = await res.json<Array<{ full_name: string; name: string; owner: { login: string }; private: boolean; language: string | null; default_branch: string }>>();
    for (const r of data) {
      repos.push({
        full_name: r.full_name,
        name: r.name,
        owner: r.owner.login,
        private: r.private,
        language: r.language,
        default_branch: r.default_branch,
      });
    }
  }

  return c.json({ repos });
});

export default projects;
