import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { logUsageEvent, getMonthlyToolCallCount, getOrgById } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const query = new Hono<HonoEnv>();

const FREE_MONTHLY_CALLS = 10;

// POST /api/v1/query - Proxy a raw graph query to astra
query.post("/", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ cypher: string; graph?: string; params?: Record<string, unknown> }>();

  if (!body.cypher) {
    return c.json({ error: "invalid_request", message: "cypher query is required", status: 400 }, 400);
  }

  // Check quota
  const org = await getOrgById(c.env.DB, auth.orgId);
  const isPaid = org?.plan === "cloud" || org?.plan === "enterprise";
  const monthlyCount = await getMonthlyToolCallCount(c.env.DB, auth.orgId);

  if (!isPaid && monthlyCount >= FREE_MONTHLY_CALLS) {
    return c.json(
      {
        error: "quota_exceeded",
        message: `Free plan limited to ${FREE_MONTHLY_CALLS} calls/month. Upgrade at /api/v1/billing/checkout`,
        usage: { current: monthlyCount, limit: FREE_MONTHLY_CALLS },
        status: 429,
      },
      429
    );
  }

  const startTime = Date.now();

  try {
    const proxyRes = await fetch(`${c.env.GRAPH_PROXY_URL}/api/v1/query`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Org-Id": auth.orgId,
        "X-User-Id": auth.userId,
      },
      body: JSON.stringify({
        cypher: body.cypher,
        graph: body.graph,
        params: body.params,
      }),
    });

    const durationMs = Date.now() - startTime;

    if (!proxyRes.ok) {
      const errText = await proxyRes.text();
      return c.json(
        { error: "proxy_error", message: `Upstream error: ${proxyRes.status}`, detail: errText, status: 502 },
        502
      );
    }

    const result = await proxyRes.json<Record<string, unknown>>();

    // Log usage
    await logUsageEvent(c.env.DB, {
      id: crypto.randomUUID(),
      orgId: auth.orgId,
      userId: auth.userId,
      toolName: "raw_query",
      graphScopeId: null,
      tokensIn: 0,
      tokensOut: 0,
      durationMs,
    });

    return c.json({
      result,
      usage: {
        calls_this_month: monthlyCount + 1,
        limit: isPaid ? null : FREE_MONTHLY_CALLS,
      },
    });
  } catch (err) {
    const message = err instanceof Error ? err.message : "Unknown proxy error";
    return c.json({ error: "proxy_error", message, status: 502 }, 502);
  }
});

export default query;
