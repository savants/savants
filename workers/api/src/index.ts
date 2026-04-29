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
import sentryWebhook from "./webhooks/sentry";
import integrationsRoutes from "./api/integrations";
import { sentrySetupPage } from "./pages/sentry-setup";
import { dashboardPage } from "./pages/dashboard";

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
      message: err.message,
      trace: c.env.ENVIRONMENT === "production" ? undefined : err.stack,
      status: 500,
    },
    500
  );
});

// ─── Health check ────────────────────────────────────────────────────────────

app.get("/health", (c) => {
  return c.json({ status: "ok", timestamp: Math.floor(Date.now() / 1000) });
});

// ─── savants.cloud routes (dashboard / redirects) ───────────────────────────

// Root redirects to savants.dev
app.get("/", (c) => {
  const host = new URL(c.req.url).hostname;
  if (host === "savants.cloud" || host === "www.savants.cloud") {
    return c.redirect("https://savants.dev", 302);
  }
  return c.json({ name: "savants-cloud-api", version: "1.0.0", status: "ok" });
});

// Activate page (device flow landing)
app.get("/activate", (c) => {
  const code = new URL(c.req.url).searchParams.get("code") || "";
  const status = new URL(c.req.url).searchParams.get("status") || "";

  if (status === "success") {
    return c.html(`<!DOCTYPE html><html><head><meta charset="utf-8"><title>savants - Connected</title>
<style>*{margin:0;padding:0;box-sizing:border-box}body{font-family:Inter,-apple-system,sans-serif;background:#0a0a0a;color:#e5e5e5;display:flex;align-items:center;justify-content:center;min-height:100vh}
.card{text-align:center;max-width:420px;padding:48px}.icon{font-size:48px;margin-bottom:24px}.h{font-size:1.5rem;font-weight:700;margin-bottom:12px}.p{color:#737373;line-height:1.6;margin-bottom:24px}
.cmd{background:#141414;border:1px solid #262626;border-radius:10px;padding:14px 20px;font-family:'JetBrains Mono',monospace;font-size:0.9rem;color:#22d3ee}</style></head>
<body><div class="card"><div class="icon">&#10003;</div><div class="h">Connected to savants.cloud</div><p class="p">Your CLI is now authenticated. You can close this tab.</p>
<div class="cmd">savants status</div></div></body></html>`);
  }

  return c.html(`<!DOCTYPE html><html><head><meta charset="utf-8"><title>savants - Activate</title>
<style>*{margin:0;padding:0;box-sizing:border-box}body{font-family:Inter,-apple-system,sans-serif;background:#0a0a0a;color:#e5e5e5;display:flex;align-items:center;justify-content:center;min-height:100vh}
.card{text-align:center;max-width:420px;padding:48px}.h{font-size:1.5rem;font-weight:700;margin-bottom:12px}.p{color:#737373;line-height:1.6;margin-bottom:32px}
.code{font-family:'JetBrains Mono',monospace;font-size:2rem;font-weight:700;color:#22d3ee;letter-spacing:0.1em;margin-bottom:32px}
a.btn{display:inline-block;padding:12px 28px;background:linear-gradient(135deg,#22d3ee,#a78bfa);color:#0a0a0a;font-weight:600;border-radius:10px;text-decoration:none;margin:6px;transition:transform 0.2s}
a.btn:hover{transform:translateY(-2px)}</style></head>
<body><div class="card"><div class="h">Activate savants</div><p class="p">Sign in to connect your CLI to savants.cloud</p>
${code ? `<div class="code">${code}</div>` : ""}
<div><a class="btn" href="/auth/google${code ? "?user_code=" + code : ""}">Sign in with Google</a></div>
<div><a class="btn" href="/auth/github${code ? "?user_code=" + code : ""}">Sign in with GitHub</a></div>
</div></body></html>`);
});

// Sentry integration setup page
app.get("/integrations/sentry", (c) => {
  const status = new URL(c.req.url).searchParams.get("status") || undefined;
  const message = new URL(c.req.url).searchParams.get("message") || undefined;
  return c.html(sentrySetupPage(status, message));
});

// Dashboard pages (server-rendered)
app.get("/dashboard", (c) => c.html(dashboardPage()));
app.get("/dashboard/:page", (c) => {
  const page = c.req.param("page");
  return c.html(dashboardPage(page));
});
app.get("/docs", (c) => c.redirect("https://savants.dev", 302));
app.get("/docs/*", (c) => c.redirect("https://savants.dev", 302));

// ─── Public auth routes (no auth required) ───────────────────────────────────

app.route("/auth/device", deviceRoutes);
app.route("/auth", oauthRoutes);

// ─── Public API routes ──────────────────────────────────────────────────────

app.route("/api/v1/tools", toolsRoutes);

// ─── Webhook routes (verified by their own signature checks) ─────────────────

app.route("/webhooks/stripe", stripeWebhook);
app.route("/webhooks/github", githubWebhook);
app.route("/webhooks/slack", slackWebhook);
app.route("/webhooks/sentry", sentryWebhook);

// ─── Authenticated API routes ────────────────────────────────────────────────

const api = new Hono<HonoEnv>();
api.use("*", authMiddleware());
api.route("/org", orgRoutes);
api.route("/usage", usageRoutes);
api.route("/billing", billingRoutes);
api.route("/graphs", graphsRoutes);
api.route("/query", queryRoutes);
api.route("/integrations", integrationsRoutes);

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
