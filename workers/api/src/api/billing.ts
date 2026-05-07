import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { getOrgById } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const billing = new Hono<HonoEnv>();

// Plan definitions
const PLANS = {
  free: { name: "Free", price: 0, nodes: 0, agents: 0, users: 1, clusters: 0, history_days: 0 },
  starter: { name: "Starter", price: 499, nodes: 10000, agents: 1, users: 10, clusters: 1, history_days: 7 },
  pro: { name: "Pro", price: 2999, nodes: 100000, agents: 5, users: 50, clusters: 3, history_days: 90 },
  enterprise: { name: "Enterprise", price: 9999, nodes: -1, agents: -1, users: -1, clusters: -1, history_days: 365 },
} as const;

// Stripe Price IDs are stored as env vars: STRIPE_PRICE_STARTER, STRIPE_PRICE_PRO
// Enterprise is custom (contact sales)
function getPriceId(env: Env, plan: string): string | null {
  if (plan === "starter") return (env as any).STRIPE_PRICE_STARTER || env.STRIPE_PRICE_ID;
  if (plan === "pro") return (env as any).STRIPE_PRICE_PRO || env.STRIPE_PRICE_ID;
  return null;
}

// GET /api/v1/billing - Return plan + usage + limits
billing.get("/", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);
  if (!org) return c.json({ error: "not_found" }, 404);

  const plan = (org.plan || "free") as keyof typeof PLANS;
  const limits = PLANS[plan] || PLANS.free;

  // Get current usage
  const nodeCount = await c.env.DB.prepare(
    "SELECT COUNT(*) as c FROM graph_nodes WHERE project_id IN (SELECT id FROM projects WHERE org_id = ?1)"
  ).bind(auth.orgId).first<{ c: number }>();

  const agentCount = await c.env.DB.prepare(
    "SELECT COUNT(*) as c FROM agents WHERE org_id = ?1"
  ).bind(auth.orgId).first<{ c: number }>();

  const userCount = await c.env.DB.prepare(
    "SELECT COUNT(*) as c FROM users WHERE org_id = ?1"
  ).bind(auth.orgId).first<{ c: number }>();

  let subscription = null;
  if (org.stripe_subscription_id) {
    try {
      const subRes = await fetch(
        `https://api.stripe.com/v1/subscriptions/${org.stripe_subscription_id}`,
        { headers: { Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}` } }
      );
      if (subRes.ok) {
        const sub = await subRes.json<any>();
        subscription = {
          status: sub.status,
          current_period_end: sub.current_period_end,
          cancel_at_period_end: sub.cancel_at_period_end,
        };
      }
    } catch {}
  }

  return c.json({
    plan,
    plan_name: limits.name,
    price: limits.price,
    limits: {
      graph_nodes: limits.nodes,
      agents: limits.agents,
      users: limits.users,
      clusters: limits.clusters,
      history_days: limits.history_days,
    },
    usage: {
      graph_nodes: nodeCount?.c || 0,
      agents: agentCount?.c || 0,
      users: userCount?.c || 0,
    },
    subscription,
    stripe_customer_id: org.stripe_customer_id,
  });
});

// POST /api/v1/billing/checkout - Create Stripe checkout for a specific plan
billing.post("/checkout", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);
  if (!org) return c.json({ error: "not_found" }, 404);

  const body = await c.req.json<{ plan?: string; success_url?: string; cancel_url?: string }>().catch(() => ({ plan: "starter", success_url: undefined, cancel_url: undefined }));
  const plan = body.plan || "starter";

  if (plan !== "starter" && plan !== "pro") {
    return c.json({ error: "invalid_plan", message: "Use 'starter' or 'pro'. Enterprise: contact sales@savants.dev" }, 400);
  }

  // Don't allow downgrade via checkout (use portal)
  const currentPlanOrder = { free: 0, starter: 1, pro: 2, enterprise: 3 };
  const currentOrder = currentPlanOrder[org.plan as keyof typeof currentPlanOrder] || 0;
  const newOrder = currentPlanOrder[plan as keyof typeof currentPlanOrder] || 0;
  if (newOrder <= currentOrder && currentOrder > 0) {
    return c.json({ error: "already_subscribed", message: "Already on " + org.plan + ". Use Stripe portal to manage." }, 409);
  }

  const priceId = getPriceId(c.env, plan);
  if (!priceId) return c.json({ error: "config_error", message: "Stripe price not configured for " + plan }, 500);

  const successUrl = body.success_url || "https://savants.cloud/dashboard?checkout=success";
  const cancelUrl = body.cancel_url || "https://savants.cloud/pricing";

  // Create or reuse Stripe customer
  let customerId = org.stripe_customer_id;
  if (!customerId) {
    const user = await c.env.DB.prepare("SELECT email FROM users WHERE id = ?1").bind(auth.userId).first<{ email: string }>();
    const createRes = await fetch("https://api.stripe.com/v1/customers", {
      method: "POST",
      headers: { Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`, "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ email: user?.email || "", name: org.name, "metadata[org_id]": org.id }),
    });
    if (!createRes.ok) return c.json({ error: "stripe_error", message: await createRes.text() }, 502);
    const customer = await createRes.json<{ id: string }>();
    customerId = customer.id;
    await c.env.DB.prepare("UPDATE orgs SET stripe_customer_id = ?1, updated_at = ?2 WHERE id = ?3")
      .bind(customerId, Math.floor(Date.now() / 1000), org.id).run();
  }

  // Create checkout session
  const checkoutRes = await fetch("https://api.stripe.com/v1/checkout/sessions", {
    method: "POST",
    headers: { Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`, "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      customer: customerId,
      mode: "subscription",
      "line_items[0][price]": priceId,
      "line_items[0][quantity]": "1",
      success_url: successUrl,
      cancel_url: cancelUrl,
      "metadata[org_id]": org.id,
      "metadata[plan]": plan,
      "subscription_data[metadata][org_id]": org.id,
      "subscription_data[metadata][plan]": plan,
    }),
  });

  if (!checkoutRes.ok) return c.json({ error: "stripe_error", message: await checkoutRes.text() }, 502);
  const session = await checkoutRes.json<{ id: string; url: string }>();
  return c.json({ checkout_url: session.url, url: session.url, session_id: session.id });
});

// POST /api/v1/billing/portal - Create Stripe customer portal session
billing.post("/portal", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);
  if (!org?.stripe_customer_id) {
    return c.json({ error: "no_subscription", message: "No active subscription" }, 400);
  }

  const portalRes = await fetch("https://api.stripe.com/v1/billing_portal/sessions", {
    method: "POST",
    headers: { Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`, "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      customer: org.stripe_customer_id,
      return_url: "https://savants.cloud/billing",
    }),
  });

  if (!portalRes.ok) return c.json({ error: "stripe_error", message: await portalRes.text() }, 502);
  const portal = await portalRes.json<{ url: string }>();
  return c.json({ url: portal.url });
});

// GET /api/v1/billing/invoices - List invoices from Stripe
billing.get("/invoices", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);
  if (!org?.stripe_customer_id) return c.json({ invoices: [] });

  const invRes = await fetch(
    `https://api.stripe.com/v1/invoices?customer=${org.stripe_customer_id}&limit=12`,
    { headers: { Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}` } }
  );

  if (!invRes.ok) return c.json({ invoices: [] });
  const data = await invRes.json<{ data: any[] }>();

  return c.json({
    invoices: data.data.map((inv: any) => ({
      date: inv.created * 1000,
      description: inv.lines?.data?.[0]?.description || "Savants Cloud",
      amount_cents: inv.amount_paid || inv.total,
      status: inv.status,
      pdf_url: inv.invoice_pdf,
    })),
  });
});

// GET /api/v1/billing/limits - Check if org is within plan limits (used by tools)
billing.get("/limits", async (c) => {
  const auth = c.get("auth");
  const org = await getOrgById(c.env.DB, auth.orgId);
  const plan = (org?.plan || "free") as keyof typeof PLANS;
  const limits = PLANS[plan] || PLANS.free;

  if (plan === "free") {
    return c.json({ allowed: false, reason: "Cloud tools require a paid plan. See https://savants.dev/pricing" });
  }

  // Check node limit (-1 = unlimited)
  if (limits.nodes > 0) {
    const nodeCount = await c.env.DB.prepare(
      "SELECT COUNT(*) as c FROM graph_nodes WHERE project_id IN (SELECT id FROM projects WHERE org_id = ?1)"
    ).bind(auth.orgId).first<{ c: number }>();
    if ((nodeCount?.c || 0) > limits.nodes) {
      return c.json({ allowed: false, reason: `Graph node limit exceeded (${nodeCount?.c}/${limits.nodes}). Upgrade at https://savants.dev/pricing` });
    }
  }

  return c.json({ allowed: true, plan, limits });
});

export default billing;
