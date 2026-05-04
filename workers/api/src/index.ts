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
import creditsRoutes from "./api/credits";
import projectsRoutes from "./api/projects";
import graphRoutes from "./api/graph";
import auditRoutes from "./api/audit";
import transfersRoutes from "./api/transfers";
import docsRoutes from "./api/docs";
import docsUploadRoutes from "./api/docs-upload";
import docsSearchRoutes from "./api/docs-search";
import docsIndexerRoutes from "./api/docs-indexer";
import agentsRoutes from "./api/agents";
import { sentrySetupPage } from "./pages/sentry-setup";
import { githubSetupPage } from "./pages/github-setup";
import { dashboardPage, projectDetailPage } from "./pages/dashboard";

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

// Root: savants.cloud goes to activate, api.savants.cloud shows API info
app.get("/", (c) => {
  const host = new URL(c.req.url).hostname;
  if (host === "savants.cloud" || host === "www.savants.cloud") {
    return c.redirect("/activate", 302);
  }
  return c.json({ name: "savants-cloud-api", version: "1.0.0", status: "ok" });
});

// Activate page (device flow landing + Get Started)
app.get("/activate", (c) => {
  const code = new URL(c.req.url).searchParams.get("code") || "";
  const status = new URL(c.req.url).searchParams.get("status") || "";
  const token = new URL(c.req.url).searchParams.get("token") || "";
  const isSuccess = status === "success";
  const isGetStarted = !code && !isSuccess;

  return c.html(`<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>savants${isSuccess ? " - Connected" : isGetStarted ? " - Get Started" : " - Activate"}</title>
<link rel="icon" type="image/svg+xml" href="https://savants.dev/favicon.svg">
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700;800&family=JetBrains+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:'Inter',system-ui,sans-serif;background:#0a0a0a;color:#e5e5e5;min-height:100vh;overflow:hidden;display:flex;align-items:center;justify-content:center}

/* Film grain */
body::after{content:'';position:fixed;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:9998;opacity:0.03;
background-image:url("data:image/svg+xml,%3Csvg viewBox='0 0 256 256' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noise'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='4' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noise)'/%3E%3C/svg%3E")}

/* Aurora */
.aurora{position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);width:140%;height:140%;pointer-events:none;z-index:0;filter:blur(120px);opacity:0.12}
.orb{position:absolute;border-radius:50%}
.orb-1{width:500px;height:500px;top:10%;left:20%;background:radial-gradient(circle,#22d3ee 0%,transparent 70%);animation:float1 8s ease-in-out infinite}
.orb-2{width:400px;height:400px;top:30%;right:15%;background:radial-gradient(circle,#a78bfa 0%,transparent 70%);animation:float2 10s ease-in-out infinite}
.orb-3{width:350px;height:350px;bottom:15%;left:35%;background:radial-gradient(circle,#7c3aed 0%,transparent 70%);animation:float1 12s ease-in-out infinite reverse}
@keyframes float1{0%,100%{transform:translate(0,0) scale(1)}50%{transform:translate(30px,-40px) scale(1.05)}}
@keyframes float2{0%,100%{transform:translate(0,0) scale(1)}50%{transform:translate(-20px,30px) scale(0.95)}}

/* Constellation */
canvas{position:fixed;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:1}

/* Content */
.page{position:relative;z-index:10;text-align:center;max-width:440px;padding:32px 24px;animation:fadeUp 0.6s ease-out}
@keyframes fadeUp{from{opacity:0;transform:translateY(20px)}to{opacity:1;transform:translateY(0)}}

.brand{font-size:1.1rem;font-weight:700;color:#e5e5e5;margin-bottom:48px;letter-spacing:-0.02em;display:block;text-decoration:none}
.brand span{color:#22d3ee}

.title{font-size:clamp(1.5rem,4vw,2rem);font-weight:800;letter-spacing:-0.03em;margin-bottom:12px;background:linear-gradient(135deg,#e5e5e5 0%,#a3a3a3 100%);-webkit-background-clip:text;-webkit-text-fill-color:transparent;background-clip:text}
.subtitle{color:#737373;font-size:0.95rem;line-height:1.6;margin-bottom:36px;max-width:340px;margin-left:auto;margin-right:auto}

.device-code{font-family:'JetBrains Mono',monospace;font-size:2.2rem;font-weight:700;color:#22d3ee;letter-spacing:0.15em;margin-bottom:36px;text-shadow:0 0 40px rgba(34,211,238,0.3)}

/* OAuth buttons */
.oauth-btn{display:flex;align-items:center;justify-content:center;gap:12px;width:100%;padding:14px 24px;border-radius:12px;font-size:0.95rem;font-weight:600;cursor:pointer;border:none;text-decoration:none;transition:all 0.2s;margin-bottom:12px}
.oauth-google{background:#fff;color:#1a1a1a}
.oauth-google:hover{background:#f0f0f0;transform:translateY(-1px);box-shadow:0 8px 30px rgba(255,255,255,0.1)}
.oauth-github{background:#e5e5e5;color:#0a0a0a}
.oauth-github:hover{background:#d4d4d4;transform:translateY(-1px);box-shadow:0 8px 30px rgba(255,255,255,0.1)}
.oauth-btn svg{width:20px;height:20px;flex-shrink:0}

.divider{display:flex;align-items:center;gap:16px;margin:24px 0;color:#525252;font-size:0.8rem}
.divider::before,.divider::after{content:'';flex:1;height:1px;background:#262626}

.install-hint{background:#141414;border:1px solid #262626;border-radius:12px;padding:16px 20px;margin-top:24px}
.install-hint code{font-family:'JetBrains Mono',monospace;font-size:0.85rem;color:#22d3ee}
.install-hint p{color:#525252;font-size:0.75rem;margin-top:8px}

.footer-text{color:#525252;font-size:0.75rem;margin-top:24px;line-height:1.6}
.footer-text a{color:#737373;text-decoration:none}
.footer-text a:hover{color:#e5e5e5}

/* Success state */
.success-icon{width:64px;height:64px;border-radius:50%;background:linear-gradient(135deg,#22d3ee,#a78bfa);display:flex;align-items:center;justify-content:center;margin:0 auto 24px;font-size:28px;animation:scaleIn 0.5s cubic-bezier(0.34,1.56,0.64,1)}
@keyframes scaleIn{from{transform:scale(0)}to{transform:scale(1)}}
.success-cmd{background:#141414;border:1px solid #262626;border-radius:12px;padding:16px 24px;font-family:'JetBrains Mono',monospace;font-size:0.9rem;color:#22d3ee;margin-top:24px;display:inline-block}

.stats{display:flex;gap:24px;justify-content:center;margin:32px 0;flex-wrap:wrap}
.stat{text-align:center}
.stat-value{font-family:'JetBrains Mono',monospace;font-size:1.2rem;font-weight:700;color:#e5e5e5}
.stat-label{font-size:0.7rem;color:#525252;margin-top:4px;text-transform:uppercase;letter-spacing:0.05em}

@media(max-width:480px){
  .page{padding:24px 16px}
  .device-code{font-size:1.8rem}
  .stats{gap:16px}
}
</style>
</head>
<body>
<div class="aurora">
  <div class="orb orb-1"></div>
  <div class="orb orb-2"></div>
  <div class="orb orb-3"></div>
</div>
<canvas id="constellation"></canvas>

<div class="page">
  <a href="https://savants.dev" class="brand">savants<span>.</span>cloud</a>

  ${isSuccess ? `
    <div class="success-icon">&#10003;</div>
    <div class="title">You're in</div>
    <div class="subtitle">Your CLI is now connected to savants.cloud. You can close this tab.</div>
    <div class="success-cmd">savants status</div>
    <div class="stats">
      <div class="stat"><div class="stat-value">100</div><div class="stat-label">Free credits</div></div>
      <div class="stat"><div class="stat-value">10</div><div class="stat-label">Doc sources</div></div>
      <div class="stat"><div class="stat-value">&infin;</div><div class="stat-label">Local queries</div></div>
    </div>
  ` : `
    <div class="title">${isGetStarted ? "Get started with savants" : "Connect your CLI"}</div>
    <div class="subtitle">${isGetStarted
      ? "Sign in to unlock cloud tools, team features, and documentation search."
      : "Sign in to authenticate your CLI and unlock cloud features."}</div>

    ${code ? `<div class="device-code">${code}</div>` : ""}

    <a class="oauth-btn oauth-google" href="/auth/google${code ? "?user_code=" + code : ""}">
      <svg viewBox="0 0 24 24"><path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92a5.06 5.06 0 01-2.2 3.32v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.1z" fill="#4285F4"/><path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/><path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18A10.96 10.96 0 001 12c0 1.77.42 3.45 1.18 4.93l3.66-2.84z" fill="#FBBC05"/><path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/></svg>
      Continue with Google
    </a>

    <a class="oauth-btn oauth-github" href="/auth/github${code ? "?user_code=" + code : ""}">
      <svg viewBox="0 0 24 24" fill="currentColor"><path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.531 1.032 1.531 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z"/></svg>
      Continue with GitHub
    </a>

    ${isGetStarted ? `
      <div class="divider">or install first</div>
      <div class="install-hint">
        <code>curl -fsSL savants.sh | sh</code>
        <p>5MB binary. Zero dependencies. Then run: savants connect</p>
      </div>
    ` : ""}

    <div class="footer-text">
      By signing in, you agree to the <a href="https://savants.dev/terms">Terms</a> and <a href="https://savants.dev/privacy">Privacy Policy</a>.
    </div>
  `}
</div>

<script>
// Constellation particles
(function(){
  var canvas = document.getElementById('constellation');
  if (!canvas) return;
  var ctx = canvas.getContext('2d');
  var w, h, particles = [];
  function resize() { w = canvas.width = window.innerWidth; h = canvas.height = window.innerHeight; }
  resize(); window.addEventListener('resize', resize);

  for (var i = 0; i < 40; i++) {
    particles.push({
      x: Math.random() * w, y: Math.random() * h,
      vx: (Math.random() - 0.5) * 0.3, vy: (Math.random() - 0.5) * 0.3,
      r: Math.random() * 1.5 + 0.5,
      color: Math.random() > 0.5 ? 'rgba(34,211,238,' : 'rgba(167,139,250,'
    });
  }

  function draw() {
    ctx.clearRect(0, 0, w, h);
    for (var i = 0; i < particles.length; i++) {
      var p = particles[i];
      p.x += p.vx; p.y += p.vy;
      if (p.x < 0) p.x = w; if (p.x > w) p.x = 0;
      if (p.y < 0) p.y = h; if (p.y > h) p.y = 0;

      ctx.beginPath(); ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
      ctx.fillStyle = p.color + '0.4)'; ctx.fill();

      for (var j = i + 1; j < particles.length; j++) {
        var q = particles[j];
        var dx = p.x - q.x, dy = p.y - q.y;
        var dist = Math.sqrt(dx * dx + dy * dy);
        if (dist < 120) {
          ctx.beginPath(); ctx.moveTo(p.x, p.y); ctx.lineTo(q.x, q.y);
          ctx.strokeStyle = p.color + ((1 - dist / 120) * 0.12) + ')';
          ctx.lineWidth = 0.5; ctx.stroke();
        }
      }
    }
    requestAnimationFrame(draw);
  }
  draw();

  ${token ? `
  // Store token and redirect to dashboard
  localStorage.setItem('savants_token', '${token}');
  ` : ""}
})();
</script>
</body>
</html>`);
});

// GitHub integration setup page
app.get("/integrations/github", (c) => {
  const status = new URL(c.req.url).searchParams.get("status") || undefined;
  const message = new URL(c.req.url).searchParams.get("message") || undefined;
  return c.html(githubSetupPage(status, message));
});

// Sentry integration setup page
app.get("/integrations/sentry", (c) => {
  const status = new URL(c.req.url).searchParams.get("status") || undefined;
  const message = new URL(c.req.url).searchParams.get("message") || undefined;
  return c.html(sentrySetupPage(status, message));
});

// Dashboard pages (server-rendered)
app.get("/dashboard", (c) => c.html(dashboardPage()));
app.get("/dashboard/project/:slug", (c) => {
  return c.html(projectDetailPage(c.req.param("slug")));
});
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
app.route("/api/v1/docs", docsRoutes);
app.route("/api/v1/docs", docsSearchRoutes);

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
api.route("/credits", creditsRoutes);
api.route("/projects", projectsRoutes);
api.route("/graph", graphRoutes);
api.route("/ingest", graphRoutes);   // aliases: /api/v1/ingest/* maps to same handlers
api.route("/audit", auditRoutes);
api.route("/transfers", transfersRoutes);
api.route("/docs", docsUploadRoutes);
api.route("/docs", docsIndexerRoutes);
api.route("/agents", agentsRoutes);

// Telemetry - no auth, anonymous, registered before auth middleware
app.post("/api/v1/telemetry", async (c) => {
  const body = await c.req.json<{ d: string; t: string; ms: number; os?: string; arch?: string; v?: string }>();
  if (!body.d || !body.t) return c.json({ ok: false }, 400);
  await c.env.DB.prepare(
    "INSERT INTO telemetry_events (device_id, tool, duration_ms, os, arch, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
  ).bind(body.d, body.t, body.ms || 0, body.os || null, body.arch || null, body.v || null).run();
  return c.json({ ok: true });
});

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
