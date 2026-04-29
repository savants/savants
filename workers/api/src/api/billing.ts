import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { getOrgById } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const billing = new Hono<HonoEnv>();

// GET /api/v1/billing - Return plan + stripe info
billing.get("/", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);

  if (!org) {
    return c.json({ error: "not_found", message: "Org not found", status: 404 }, 404);
  }

  let subscription = null;
  if (org.stripe_subscription_id && org.stripe_customer_id) {
    try {
      const subRes = await fetch(
        `https://api.stripe.com/v1/subscriptions/${org.stripe_subscription_id}`,
        {
          headers: { Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}` },
        }
      );
      if (subRes.ok) {
        const sub = await subRes.json<{
          status: string;
          current_period_start: number;
          current_period_end: number;
          cancel_at_period_end: boolean;
        }>();
        subscription = {
          status: sub.status,
          current_period_start: sub.current_period_start,
          current_period_end: sub.current_period_end,
          cancel_at_period_end: sub.cancel_at_period_end,
        };
      }
    } catch {
      // If Stripe is unreachable, still return what we know
    }
  }

  return c.json({
    org_id: org.id,
    plan: org.plan,
    stripe_customer_id: org.stripe_customer_id,
    subscription,
    pricing: {
      local: { price: "Free forever", description: "Unlimited local queries - semantic search, file skeleton, callers, where_used" },
      cloud: { price: "Pay per call", description: "10 free/month, then $0.10-$5.00 per call. No minimums, no commitments." },
      enterprise: { price: "Volume discounts", description: "SSO, audit logs, SLA, dedicated support. Contact sales." },
    },
  });
});

// POST /api/v1/billing/checkout - Create Stripe checkout session
billing.post("/checkout", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);

  if (!org) {
    return c.json({ error: "not_found", message: "Org not found", status: 404 }, 404);
  }

  if (org.plan === "cloud" || org.plan === "enterprise") {
    return c.json({ error: "already_subscribed", message: "Org already has a paid plan", status: 409 }, 409);
  }

  const body = await c.req.json<{ success_url?: string; cancel_url?: string }>().catch(() => ({ success_url: undefined, cancel_url: undefined }));
  const successUrl = body.success_url ?? "https://savants.cloud/dashboard?checkout=success";
  const cancelUrl = body.cancel_url ?? "https://savants.cloud/dashboard?checkout=cancelled";

  // Create or reuse Stripe customer
  let customerId = org.stripe_customer_id;
  if (!customerId) {
    const user = await c.env.DB.prepare("SELECT email FROM users WHERE id = ?1").bind(auth.userId).first<{ email: string }>();

    const createRes = await fetch("https://api.stripe.com/v1/customers", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({
        email: user?.email ?? "",
        name: org.name,
        "metadata[org_id]": org.id,
      }),
    });

    if (!createRes.ok) {
      const errText = await createRes.text();
      return c.json({ error: "stripe_error", message: errText, status: 502 }, 502);
    }

    const customer = await createRes.json<{ id: string }>();
    customerId = customer.id;

    // Save customer ID
    await c.env.DB
      .prepare("UPDATE orgs SET stripe_customer_id = ?1, updated_at = ?2 WHERE id = ?3")
      .bind(customerId, Math.floor(Date.now() / 1000), org.id)
      .run();
  }

  // Create checkout session
  const checkoutRes = await fetch("https://api.stripe.com/v1/checkout/sessions", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`,
      "Content-Type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({
      customer: customerId,
      mode: "subscription",
      "line_items[0][price]": c.env.STRIPE_PRICE_ID,
      "line_items[0][quantity]": "1",
      success_url: successUrl,
      cancel_url: cancelUrl,
      "metadata[org_id]": org.id,
      "subscription_data[metadata][org_id]": org.id,
    }),
  });

  if (!checkoutRes.ok) {
    const errText = await checkoutRes.text();
    return c.json({ error: "stripe_error", message: errText, status: 502 }, 502);
  }

  const session = await checkoutRes.json<{ id: string; url: string }>();

  return c.json({ checkout_url: session.url, session_id: session.id });
});

export default billing;
