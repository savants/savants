import { Hono } from "hono";
import type { Env, AuthContext, SentryConfig, SentryCredentials } from "../lib/types";
import { getIntegration, listIntegrations, upsertIntegration, deleteIntegration } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const integrations = new Hono<HonoEnv>();

// GET /api/v1/integrations - list all integrations for the org
integrations.get("/", async (c) => {
  const auth = c.get("auth");
  const rows = await listIntegrations(c.env.DB, auth.orgId);

  // Strip credentials from the response
  const safe = rows.map((row) => ({
    id: row.id,
    type: row.type,
    config: JSON.parse(row.config),
    enabled: row.enabled === 1,
    created_at: row.created_at,
    updated_at: row.updated_at,
  }));

  return c.json({ integrations: safe });
});

// GET /api/v1/integrations/sentry - get Sentry integration details
integrations.get("/sentry", async (c) => {
  const auth = c.get("auth");
  const row = await getIntegration(c.env.DB, auth.orgId, "sentry");

  if (!row) {
    return c.json({ error: "not_found", message: "No Sentry integration configured", status: 404 }, 404);
  }

  const config = JSON.parse(row.config) as SentryConfig;
  const creds = JSON.parse(row.credentials) as SentryCredentials;

  return c.json({
    id: row.id,
    type: row.type,
    config: {
      org_slug: config.org_slug,
      project_slugs: config.project_slugs ?? [],
      auto_diagnose: config.auto_diagnose,
      slack_channel: config.slack_channel ?? null,
    },
    has_auth_token: !!creds.auth_token,
    has_client_secret: !!creds.client_secret,
    enabled: row.enabled === 1,
    webhook_url: "https://api.savants.cloud/webhooks/sentry",
    created_at: row.created_at,
    updated_at: row.updated_at,
  });
});

// POST /api/v1/integrations/sentry - create or update Sentry integration
integrations.post("/sentry", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    auth_token: string;
    org_slug: string;
    client_secret: string;
    slack_channel?: string;
    auto_diagnose?: boolean;
    project_slugs?: string[];
  }>();

  if (!body.auth_token || !body.org_slug) {
    return c.json(
      { error: "invalid_request", message: "auth_token and org_slug are required", status: 400 },
      400
    );
  }

  // Validate the token by calling Sentry API
  let sentryOrgName = "";
  try {
    const verifyRes = await fetch(`https://sentry.io/api/0/organizations/${body.org_slug}/`, {
      headers: {
        Authorization: `Bearer ${body.auth_token}`,
        "Content-Type": "application/json",
      },
    });

    if (!verifyRes.ok) {
      const errText = await verifyRes.text();
      return c.json(
        {
          error: "sentry_auth_failed",
          message: `Could not authenticate with Sentry: ${verifyRes.status}`,
          detail: errText.substring(0, 500),
          status: 400,
        },
        400
      );
    }

    const orgData = await verifyRes.json<{ name?: string; slug?: string }>();
    sentryOrgName = orgData.name ?? body.org_slug;
  } catch (err) {
    const message = err instanceof Error ? err.message : "Unknown error";
    return c.json(
      { error: "sentry_connection_failed", message: `Could not reach Sentry API: ${message}`, status: 502 },
      502
    );
  }

  const config: SentryConfig = {
    org_slug: body.org_slug,
    project_slugs: body.project_slugs ?? [],
    auto_diagnose: body.auto_diagnose !== false,
    slack_channel: body.slack_channel,
  };

  const credentials: SentryCredentials = {
    auth_token: body.auth_token,
    client_secret: body.client_secret,
  };

  const integration = await upsertIntegration(c.env.DB, {
    id: crypto.randomUUID(),
    orgId: auth.orgId,
    type: "sentry",
    config: JSON.stringify(config),
    credentials: JSON.stringify(credentials),
  });

  return c.json({
    id: integration.id,
    type: "sentry",
    sentry_org: sentryOrgName,
    config: {
      org_slug: config.org_slug,
      project_slugs: config.project_slugs,
      auto_diagnose: config.auto_diagnose,
      slack_channel: config.slack_channel,
    },
    webhook_url: "https://api.savants.cloud/webhooks/sentry",
    enabled: true,
    created_at: integration.created_at,
    updated_at: integration.updated_at,
  });
});

// DELETE /api/v1/integrations/sentry - remove Sentry integration
integrations.delete("/sentry", async (c) => {
  const auth = c.get("auth");

  const existing = await getIntegration(c.env.DB, auth.orgId, "sentry");
  if (!existing) {
    return c.json({ error: "not_found", message: "No Sentry integration configured", status: 404 }, 404);
  }

  await deleteIntegration(c.env.DB, auth.orgId, "sentry");

  return c.json({ deleted: true, type: "sentry" });
});

// POST /api/v1/integrations/sentry/test - test the Sentry connection
integrations.post("/sentry/test", async (c) => {
  const auth = c.get("auth");

  const row = await getIntegration(c.env.DB, auth.orgId, "sentry");
  if (!row) {
    return c.json({ error: "not_found", message: "No Sentry integration configured", status: 404 }, 404);
  }

  const config = JSON.parse(row.config) as SentryConfig;
  const creds = JSON.parse(row.credentials) as SentryCredentials;

  // Test 1: Verify org access
  let orgOk = false;
  try {
    const orgRes = await fetch(`https://sentry.io/api/0/organizations/${config.org_slug}/`, {
      headers: {
        Authorization: `Bearer ${creds.auth_token}`,
        "Content-Type": "application/json",
      },
    });
    orgOk = orgRes.ok;
  } catch {
    orgOk = false;
  }

  // Test 2: Fetch recent events
  let eventCount = 0;
  let projectCount = 0;

  try {
    const projectsRes = await fetch(
      `https://sentry.io/api/0/organizations/${config.org_slug}/projects/`,
      {
        headers: {
          Authorization: `Bearer ${creds.auth_token}`,
          "Content-Type": "application/json",
        },
      }
    );

    if (projectsRes.ok) {
      const projects = await projectsRes.json<Array<{ slug: string; id: string }>>();
      projectCount = projects.length;

      // Fetch recent events from the first project (or first configured project)
      const targetProject = config.project_slugs?.[0] ?? projects[0]?.slug;
      if (targetProject) {
        const eventsRes = await fetch(
          `https://sentry.io/api/0/projects/${config.org_slug}/${targetProject}/events/?per_page=5`,
          {
            headers: {
              Authorization: `Bearer ${creds.auth_token}`,
              "Content-Type": "application/json",
            },
          }
        );

        if (eventsRes.ok) {
          const events = await eventsRes.json<Array<Record<string, unknown>>>();
          eventCount = events.length;
        }
      }
    }
  } catch {
    // Non-fatal
  }

  return c.json({
    ok: orgOk,
    org_slug: config.org_slug,
    org_accessible: orgOk,
    projects_found: projectCount,
    recent_events: eventCount,
    webhook_url: "https://api.savants.cloud/webhooks/sentry",
  });
});

// ── Generic integration CRUD (works for any type) ──

// POST /api/v1/integrations/:type - create/update any integration
integrations.post("/:type", async (c) => {
  const auth = c.get("auth");
  const integrationType = c.req.param("type");
  const body = await c.req.json<Record<string, unknown>>();

  const validTypes = ["slack", "github", "linear", "jira", "gotify", "pagerduty", "opsgenie", "webhook"];
  if (!validTypes.includes(integrationType)) {
    return c.json({ error: "invalid_type", message: `Supported: ${validTypes.join(", ")}` }, 400);
  }

  // Extract config vs credentials based on type
  let config: Record<string, unknown> = {};
  let credentials: Record<string, unknown> = {};

  switch (integrationType) {
    case "slack":
      if (!body.bot_token) return c.json({ error: "bot_token required" }, 400);
      credentials = { bot_token: body.bot_token };
      config = { channels: body.channels || [], team_name: body.team_name || "" };
      // Validate token
      try {
        const res = await fetch("https://slack.com/api/auth.test", {
          headers: { Authorization: `Bearer ${body.bot_token}` },
        });
        const data = await res.json<{ ok: boolean; team?: string; error?: string }>();
        if (!data.ok) return c.json({ error: "slack_auth_failed", message: data.error }, 400);
        config.team_name = data.team || "";
      } catch (e) {
        return c.json({ error: "slack_unreachable" }, 502);
      }
      break;

    case "github":
      if (!body.token) return c.json({ error: "token required" }, 400);
      credentials = { token: body.token };
      config = { org: body.org || "", repos: body.repos || [] };
      break;

    case "linear":
      if (!body.api_key) return c.json({ error: "api_key required" }, 400);
      credentials = { api_key: body.api_key };
      config = { team_id: body.team_id || "" };
      // Validate
      try {
        const res = await fetch("https://api.linear.app/graphql", {
          method: "POST",
          headers: { Authorization: body.api_key as string, "Content-Type": "application/json" },
          body: JSON.stringify({ query: "{ viewer { id name } }" }),
        });
        const data = await res.json<{ data?: { viewer?: { name: string } } }>();
        if (!data.data?.viewer) return c.json({ error: "linear_auth_failed" }, 400);
        config.user_name = data.data.viewer.name;
      } catch {
        return c.json({ error: "linear_unreachable" }, 502);
      }
      break;

    case "jira":
      if (!body.email || !body.api_token || !body.domain) return c.json({ error: "email, api_token, domain required" }, 400);
      credentials = { email: body.email, api_token: body.api_token };
      config = { domain: body.domain, project_key: body.project_key || "" };
      break;

    case "gotify":
      if (!body.url || !body.token) return c.json({ error: "url and token required" }, 400);
      config = { url: body.url, token: body.token };
      break;

    case "pagerduty":
      if (!body.routing_key) return c.json({ error: "routing_key required" }, 400);
      config = { routing_key: body.routing_key, service_name: body.service_name || "" };
      break;

    case "opsgenie":
      if (!body.api_key) return c.json({ error: "api_key required" }, 400);
      credentials = { api_key: body.api_key };
      config = { team: body.team || "" };
      break;

    case "webhook":
      if (!body.url) return c.json({ error: "url required" }, 400);
      config = { url: body.url, headers: body.headers || {} };
      break;
  }

  const integration = await upsertIntegration(c.env.DB, {
    id: crypto.randomUUID(),
    orgId: auth.orgId,
    type: integrationType,
    config: JSON.stringify({ ...config, ...credentials }),
    credentials: JSON.stringify(credentials),
  });

  return c.json({
    id: integration.id,
    type: integrationType,
    config,
    enabled: true,
  });
});

// DELETE /api/v1/integrations/:type
integrations.delete("/:type", async (c) => {
  const auth = c.get("auth");
  const integrationType = c.req.param("type");

  // Don't match "sentry" - that has its own handler above
  if (integrationType === "sentry") return c.notFound();

  const existing = await getIntegration(c.env.DB, auth.orgId, integrationType);
  if (!existing) return c.json({ error: "not_found" }, 404);

  await deleteIntegration(c.env.DB, auth.orgId, integrationType);
  return c.json({ deleted: true, type: integrationType });
});

// POST /api/v1/integrations/:type/test - test any integration
integrations.post("/:type/test", async (c) => {
  const auth = c.get("auth");
  const integrationType = c.req.param("type");

  const row = await getIntegration(c.env.DB, auth.orgId, integrationType);
  if (!row) return c.json({ error: "not_found" }, 404);

  const config = JSON.parse(row.config || "{}");
  const creds = JSON.parse(row.credentials || "{}");
  let ok = false;
  let detail = "";

  switch (integrationType) {
    case "slack": {
      try {
        const res = await fetch("https://slack.com/api/auth.test", {
          headers: { Authorization: `Bearer ${creds.bot_token}` },
        });
        const data = await res.json<{ ok: boolean; team?: string }>();
        ok = !!data.ok;
        detail = data.team || "";
      } catch { ok = false; }
      break;
    }
    case "github": {
      try {
        const res = await fetch("https://api.github.com/user", {
          headers: { Authorization: `Bearer ${creds.token}`, "User-Agent": "savants" },
        });
        ok = res.ok;
      } catch { ok = false; }
      break;
    }
    case "linear": {
      try {
        const res = await fetch("https://api.linear.app/graphql", {
          method: "POST",
          headers: { Authorization: creds.api_key, "Content-Type": "application/json" },
          body: JSON.stringify({ query: "{ viewer { id } }" }),
        });
        const data = await res.json<{ data?: unknown }>();
        ok = !!data.data;
      } catch { ok = false; }
      break;
    }
    case "jira": {
      try {
        const auth64 = btoa(`${creds.email}:${creds.api_token}`);
        const res = await fetch(`https://${config.domain}/rest/api/3/myself`, {
          headers: { Authorization: `Basic ${auth64}` },
        });
        ok = res.ok;
      } catch { ok = false; }
      break;
    }
    default:
      ok = true;
      detail = "No validation available for this type";
  }

  return c.json({ ok, type: integrationType, detail });
});

export default integrations;
