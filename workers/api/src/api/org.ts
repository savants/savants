import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import {
  getOrgById,
  getOrgMembers,
  listApiKeys,
  createApiKey,
  deleteApiKey,
  getUserByEmail,
  addMembership,
  upsertUser,
} from "../db/queries";
import { generateApiKey, hashKey, extractKeyPrefix } from "../lib/crypto";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const org = new Hono<HonoEnv>();

// GET /api/v1/org - Return org details
org.get("/", async (c) => {
  const auth = c.get("auth");
  const orgData = await getOrgById(c.env.DB, auth.orgId);

  if (!orgData) {
    return c.json({ error: "not_found", message: "Org not found", status: 404 }, 404);
  }

  return c.json({
    id: orgData.id,
    name: orgData.name,
    slug: orgData.slug,
    plan: orgData.plan,
    created_at: orgData.created_at,
  });
});

// GET /api/v1/org/members - List members
org.get("/members", async (c) => {
  const auth = c.get("auth");
  const members = await getOrgMembers(c.env.DB, auth.orgId);
  return c.json({ members });
});

// POST /api/v1/org/members/invite - Invite by email
org.post("/members/invite", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ email: string; role?: string }>();

  if (!body.email) {
    return c.json({ error: "invalid_request", message: "email is required", status: 400 }, 400);
  }

  const role = body.role ?? "member";
  if (!["member", "admin"].includes(role)) {
    return c.json({ error: "invalid_request", message: "role must be member or admin", status: 400 }, 400);
  }

  // Check if user exists
  let user = await getUserByEmail(c.env.DB, body.email);

  if (!user) {
    // Create a placeholder user for the invite
    const userId = crypto.randomUUID();
    user = await upsertUser(c.env.DB, {
      id: userId,
      email: body.email,
      name: body.email.split("@")[0],
      avatar_url: null,
      provider: "invite",
      provider_id: `invite-${userId}`,
    });
  }

  // Check for existing membership
  const existing = await c.env.DB
    .prepare("SELECT id FROM memberships WHERE user_id = ?1 AND org_id = ?2")
    .bind(user.id, auth.orgId)
    .first();

  if (existing) {
    return c.json({ error: "conflict", message: "User is already a member", status: 409 }, 409);
  }

  await addMembership(c.env.DB, {
    id: crypto.randomUUID(),
    userId: user.id,
    orgId: auth.orgId,
    role,
  });

  return c.json({ status: "invited", email: body.email, role });
});

// GET /api/v1/org/keys - List API keys (prefix only)
org.get("/keys", async (c) => {
  const auth = c.get("auth");
  const keys = await listApiKeys(c.env.DB, auth.orgId);
  return c.json({ keys });
});

// POST /api/v1/org/keys - Generate new API key
org.post("/keys", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ name: string; scopes?: string }>();

  if (!body.name) {
    return c.json({ error: "invalid_request", message: "name is required", status: 400 }, 400);
  }

  const rawKey = generateApiKey();
  const prefix = extractKeyPrefix(rawKey);
  const keyHash = await hashKey(rawKey);

  const keyId = crypto.randomUUID();
  await createApiKey(c.env.DB, {
    id: keyId,
    orgId: auth.orgId,
    name: body.name,
    prefix,
    keyHash,
    scopes: body.scopes ?? "*",
    createdBy: auth.userId,
  });

  return c.json({
    id: keyId,
    name: body.name,
    key: rawKey,
    prefix,
    message: "Store this key securely. It will not be shown again.",
  });
});

// DELETE /api/v1/org/keys/:id - Delete API key
org.delete("/keys/:id", async (c) => {
  const auth = c.get("auth");
  const keyId = c.req.param("id");

  const deleted = await deleteApiKey(c.env.DB, keyId, auth.orgId);
  if (!deleted) {
    return c.json({ error: "not_found", message: "Key not found", status: 404 }, 404);
  }

  return c.json({ status: "deleted", id: keyId });
});

export default org;
