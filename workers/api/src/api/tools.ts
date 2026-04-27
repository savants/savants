import { Hono } from "hono";
import type { Env, AuthContext, ToolDefinition } from "../lib/types";
import { getMonthlyToolCallCount, logUsageEvent, getOrgById } from "../db/queries";
import { authMiddleware } from "../auth/middleware";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const tools = new Hono<HonoEnv>();

// POST /call requires auth
tools.use("/call", authMiddleware());

const FREE_MONTHLY_CALLS = 10;

const TOOL_LIST: ToolDefinition[] = [
  {
    name: "diagnose_error",
    description: "Diagnose an error using the code knowledge graph. Traces call chains, identifies root cause, suggests fix.",
    input_schema: {
      type: "object",
      properties: {
        error_message: { type: "string", description: "The error message or stack trace" },
        file_path: { type: "string", description: "Optional file path for context" },
      },
      required: ["error_message"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 5 },
  },
  {
    name: "pr_risk",
    description: "Analyze a pull request for risk. Scores blast radius, identifies affected downstream consumers.",
    input_schema: {
      type: "object",
      properties: {
        diff: { type: "string", description: "Unified diff of the PR" },
        base_branch: { type: "string", description: "Base branch name" },
      },
      required: ["diff"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 10 },
  },
  {
    name: "explain_symbol",
    description: "Explain what a function, class, or module does based on the knowledge graph.",
    input_schema: {
      type: "object",
      properties: {
        symbol: { type: "string", description: "Fully qualified symbol name" },
      },
      required: ["symbol"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 2 },
  },
  {
    name: "find_callers",
    description: "Find all callers of a function or method across the codebase graph.",
    input_schema: {
      type: "object",
      properties: {
        symbol: { type: "string", description: "Function or method name to search for" },
        depth: { type: "number", description: "Max depth of caller chain (default 3)" },
      },
      required: ["symbol"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 2 },
  },
  {
    name: "refactor_impact",
    description: "Predict the impact of renaming or moving a symbol. Lists all files and tests affected.",
    input_schema: {
      type: "object",
      properties: {
        symbol: { type: "string", description: "Symbol to refactor" },
        action: { type: "string", enum: ["rename", "move", "delete"], description: "Type of refactor" },
      },
      required: ["symbol", "action"],
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 5 },
  },
  {
    name: "unanswered_questions",
    description: "Surface unanswered questions and open issues from Slack, email, and other communication channels.",
    input_schema: {
      type: "object",
      properties: {
        channel: { type: "string", description: "Channel or source to search (optional)" },
        since_hours: { type: "number", description: "Look back N hours (default 24)" },
      },
    },
    pricing: { free_monthly_calls: FREE_MONTHLY_CALLS, overage_per_call_cents: 5 },
  },
];

// GET /api/v1/tools - Return tool list
tools.get("/", async (c) => {
  return c.json({ tools: TOOL_LIST });
});

// POST /api/v1/tools/call - Proxy a tool call to astra
tools.post("/call", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ tool: string; input: Record<string, unknown> }>();

  if (!body.tool || !body.input) {
    return c.json({ error: "invalid_request", message: "tool and input are required", status: 400 }, 400);
  }

  const toolDef = TOOL_LIST.find((t) => t.name === body.tool);
  if (!toolDef) {
    return c.json({ error: "unknown_tool", message: `Tool '${body.tool}' not found`, status: 404 }, 404);
  }

  // Check quota
  const org = await getOrgById(c.env.DB, auth.orgId);
  const isPaid = org?.plan === "cloud" || org?.plan === "enterprise";
  const monthlyCount = await getMonthlyToolCallCount(c.env.DB, auth.orgId);

  if (!isPaid && monthlyCount >= FREE_MONTHLY_CALLS) {
    return c.json(
      {
        error: "quota_exceeded",
        message: `Free plan limited to ${FREE_MONTHLY_CALLS} tool calls/month. Upgrade to Cloud at /api/v1/billing/checkout`,
        usage: { current: monthlyCount, limit: FREE_MONTHLY_CALLS },
        status: 429,
      },
      429
    );
  }

  // Proxy to astra
  const startTime = Date.now();
  let proxyResult: Record<string, unknown>;

  try {
    const proxyRes = await fetch(`${c.env.GRAPH_PROXY_URL}/api/v1/tools/call`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Org-Id": auth.orgId,
        "X-User-Id": auth.userId,
      },
      body: JSON.stringify({ tool: body.tool, input: body.input }),
    });

    if (!proxyRes.ok) {
      const errText = await proxyRes.text();
      return c.json(
        { error: "proxy_error", message: `Upstream error: ${proxyRes.status}`, detail: errText, status: 502 },
        502
      );
    }

    proxyResult = await proxyRes.json<Record<string, unknown>>();
  } catch (err) {
    const message = err instanceof Error ? err.message : "Unknown proxy error";
    return c.json({ error: "proxy_error", message, status: 502 }, 502);
  }

  const durationMs = Date.now() - startTime;

  // Log usage
  await logUsageEvent(c.env.DB, {
    id: crypto.randomUUID(),
    orgId: auth.orgId,
    userId: auth.userId,
    toolName: body.tool,
    graphScopeId: null,
    tokensIn: (proxyResult.tokens_in as number) ?? 0,
    tokensOut: (proxyResult.tokens_out as number) ?? 0,
    durationMs,
  });

  return c.json({
    tool: body.tool,
    result: proxyResult,
    usage: {
      calls_this_month: monthlyCount + 1,
      limit: isPaid ? null : FREE_MONTHLY_CALLS,
    },
  });
});

export default tools;
