/**
 * Agent API: registration, heartbeat, query routing.
 *
 * Flow:
 * 1. Agent on server: POST /agents/register (sends hostname, capabilities)
 * 2. Agent polls:     GET  /agents/poll     (gets pending queries)
 * 3. Agent responds:  POST /agents/result   (sends query result)
 * 4. User via MCP:    POST /agents/query    (sends query, waits for result)
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const agents = new Hono<HonoEnv>();

// POST /agents/register - Agent registers itself
agents.post("/register", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    name: string;
    hostname?: string;
    os?: string;
    arch?: string;
    capabilities?: string[];
    version?: string;
  }>();

  if (!body.name) return c.json({ error: "name required" }, 400);

  const now = Math.floor(Date.now() / 1000);
  const caps = JSON.stringify(body.capabilities || ["host_health", "pod_status", "pod_logs"]);
  const machineId = (body as any).machine_id;

  // Upsert: match by machine_id (unique per host), fallback to name+org
  const existing = machineId
    ? await c.env.DB.prepare(
        "SELECT id FROM agents WHERE org_id = ?1 AND hostname = ?2 LIMIT 1"
      ).bind(auth.orgId, machineId).first<{ id: string }>()
    : await c.env.DB.prepare(
        "SELECT id FROM agents WHERE org_id = ?1 AND name = ?2 LIMIT 1"
      ).bind(auth.orgId, body.name).first<{ id: string }>();

  let agentId: string;
  if (existing) {
    agentId = existing.id;
    await c.env.DB.prepare(`
      UPDATE agents SET name = ?1, hostname = ?2, os = ?3, arch = ?4, capabilities = ?5,
        version = ?6, last_heartbeat = ?7, status = 'online'
      WHERE id = ?8
    `).bind(
      body.name, machineId || body.hostname || null, body.os || null, body.arch || null,
      caps, body.version || null, now, agentId
    ).run();
  } else {
    agentId = crypto.randomUUID();
    await c.env.DB.prepare(`
      INSERT INTO agents (id, org_id, name, hostname, os, arch, capabilities, version, last_heartbeat, status, created_at)
      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'online', ?9)
    `).bind(
      agentId, auth.orgId, body.name,
      machineId || body.hostname || null, body.os || null, body.arch || null,
      caps, body.version || null, now
    ).run();
  }

  return c.json({ agent_id: agentId, status: "registered" });
});

// GET /agents - List registered agents
agents.get("/", async (c) => {
  const auth = c.get("auth");
  const result = await c.env.DB.prepare(
    "SELECT id, name, hostname, os, arch, capabilities, last_heartbeat, version, status FROM agents WHERE org_id = ?1 ORDER BY last_heartbeat DESC"
  ).bind(auth.orgId).all();

  return c.json({
    agents: result.results.map((a: any) => ({
      ...a,
      capabilities: JSON.parse(a.capabilities || "[]"),
      online: a.last_heartbeat && (Math.floor(Date.now() / 1000) - a.last_heartbeat) < 120,
    })),
  });
});

// POST /agents/heartbeat - Agent sends heartbeat
agents.post("/heartbeat", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ agent_id: string }>();
  const now = Math.floor(Date.now() / 1000);

  await c.env.DB.prepare(
    "UPDATE agents SET last_heartbeat = ?1, status = 'online' WHERE id = ?2 AND org_id = ?3"
  ).bind(now, body.agent_id, auth.orgId).run();

  return c.json({ ok: true });
});

// GET /agents/poll - Agent polls for pending queries
agents.get("/poll", async (c) => {
  const auth = c.get("auth");
  const agentId = c.req.query("agent_id");
  if (!agentId) return c.json({ error: "agent_id required" }, 400);

  // Update heartbeat
  const now = Math.floor(Date.now() / 1000);
  await c.env.DB.prepare(
    "UPDATE agents SET last_heartbeat = ?1, status = 'online' WHERE id = ?2 AND org_id = ?3"
  ).bind(now, agentId, auth.orgId).run();

  // Get pending queries
  const result = await c.env.DB.prepare(
    "SELECT id, tool, input FROM agent_queries WHERE agent_id = ?1 AND status = 'pending' ORDER BY created_at LIMIT 10"
  ).bind(agentId).all();

  return c.json({ queries: result.results });
});

// POST /agents/result - Agent sends query result
agents.post("/result", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ query_id: string; result: unknown }>();
  const now = Math.floor(Date.now() / 1000);

  await c.env.DB.prepare(
    "UPDATE agent_queries SET status = 'completed', result = ?1, completed_at = ?2 WHERE id = ?3 AND org_id = ?4"
  ).bind(JSON.stringify(body.result), now, body.query_id, auth.orgId).run();

  return c.json({ ok: true });
});

// POST /agents/query - User sends a query to an agent (via MCP tool call)
agents.post("/query", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    agent_id?: string;
    agent_name?: string;
    tool: string;
    input?: Record<string, unknown>;
  }>();

  if (!body.tool) return c.json({ error: "tool required" }, 400);

  // Find agent by ID or name
  let agentId = body.agent_id;
  if (!agentId && body.agent_name) {
    const agent = await c.env.DB.prepare(
      "SELECT id FROM agents WHERE org_id = ?1 AND name = ?2 AND status = 'online' ORDER BY last_heartbeat DESC LIMIT 1"
    ).bind(auth.orgId, body.agent_name).first<{ id: string }>();
    agentId = agent?.id;
  }

  // Fallback: pick any online agent
  if (!agentId) {
    const agent = await c.env.DB.prepare(
      "SELECT id FROM agents WHERE org_id = ?1 AND status = 'online' ORDER BY last_heartbeat DESC LIMIT 1"
    ).bind(auth.orgId).first<{ id: string }>();
    agentId = agent?.id;
  }

  if (!agentId) {
    return c.json({
      error: "no_agent",
      message: "No online agents. Install savants on your server: curl -fsSL savants.sh | sh && savants agent start",
    }, 404);
  }

  // Create pending query
  const queryId = crypto.randomUUID();
  await c.env.DB.prepare(
    "INSERT INTO agent_queries (id, org_id, agent_id, tool, input, status) VALUES (?1, ?2, ?3, ?4, ?5, 'pending')"
  ).bind(queryId, auth.orgId, agentId, body.tool, JSON.stringify(body.input || {})).run();

  // Poll for result (long-poll up to 30s)
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const result = await c.env.DB.prepare(
      "SELECT result, status FROM agent_queries WHERE id = ?1 AND status = 'completed'"
    ).bind(queryId).first<{ result: string; status: string }>();

    if (result) {
      return c.json({
        tool: body.tool,
        agent_id: agentId,
        result: JSON.parse(result.result || "{}"),
      });
    }

    // Wait 500ms before polling again
    await new Promise(r => setTimeout(r, 500));
  }

  // Timeout
  await c.env.DB.prepare(
    "UPDATE agent_queries SET status = 'timeout' WHERE id = ?1"
  ).bind(queryId).run();

  return c.json({
    error: "timeout",
    message: "Agent did not respond within 30s. It may be offline or overloaded.",
  }, 504);
});

// GET /agents/events - Recent agent findings (for dashboard/API consumers)
agents.get("/events", async (c) => {
  const auth = c.get("auth");
  const limit = parseInt(c.req.query("limit") || "20");
  const since = c.req.query("since"); // unix timestamp

  let query = `
    SELECT id, action, resource_id, metadata, created_at
    FROM audit_log
    WHERE org_id = ?1 AND action = 'agent.notify'
    ORDER BY created_at DESC LIMIT ?2
  `;
  const params: unknown[] = [auth.orgId, limit];

  if (since) {
    query = `
      SELECT id, action, resource_id, metadata, created_at
      FROM audit_log
      WHERE org_id = ?1 AND action = 'agent.notify' AND created_at > ?3
      ORDER BY created_at DESC LIMIT ?2
    `;
    params.push(parseInt(since));
  }

  const result = await c.env.DB.prepare(query).bind(...params).all();

  const events = (result.results as any[]).map(r => {
    const meta = JSON.parse(r.metadata || "{}");
    return {
      id: r.id,
      severity: meta.severity,
      category: meta.category,
      title: meta.title,
      message: meta.message,
      agent: meta.agent_name,
      timestamp: r.created_at,
    };
  });

  return c.json({ events });
});

// POST /agents/notify - Agent sends a finding, cloud routes to notification channels
agents.post("/notify", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    agent_id: string;
    agent_name: string;
    severity: string;
    category: string;
    title: string;
    message: string;
    key: string;
    metadata?: Record<string, unknown>;
  }>();

  const now = Math.floor(Date.now() / 1000);

  // Store as audit log entry (no project FK needed)
  await c.env.DB.prepare(`
    INSERT INTO audit_log (id, org_id, actor_id, action, resource_type, resource_id, metadata, ip, user_agent, created_at)
    VALUES (?1, ?2, ?3, 'agent.notify', 'agent', ?4, ?5, '', '', ?6)
  `).bind(
    crypto.randomUUID(),
    auth.orgId,
    body.agent_id,
    body.agent_id,
    JSON.stringify({ severity: body.severity, category: body.category, title: body.title, message: body.message, key: body.key, agent_name: body.agent_name, ...body.metadata }),
    now,
  ).run();

  // Route to notification channels
  const integrations = await c.env.DB.prepare(
    "SELECT type, config FROM integrations WHERE org_id = ?1 AND type IN ('slack', 'gotify', 'pagerduty', 'webhook')"
  ).bind(auth.orgId).all();

  const notifications: Promise<void>[] = [];

  for (const integration of integrations.results as any[]) {
    const config = JSON.parse(integration.config || "{}");

    if (integration.type === "gotify" && config.url && config.token) {
      notifications.push(
        fetch(`${config.url}/message`, {
          method: "POST",
          headers: { "Content-Type": "application/json", "X-Gotify-Key": config.token },
          body: JSON.stringify({
            title: `[${body.severity}] ${body.title}`,
            message: `${body.message}\n\nAgent: ${body.agent_name}\nCategory: ${body.category}`,
            priority: body.severity === "critical" ? 8 : 4,
          }),
        }).then(() => {})
      );
    }

    if (integration.type === "slack" && config.webhook_url) {
      const emoji = body.severity === "critical" ? ":rotating_light:" : ":warning:";
      notifications.push(
        fetch(config.webhook_url, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            text: `${emoji} *${body.title}*\n${body.message}\n_Agent: ${body.agent_name}_`,
          }),
        }).then(() => {})
      );
    }

    if (integration.type === "webhook" && config.url) {
      notifications.push(
        fetch(config.url, {
          method: "POST",
          headers: { "Content-Type": "application/json", ...(config.headers || {}) },
          body: JSON.stringify({
            severity: body.severity,
            category: body.category,
            title: body.title,
            message: body.message,
            agent: body.agent_name,
            metadata: body.metadata,
            timestamp: now,
          }),
        }).then(() => {})
      );
    }
  }

  await Promise.allSettled(notifications);

  return c.json({ ok: true, notified: integrations.results.length });
});

export default agents;
