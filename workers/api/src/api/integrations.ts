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

export default integrations;
