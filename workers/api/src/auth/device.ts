import { Hono } from "hono";
import type { Env, AuthContext, DeviceAuthSession } from "../lib/types";
import { generateUserCode } from "../lib/crypto";
import { signJwt } from "./jwt";
import { upsertUser, createOrg, addMembership } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const device = new Hono<HonoEnv>();

const DEVICE_CODE_TTL = 900;
const POLL_INTERVAL = 5;
const VERIFICATION_URI = "https://savants.cloud/activate";

// POST /auth/device/code - Generate device + user codes
device.post("/code", async (c) => {
  const deviceCode = crypto.randomUUID();
  const userCode = generateUserCode();
  const expiresAt = Math.floor(Date.now() / 1000) + DEVICE_CODE_TTL;

  await c.env.DB.prepare(
    "INSERT INTO device_auth_sessions (device_code, user_code, status, expires_at) VALUES (?1, ?2, 'pending', ?3)"
  ).bind(deviceCode, userCode, expiresAt).run();

  return c.json({
    device_code: deviceCode,
    user_code: userCode,
    verification_uri: VERIFICATION_URI,
    verification_uri_complete: `${VERIFICATION_URI}?code=${userCode}`,
    expires_in: DEVICE_CODE_TTL,
    interval: POLL_INTERVAL,
  });
});

// POST /auth/device/token - Poll for token (RFC 8628)
device.post("/token", async (c) => {
  const body = await c.req.json<{ device_code: string }>();
  const { device_code } = body;

  if (!device_code) {
    return c.json({ error: "invalid_request", message: "device_code is required" }, 400);
  }

  const session = await c.env.DB.prepare(
    "SELECT * FROM device_auth_sessions WHERE device_code = ?1"
  ).bind(device_code).first<DeviceAuthSession>();

  if (!session) {
    return c.json({ error: "expired_token", message: "Device code expired or not found" }, 400);
  }

  // Check expiry
  if (session.expires_at < Math.floor(Date.now() / 1000)) {
    await c.env.DB.prepare("DELETE FROM device_auth_sessions WHERE device_code = ?1").bind(device_code).run();
    return c.json({ error: "expired_token", message: "Device code expired" }, 400);
  }

  if (session.status === "pending") {
    return c.json({ error: "authorization_pending", message: "User has not yet authorized" }, 428);
  }

  if (session.status !== "approved" || !session.user_id || !session.org_id) {
    return c.json({ error: "expired_token", message: "Session expired" }, 400);
  }

  // Approved - issue JWT
  const user = await c.env.DB.prepare("SELECT * FROM users WHERE id = ?1").bind(session.user_id).first<{ email: string }>();
  const token = await signJwt(
    { sub: session.user_id, org: session.org_id, email: user?.email ?? "" },
    c.env.JWT_SECRET
  );

  // Clean up
  await c.env.DB.prepare("DELETE FROM device_auth_sessions WHERE device_code = ?1").bind(device_code).run();

  return c.json({
    access_token: token,
    token_type: "Bearer",
    expires_in: 86400 * 30,
    user_id: session.user_id,
    org_id: session.org_id,
  });
});

// POST /auth/device/activate - Approve a device session after OAuth
device.post("/activate", async (c) => {
  const body = await c.req.json<{
    user_code: string;
    email: string;
    name: string;
    avatar_url?: string;
    provider: string;
    provider_id: string;
  }>();

  const { user_code, email, name, avatar_url, provider, provider_id } = body;

  if (!user_code || !email || !name || !provider || !provider_id) {
    return c.json({ error: "invalid_request", message: "Missing required fields" }, 400);
  }

  // Find session by user_code
  const session = await c.env.DB.prepare(
    "SELECT * FROM device_auth_sessions WHERE user_code = ?1 AND status = 'pending'"
  ).bind(user_code).first<DeviceAuthSession>();

  if (!session) {
    return c.json({ error: "expired_token", message: "User code expired or not found" }, 400);
  }

  // Upsert user
  const userId = crypto.randomUUID();
  const user = await upsertUser(c.env.DB, {
    id: userId,
    email,
    name,
    avatar_url: avatar_url ?? null,
    provider,
    provider_id,
  });

  // Check if user has an org, if not create one
  const existingMembership = await c.env.DB
    .prepare("SELECT org_id FROM memberships WHERE user_id = ?1 LIMIT 1")
    .bind(user.id)
    .first<{ org_id: string }>();

  let orgId: string;
  if (existingMembership) {
    orgId = existingMembership.org_id;
  } else {
    const slug = email.split("@")[0].toLowerCase().replace(/[^a-z0-9-]/g, "-");
    const org = await createOrg(c.env.DB, {
      id: crypto.randomUUID(),
      name: `${name}'s Org`,
      slug: `${slug}-${Date.now().toString(36)}`,
    });
    orgId = org.id;
    await addMembership(c.env.DB, {
      id: crypto.randomUUID(),
      userId: user.id,
      orgId: org.id,
      role: "owner",
    });
  }

  // Approve session in D1 (strongly consistent - immediately visible)
  await c.env.DB.prepare(
    "UPDATE device_auth_sessions SET status = 'approved', user_id = ?1, org_id = ?2 WHERE device_code = ?3"
  ).bind(user.id, orgId, session.device_code).run();

  return c.json({ status: "approved", user_id: user.id, org_id: orgId });
});

export default device;
