/**
 * WorkOS SSO integration for enterprise customers.
 * Handles SAML/OIDC single sign-on via WorkOS.
 *
 * Flow:
 * 1. Enterprise admin configures SSO in WorkOS dashboard
 * 2. User clicks "Sign in with SSO" -> redirected to WorkOS
 * 3. WorkOS authenticates via their IdP (Okta, Azure AD, etc)
 * 4. Callback receives profile -> upsert user -> issue JWT
 *
 * Requires: WORKOS_API_KEY, WORKOS_CLIENT_ID secrets
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { upsertUser, createOrg, addMembership } from "../db/queries";
import { signJwt } from "./jwt";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const sso = new Hono<HonoEnv>();

const API_ORIGIN = "https://api.savants.cloud";
const DASHBOARD_URL = "https://savants.cloud/dashboard";

// GET /auth/sso - Start SSO login flow
// Query params: organization (WorkOS org ID) or connection (specific connection)
// or domain (auto-detect from email domain)
sso.get("/sso", async (c) => {
  const apiKey = (c.env as any).WORKOS_API_KEY;
  const clientId = (c.env as any).WORKOS_CLIENT_ID;
  if (!apiKey || !clientId) {
    return c.json({ error: "SSO not configured. Set WORKOS_API_KEY and WORKOS_CLIENT_ID." }, 501);
  }

  const redirect = c.req.query("redirect") || DASHBOARD_URL;
  const domain = c.req.query("domain") || "";
  const organization = c.req.query("organization") || "";
  const connection = c.req.query("connection") || "";

  // Build WorkOS authorization URL
  const params = new URLSearchParams({
    client_id: clientId,
    redirect_uri: `${API_ORIGIN}/auth/callback/sso`,
    response_type: "code",
    state: btoa(JSON.stringify({ redirect })),
  });

  // WorkOS supports routing by organization, connection, or domain
  if (organization) params.set("organization", organization);
  else if (connection) params.set("connection", connection);
  else if (domain) params.set("login_hint", domain);

  return c.redirect(`https://api.workos.com/sso/authorize?${params.toString()}`);
});

// GET /auth/callback/sso - WorkOS callback after authentication
sso.get("/callback/sso", async (c) => {
  const apiKey = (c.env as any).WORKOS_API_KEY;
  const clientId = (c.env as any).WORKOS_CLIENT_ID;
  const code = c.req.query("code");
  const stateRaw = c.req.query("state") || "";

  if (!code) {
    return c.redirect(`${DASHBOARD_URL}?error=sso_failed`);
  }

  let redirect = DASHBOARD_URL;
  try {
    const state = JSON.parse(atob(stateRaw));
    redirect = state.redirect || DASHBOARD_URL;
  } catch {}

  // Exchange code for profile via WorkOS API
  const tokenRes = await fetch("https://api.workos.com/sso/token", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      client_id: clientId,
      client_secret: apiKey,
      grant_type: "authorization_code",
      code,
    }),
  });

  if (!tokenRes.ok) {
    console.error("[sso] token exchange failed:", await tokenRes.text());
    return c.redirect(`${DASHBOARD_URL}?error=sso_token_failed`);
  }

  const tokenData = await tokenRes.json() as {
    profile: {
      id: string;
      email: string;
      first_name: string;
      last_name: string;
      organization_id: string;
      connection_id: string;
      connection_type: string;
      idp_id: string;
      raw_attributes: Record<string, unknown>;
    };
  };

  const profile = tokenData.profile;
  if (!profile?.email) {
    return c.redirect(`${DASHBOARD_URL}?error=sso_no_email`);
  }

  // Upsert user
  const name = `${profile.first_name || ""} ${profile.last_name || ""}`.trim() || profile.email.split("@")[0];
  const userId = crypto.randomUUID();
  const user = await upsertUser(c.env.DB, {
    id: userId,
    email: profile.email,
    name,
    avatar_url: null,
    provider: "workos",
    provider_id: profile.id,
  });

  // Find or create org based on WorkOS organization
  let orgId: string;
  const domain = profile.email.split("@")[1];

  // Check if there's an existing org linked to this WorkOS organization
  const existingOrg = await c.env.DB.prepare(
    "SELECT id FROM orgs WHERE metadata LIKE ?1 LIMIT 1"
  ).bind(`%${profile.organization_id}%`).first<{ id: string }>();

  if (existingOrg) {
    orgId = existingOrg.id;
  } else {
    // Check if org exists by email domain
    const domainOrg = await c.env.DB.prepare(
      "SELECT o.id FROM orgs o JOIN memberships m ON m.org_id = o.id JOIN users u ON u.id = m.user_id WHERE u.email LIKE ?1 LIMIT 1"
    ).bind(`%@${domain}`).first<{ id: string }>();

    if (domainOrg) {
      orgId = domainOrg.id;
      // Link WorkOS org ID to our org
      await c.env.DB.prepare(
        "UPDATE orgs SET metadata = ?1 WHERE id = ?2"
      ).bind(JSON.stringify({ workos_org_id: profile.organization_id }), orgId).run();
    } else {
      // Create new org
      const newOrgId = crypto.randomUUID();
      const org = await createOrg(c.env.DB, {
        id: newOrgId,
        name: domain.split(".")[0],
        slug: domain.split(".")[0],
      });
      orgId = org.id;
      await c.env.DB.prepare(
        "UPDATE orgs SET plan = 'enterprise', metadata = ?1 WHERE id = ?2"
      ).bind(JSON.stringify({ workos_org_id: profile.organization_id }), orgId).run();
    }
  }

  // Ensure membership
  const membershipId = crypto.randomUUID();
  await addMembership(c.env.DB, { id: membershipId, userId: user.id, orgId, role: "member" });

  // Issue JWT
  const token = await signJwt(
    { sub: user.id, org: orgId, email: profile.email },
    c.env.JWT_SECRET
  );

  // Redirect with token
  const sep = redirect.includes("?") ? "&" : "?";
  return c.redirect(`${redirect}${sep}token=${token}`);
});

// GET /auth/sso/connections - List SSO connections for an org (admin only)
sso.get("/sso/connections", async (c) => {
  const apiKey = (c.env as any).WORKOS_API_KEY;
  if (!apiKey) return c.json({ connections: [] });

  const auth = c.get("auth");

  // Get WorkOS org ID from our org
  const org = await c.env.DB.prepare(
    "SELECT metadata FROM orgs WHERE id = ?1"
  ).bind(auth.orgId).first<{ metadata: string }>();

  let workosOrgId = "";
  try {
    const meta = JSON.parse(org?.metadata || "{}");
    workosOrgId = meta.workos_org_id || "";
  } catch {}

  if (!workosOrgId) {
    return c.json({ connections: [], setup_url: "https://savants.cloud/dashboard/settings?tab=sso" });
  }

  // List connections from WorkOS
  const res = await fetch(
    `https://api.workos.com/connections?organization_id=${workosOrgId}`,
    { headers: { Authorization: `Bearer ${apiKey}` } }
  );

  if (!res.ok) return c.json({ connections: [] });
  const data = await res.json() as { data: any[] };

  return c.json({
    connections: (data.data || []).map((conn: any) => ({
      id: conn.id,
      name: conn.name,
      type: conn.connection_type,
      state: conn.state,
      domains: conn.domains?.map((d: any) => d.domain) || [],
    })),
  });
});

export default sso;
