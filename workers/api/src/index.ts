import { Hono } from "hono";
import { cors } from "hono/cors";
import type { Env, AuthContext } from "./lib/types";
import { authMiddleware } from "./auth/middleware";
import deviceRoutes from "./auth/device";
import oauthRoutes from "./auth/oauth";
import toolsRoutes from "./api/tools";
import orgRoutes from "./api/org";
import usageRoutes from "./api/usage";
import billingRoutes from "./api/billing";
import graphsRoutes from "./api/graphs";
import queryRoutes from "./api/query";
import stripeWebhook from "./webhooks/stripe";
import githubWebhook from "./webhooks/github";
import slackWebhook from "./webhooks/slack";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const app = new Hono<HonoEnv>();

// ─── Global middleware ───────────────────────────────────────────────────────

app.use(
  "*",
  cors({
    origin: ["https://savants.cloud", "http://localhost:3000", "http://localhost:5173"],
    allowMethods: ["GET", "POST", "PUT", "DELETE", "OPTIONS"],
    allowHeaders: ["Content-Type", "Authorization", "X-Requested-With"],
    exposeHeaders: ["X-Request-Id"],
    maxAge: 86400,
    credentials: true,
  })
);

// Add request ID to all responses
app.use("*", async (c, next) => {
  const requestId = crypto.randomUUID();
  c.header("X-Request-Id", requestId);
  await next();
});

// Global error handler
app.onError((err, c) => {
  console.error(`[${c.req.method}] ${c.req.url} - Error:`, err.message);
  return c.json(
    {
      error: "internal_error",
      message: c.env.ENVIRONMENT === "production" ? "Internal server error" : err.message,
      status: 500,
    },
    500
  );
});

// ─── Health check ────────────────────────────────────────────────────────────

app.get("/", (c) => {
  return c.json({
    name: "savants-cloud-api",
    version: "1.0.0",
    status: "ok",
    docs: "https://savants.cloud/docs/api",
  });
});

app.get("/health", (c) => {
  return c.json({ status: "ok", timestamp: Math.floor(Date.now() / 1000) });
});

// ─── Public auth routes (no auth required) ───────────────────────────────────

app.route("/auth/device", deviceRoutes);
app.route("/auth", oauthRoutes);

// ─── Public API routes ──────────────────────────────────────────────────────

app.route("/api/v1/tools", toolsRoutes);

// ─── Webhook routes (verified by their own signature checks) ─────────────────

app.route("/webhooks/stripe", stripeWebhook);
app.route("/webhooks/github", githubWebhook);
app.route("/webhooks/slack", slackWebhook);

// ─── Authenticated API routes ────────────────────────────────────────────────

const api = new Hono<HonoEnv>();
api.use("*", authMiddleware());
api.route("/org", orgRoutes);
api.route("/usage", usageRoutes);
api.route("/billing", billingRoutes);
api.route("/graphs", graphsRoutes);
api.route("/query", queryRoutes);

app.route("/api/v1", api);

// ─── 404 fallback ────────────────────────────────────────────────────────────

app.notFound((c) => {
  return c.json(
    {
      error: "not_found",
      message: `Route ${c.req.method} ${new URL(c.req.url).pathname} not found`,
      status: 404,
    },
    404
  );
});

export default app;
