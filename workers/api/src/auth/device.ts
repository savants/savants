import { Hono } from "hono";
import type { Env, AuthContext, DeviceAuthSession } from "../lib/types";
import { generateUserCode } from "../lib/crypto";
import { signJwt } from "./jwt";
import { upsertUser, createOrg, addMembership, getUserByEmail } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const device = new Hono<HonoEnv>();

const DEVICE_CODE_TTL = 900;
const POLL_INTERVAL = 5;
const VERIFICATION_URI = "https://savants.cloud/activate";

// POST /auth/device/code - Generate device + user codes
device.post("/code", async (c) => {
  const deviceCode = crypto.randomUUID();
  const userCode = generateUserCode();

  const session: DeviceAuthSession = {
    device_code: deviceCode,
    user_code: userCode,
    status: "pending",
    user_id: null,
    org_id: null,
    expires_at: Math.floor(Date.now() / 1000) + DEVICE_CODE_TTL,
  };

  await c.env.KV.put(`device:${deviceCode}`, JSON.stringify(session), {
    expirationTtl: DEVICE_CODE_TTL,
  });
  await c.env.KV.put(`device_user_code:${userCode}`, deviceCode, {
    expirationTtl: DEVICE_CODE_TTL,
  });

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

  // Rate limit polling
  const rateLimitKey = `device_poll:${device_code}`;
  const lastPoll = await c.env.KV.get(rateLimitKey);
  if (lastPoll) {
    const elapsed = Date.now() - parseInt(lastPoll, 10);
    if (elapsed < POLL_INTERVAL * 1000) {
      return c.json({ error: "slow_down", message: "Polling too fast" }, 428);
    }
  }
  await c.env.KV.put(rateLimitKey, Date.now().toString(), { expirationTtl: 60 });

  const raw = await c.env.KV.get(`device:${device_code}`);
  if (!raw) {
    return c.json({ error: "expired_token", message: "Device code expired or not found" }, 400);
  }

  const session: DeviceAuthSession = JSON.parse(raw);

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

  // Clean up KV
  await c.env.KV.delete(`device:${device_code}`);
  await c.env.KV.delete(`device_user_code:${session.user_code}`);
  await c.env.KV.delete(rateLimitKey);

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

  // Resolve device_code from user_code
  const deviceCode = await c.env.KV.get(`device_user_code:${user_code}`);
  if (!deviceCode) {
    return c.json({ error: "expired_token", message: "User code expired or not found" }, 400);
  }

  const raw = await c.env.KV.get(`device:${deviceCode}`);
  if (!raw) {
    return c.json({ error: "expired_token", message: "Device session expired" }, 400);
  }

  const session: DeviceAuthSession = JSON.parse(raw);
  if (session.status !== "pending") {
    return c.json({ error: "invalid_request", message: "Session already processed" }, 400);
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

  // Approve session
  session.status = "approved";
  session.user_id = user.id;
  session.org_id = orgId;

  const ttl = session.expires_at - Math.floor(Date.now() / 1000);
  if (ttl > 0) {
    await c.env.KV.put(`device:${deviceCode}`, JSON.stringify(session), { expirationTtl: ttl });
  }

  return c.json({ status: "approved", user_id: user.id, org_id: orgId });
});

export default device;
