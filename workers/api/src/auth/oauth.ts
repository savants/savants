import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { upsertUser, createOrg, addMembership } from "../db/queries";
import { signJwt } from "./jwt";
import { audit, requestMeta } from "../lib/audit";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const oauth = new Hono<HonoEnv>();

const DASHBOARD_URL = "https://savants.cloud/dashboard";
const API_ORIGIN = "https://api.savants.cloud";

// GET /auth/google - Redirect to Google OAuth
oauth.get("/google", async (c) => {
  const userCode = c.req.query("user_code") ?? "";
  const redirect = c.req.query("redirect") ?? DASHBOARD_URL;

  const state = btoa(JSON.stringify({ user_code: userCode, redirect }));

  const params = new URLSearchParams({
    client_id: c.env.GOOGLE_CLIENT_ID,
    redirect_uri: `${API_ORIGIN}/auth/callback/google`,
    response_type: "code",
    scope: "openid email profile",
    state,
    access_type: "offline",
    prompt: "consent",
  });

  return c.redirect(`https://accounts.google.com/o/oauth2/v2/auth?${params.toString()}`);
});

// GET /auth/github - Redirect to GitHub OAuth
oauth.get("/github", async (c) => {
  const userCode = c.req.query("user_code") ?? "";
  const redirect = c.req.query("redirect") ?? DASHBOARD_URL;

  const state = btoa(JSON.stringify({ user_code: userCode, redirect }));

  const params = new URLSearchParams({
    client_id: c.env.GITHUB_CLIENT_ID,
    redirect_uri: `${API_ORIGIN}/auth/callback/github`,
    scope: "user:email read:user",
    state,
  });

  return c.redirect(`https://github.com/login/oauth/authorize?${params.toString()}`);
});

// GET /auth/callback/google - Google OAuth callback
oauth.get("/callback/google", async (c) => {
  const code = c.req.query("code");
  const stateRaw = c.req.query("state");
  const error = c.req.query("error");

  if (error || !code) {
    return c.json({ error: "oauth_error", message: error ?? "Missing authorization code" }, 400);
  }

  let userCode = "";
  let redirectUrl = DASHBOARD_URL;
  if (stateRaw) {
    try {
      const parsed = JSON.parse(atob(stateRaw));
      userCode = parsed.user_code ?? "";
      redirectUrl = parsed.redirect ?? DASHBOARD_URL;
    } catch {
      // ignore malformed state
    }
  }

  // Exchange code for tokens
  const tokenRes = await fetch("https://oauth2.googleapis.com/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      code,
      client_id: c.env.GOOGLE_CLIENT_ID,
      client_secret: c.env.GOOGLE_CLIENT_SECRET,
      redirect_uri: `${API_ORIGIN}/auth/callback/google`,
      grant_type: "authorization_code",
    }),
  });

  if (!tokenRes.ok) {
    const detail = await tokenRes.text();
    return c.json({ error: "token_exchange_failed", message: detail }, 502);
  }

  const tokens = await tokenRes.json<{ access_token: string }>();

  // Fetch user info
  const userInfoRes = await fetch("https://www.googleapis.com/oauth2/v2/userinfo", {
    headers: { Authorization: `Bearer ${tokens.access_token}` },
  });

  if (!userInfoRes.ok) {
    return c.json({ error: "userinfo_failed", message: "Failed to fetch user info" }, 502);
  }

  const userInfo = await userInfoRes.json<{ id: string; email: string; name: string; picture: string }>();

  const { user, orgId } = await upsertUserAndOrg(c.env.DB, {
    email: userInfo.email,
    name: userInfo.name,
    avatar_url: userInfo.picture,
    provider: "google",
    provider_id: userInfo.id,
  });

  // If there's a device flow user_code, approve it
  if (userCode) {
    await approveDeviceSession(c.env.KV, userCode, user.id, orgId);
    const jwt = await signJwt({ sub: user.id, org: orgId, email: user.email }, c.env.JWT_SECRET);
    return c.redirect(`https://savants.cloud/activate?status=success&token=${jwt}`);
  }

  // Issue JWT and redirect
  const jwt = await signJwt({ sub: user.id, org: orgId, email: user.email }, c.env.JWT_SECRET);
  const sep = redirectUrl.includes("?") ? "&" : "?";
  return c.redirect(`${redirectUrl}${sep}token=${jwt}`);
});

// GET /auth/callback/github - GitHub OAuth callback
oauth.get("/callback/github", async (c) => {
  const code = c.req.query("code");
  const stateRaw = c.req.query("state");
  const error = c.req.query("error");

  if (error || !code) {
    return c.json({ error: "oauth_error", message: error ?? "Missing authorization code" }, 400);
  }

  let userCode = "";
  let redirectUrl = DASHBOARD_URL;
  console.log("GitHub callback state:", stateRaw);
  if (stateRaw) {
    try {
      const decoded = atob(stateRaw);
      console.log("Decoded state:", decoded);
      const parsed = JSON.parse(decoded);
      userCode = parsed.user_code ?? "";
      redirectUrl = parsed.redirect ?? DASHBOARD_URL;
      console.log("User code from state:", userCode);
    } catch (e) {
      console.error("State parse error:", e);
    }
  }

  // Exchange code for access token
  const tokenRes = await fetch("https://github.com/login/oauth/access_token", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "application/json",
    },
    body: JSON.stringify({
      client_id: c.env.GITHUB_CLIENT_ID,
      client_secret: c.env.GITHUB_CLIENT_SECRET,
      code,
      redirect_uri: `${API_ORIGIN}/auth/callback/github`,
    }),
  });

  if (!tokenRes.ok) {
    const detail = await tokenRes.text();
    return c.json({ error: "token_exchange_failed", message: detail }, 502);
  }

  const tokens = await tokenRes.json<{ access_token: string; error?: string }>();
  if (tokens.error) {
    return c.json({ error: "token_exchange_failed", message: tokens.error }, 502);
  }

  // Fetch user info
  const userRes = await fetch("https://api.github.com/user", {
    headers: {
      Authorization: `Bearer ${tokens.access_token}`,
      "User-Agent": "Savants-Cloud-API",
    },
  });

  if (!userRes.ok) {
    return c.json({ error: "userinfo_failed", message: "Failed to fetch GitHub user" }, 502);
  }

  const ghUser = await userRes.json<{ id: number; login: string; name: string | null; avatar_url: string }>();

  // Fetch primary email
  const emailRes = await fetch("https://api.github.com/user/emails", {
    headers: {
      Authorization: `Bearer ${tokens.access_token}`,
      "User-Agent": "Savants-Cloud-API",
    },
  });

  let email = `${ghUser.login}@github.noreply.com`;
  if (emailRes.ok) {
    const emails = await emailRes.json<Array<{ email: string; primary: boolean; verified: boolean }>>();
    const primary = emails.find((e) => e.primary && e.verified);
    if (primary) email = primary.email;
  }

  const displayName = ghUser.name ?? ghUser.login;

  const { user, orgId } = await upsertUserAndOrg(c.env.DB, {
    email,
    name: displayName,
    avatar_url: ghUser.avatar_url,
    provider: "github",
    provider_id: ghUser.id.toString(),
  });

  // Audit: login event
  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId, actorId: user.id, actorEmail: email,
    action: "auth.login", resourceType: "user", resourceId: user.id,
    metadata: { provider: "github", login: ghUser.login },
    ...meta,
  });

  // Store GitHub token as an integration (for repo access later)
  await c.env.DB
    .prepare(
      `INSERT INTO integrations (id, org_id, type, config, credentials, enabled)
       VALUES (?1, ?2, 'github', ?3, ?4, 1)
       ON CONFLICT(org_id, type) DO UPDATE SET credentials = ?4, updated_at = unixepoch()`
    )
    .bind(
      crypto.randomUUID(),
      orgId,
      JSON.stringify({ login: ghUser.login, avatar_url: ghUser.avatar_url }),
      JSON.stringify({ access_token: tokens.access_token })
    )
    .run();

  if (userCode) {
    await approveDeviceSession(c.env.KV, userCode, user.id, orgId);
    // Redirect to activate success page so user sees confirmation
    const jwt = await signJwt({ sub: user.id, org: orgId, email: user.email }, c.env.JWT_SECRET);
    return c.redirect(`https://savants.cloud/activate?status=success&token=${jwt}`);
  }

  const jwt = await signJwt({ sub: user.id, org: orgId, email: user.email }, c.env.JWT_SECRET);
  const sep = redirectUrl.includes("?") ? "&" : "?";
  return c.redirect(`${redirectUrl}${sep}token=${jwt}`);
});

async function upsertUserAndOrg(
  db: D1Database,
  info: { email: string; name: string; avatar_url: string | null; provider: string; provider_id: string }
): Promise<{ user: { id: string; email: string }; orgId: string }> {
  const userId = crypto.randomUUID();
  const user = await upsertUser(db, {
    id: userId,
    email: info.email,
    name: info.name,
    avatar_url: info.avatar_url,
    provider: info.provider,
    provider_id: info.provider_id,
  });

  const existingMembership = await db
    .prepare("SELECT org_id FROM memberships WHERE user_id = ?1 LIMIT 1")
    .bind(user.id)
    .first<{ org_id: string }>();

  let orgId: string;
  if (existingMembership) {
    orgId = existingMembership.org_id;
  } else {
    const slug = info.email.split("@")[0].toLowerCase().replace(/[^a-z0-9-]/g, "-");
    const org = await createOrg(db, {
      id: crypto.randomUUID(),
      name: `${info.name}'s Org`,
      slug: `${slug}-${Date.now().toString(36)}`,
    });
    orgId = org.id;
    await addMembership(db, {
      id: crypto.randomUUID(),
      userId: user.id,
      orgId: org.id,
      role: "owner",
    });
  }

  return { user: { id: user.id, email: user.email }, orgId };
}

async function approveDeviceSession(
  kv: KVNamespace,
  userCode: string,
  userId: string,
  orgId: string
): Promise<void> {
  const deviceCode = await kv.get(`device_user_code:${userCode}`);
  if (!deviceCode) return;

  const raw = await kv.get(`device:${deviceCode}`);
  if (!raw) return;

  const session = JSON.parse(raw);
  if (session.status !== "pending") return;

  session.status = "approved";
  session.user_id = userId;
  session.org_id = orgId;

  const ttl = session.expires_at - Math.floor(Date.now() / 1000);
  if (ttl > 0) {
    await kv.put(`device:${deviceCode}`, JSON.stringify(session), { expirationTtl: ttl });
  }
}

export default oauth;
