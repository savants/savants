import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const credits = new Hono<HonoEnv>();

// Credit costs per tool
export const TOOL_CREDITS: Record<string, number> = {
  // Diagnosis (most expensive - cross-layer analysis)
  diagnose_error: 50,
  diagnose: 25,
  unanswered_questions: 50,
  pr_risk: 20,
  radar: 10,
  // Graph tools (D1 queries - cheap)
  graph_stats: 1,
  function_xray: 5,
  blast_radius: 10,
  impact_analysis: 10,
  dead_code: 5,
  import_tree: 5,
  module_exports: 3,
  search_code: 1,
  find_references: 3,
  dependency_chain: 5,
  risk_score: 3,
  community_summary: 3,
  decorated_with: 1,
  pre_change_warning: 5,
  coupling_check: 3,
  co_change_partners: 3,
  resolves_to: 1,
  diff_impact: 10,
  // K8s / infra
  cluster_state: 3,
  list_pods: 1,
  pod_story: 3,
  host_state: 3,
  host_story: 3,
  deployment_info: 3,
  pod_dependencies: 1,
  namespace_summary: 3,
  // Indexing
  reindex_diff: 3,
  reindex_full: 20,
  // Docs
  doc_upload: 1,
  doc_crawl: 2,
  // Local tools are free
  semantic_search: 0,
  file_skeleton: 0,
  where_used: 0,
  callers: 0,
  reindex: 0,
};

// Credit packages
export const CREDIT_PACKAGES = {
  starter: { name: "Starter", price_cents: 1000, credits: 100, bonus: 0 },
  builder: { name: "Builder", price_cents: 5000, credits: 600, bonus: 100 },
  scale: { name: "Scale", price_cents: 20000, credits: 3000, bonus: 1000 },
};

// Monthly plans
export const MONTHLY_PLANS = {
  free: { name: "Free", price_cents: 0, included_credits: 100, overage_rate: 0, discount: "" },
  pro: { name: "Pro", price_cents: 4900, included_credits: 600, overage_rate: 8, discount: "17% off" },
  team: { name: "Team", price_cents: 14900, included_credits: 2000, overage_rate: 7, discount: "30% off" },
};

// GET /api/v1/credits - Get current balance + transaction history
credits.get("/", async (c) => {
  const auth = c.get("auth");

  const balance = await c.env.DB
    .prepare("SELECT balance, auto_topup_enabled, auto_topup_package FROM credit_balances WHERE org_id = ?1")
    .bind(auth.orgId)
    .first<{ balance: number; auto_topup_enabled: number; auto_topup_package: string }>();

  const transactions = await c.env.DB
    .prepare("SELECT id, type, amount, balance_after, description, tool_name, created_at FROM credit_transactions WHERE org_id = ?1 ORDER BY created_at DESC LIMIT 50")
    .bind(auth.orgId)
    .all();

  return c.json({
    balance: balance?.balance ?? 0,
    auto_topup: {
      enabled: balance?.auto_topup_enabled === 1,
      package: balance?.auto_topup_package ?? "starter",
    },
    transactions: (transactions.results as unknown as any[]).map((t) => ({
      id: t.id,
      type: t.type,
      amount: t.amount,
      balance_after: t.balance_after,
      description: t.description,
      tool_name: t.tool_name,
      created_at: t.created_at,
    })),
    packages: CREDIT_PACKAGES,
    plans: MONTHLY_PLANS,
    tool_costs: TOOL_CREDITS,
  });
});

// POST /api/v1/credits/purchase - Buy a credit package
credits.post("/purchase", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ package: string }>().catch(() => ({ package: "" }));
  const pkg = CREDIT_PACKAGES[body.package as keyof typeof CREDIT_PACKAGES];

  if (!pkg) {
    return c.json({ error: "invalid_package", message: "Valid packages: starter, builder, scale", status: 400 }, 400);
  }

  // Create Stripe checkout for the package
  const org = await c.env.DB.prepare("SELECT * FROM orgs WHERE id = ?1").bind(auth.orgId).first<any>();
  const user = await c.env.DB.prepare("SELECT email FROM users WHERE id = ?1").bind(auth.userId).first<{ email: string }>();

  // Get or create Stripe customer
  let customerId = org?.stripe_customer_id;
  if (!customerId) {
    const createRes = await fetch("https://api.stripe.com/v1/customers", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({
        email: user?.email ?? "",
        name: org?.name ?? "",
        "metadata[org_id]": auth.orgId,
      }),
    });
    if (createRes.ok) {
      const customer = await createRes.json<{ id: string }>();
      customerId = customer.id;
      await c.env.DB
        .prepare("UPDATE orgs SET stripe_customer_id = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(customerId, Math.floor(Date.now() / 1000), auth.orgId)
        .run();
    }
  }

  // Create a one-time payment checkout
  const checkoutRes = await fetch("https://api.stripe.com/v1/checkout/sessions", {
    method: "POST",
    headers: {
      Authorization: `Bearer ${c.env.STRIPE_SECRET_KEY}`,
      "Content-Type": "application/x-www-form-urlencoded",
    },
    body: new URLSearchParams({
      customer: customerId || "",
      mode: "payment",
      "line_items[0][price_data][currency]": "usd",
      "line_items[0][price_data][product_data][name]": `Savants Credits - ${pkg.name}`,
      "line_items[0][price_data][product_data][description]": `${pkg.credits} credits${pkg.bonus ? ` (+${pkg.bonus} bonus)` : ""}`,
      "line_items[0][price_data][unit_amount]": pkg.price_cents.toString(),
      "line_items[0][quantity]": "1",
      success_url: `https://savants.cloud/dashboard/billing?purchase=success&package=${body.package}`,
      cancel_url: "https://savants.cloud/dashboard/billing?purchase=cancelled",
      "metadata[org_id]": auth.orgId,
      "metadata[package]": body.package,
      "metadata[credits]": (pkg.credits + pkg.bonus).toString(),
    }),
  });

  if (!checkoutRes.ok) {
    const errText = await checkoutRes.text();
    return c.json({ error: "stripe_error", message: errText, status: 502 }, 502);
  }

  const session = await checkoutRes.json<{ url: string; id: string }>();
  return c.json({ checkout_url: session.url, session_id: session.id });
});

// POST /api/v1/credits/topup-settings - Update auto top-up settings
credits.post("/topup-settings", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ enabled: boolean; package?: string }>().catch(() => ({ enabled: false, package: "starter" }));

  const pkg = body.package ?? "starter";
  if (!(pkg in CREDIT_PACKAGES)) {
    return c.json({ error: "invalid_package", message: "Invalid package", status: 400 }, 400);
  }

  await c.env.DB
    .prepare(
      "INSERT INTO credit_balances (org_id, balance, auto_topup_enabled, auto_topup_package, updated_at) VALUES (?1, 0, ?2, ?3, ?4) ON CONFLICT(org_id) DO UPDATE SET auto_topup_enabled = ?2, auto_topup_package = ?3, updated_at = ?4"
    )
    .bind(auth.orgId, body.enabled ? 1 : 0, pkg, Math.floor(Date.now() / 1000))
    .run();

  return c.json({ ok: true, auto_topup: { enabled: body.enabled, package: pkg } });
});

// ─── Helper: deduct credits for a tool call ──────────────────────────────────

export async function deductCredits(
  db: D1Database,
  orgId: string,
  toolName: string
): Promise<{ ok: boolean; balance: number; cost: number; message?: string }> {
  const cost = TOOL_CREDITS[toolName] ?? 0;

  // Free tools don't cost credits
  if (cost === 0) {
    return { ok: true, balance: -1, cost: 0 };
  }

  // Get current balance and org plan
  const row = await db
    .prepare("SELECT cb.balance, cb.auto_topup_enabled, cb.auto_topup_package, o.plan FROM credit_balances cb JOIN orgs o ON o.id = cb.org_id WHERE cb.org_id = ?1")
    .bind(orgId)
    .first<{ balance: number; auto_topup_enabled: number; auto_topup_package: string; plan: string }>();

  const currentBalance = row?.balance ?? 0;
  const isEnterprise = row?.plan === "enterprise";

  // Enterprise plans never get blocked - balance can go negative, billed monthly
  if (!isEnterprise && currentBalance < cost) {
    return {
      ok: false,
      balance: currentBalance,
      cost,
      message: `Insufficient credits. Need ${cost}, have ${currentBalance}. Buy credits at /api/v1/credits/purchase`,
    };
  }

  const newBalance = currentBalance - cost;

  // Deduct
  await db
    .prepare("UPDATE credit_balances SET balance = ?1, updated_at = ?2 WHERE org_id = ?3")
    .bind(newBalance, Math.floor(Date.now() / 1000), orgId)
    .run();

  // Log transaction
  await db
    .prepare(
      "INSERT INTO credit_transactions (id, org_id, type, amount, balance_after, description, tool_name, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
    )
    .bind(
      crypto.randomUUID(),
      orgId,
      "usage",
      -cost,
      newBalance,
      `${toolName} call`,
      toolName,
      Math.floor(Date.now() / 1000)
    )
    .run();

  return { ok: true, balance: newBalance, cost };
}

// ─── Helper: add credits (after purchase or grant) ───────────────────────────

export async function addCredits(
  db: D1Database,
  orgId: string,
  amount: number,
  description: string,
  stripePaymentId?: string
): Promise<number> {
  // Upsert balance
  await db
    .prepare(
      "INSERT INTO credit_balances (org_id, balance, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(org_id) DO UPDATE SET balance = balance + ?2, updated_at = ?3"
    )
    .bind(orgId, amount, Math.floor(Date.now() / 1000))
    .run();

  // Get new balance
  const row = await db
    .prepare("SELECT balance FROM credit_balances WHERE org_id = ?1")
    .bind(orgId)
    .first<{ balance: number }>();

  const newBalance = row?.balance ?? amount;

  // Log transaction
  await db
    .prepare(
      "INSERT INTO credit_transactions (id, org_id, type, amount, balance_after, description, stripe_payment_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
    )
    .bind(
      crypto.randomUUID(),
      orgId,
      stripePaymentId ? "purchase" : "grant",
      amount,
      newBalance,
      description,
      stripePaymentId ?? null,
      Math.floor(Date.now() / 1000)
    )
    .run();

  return newBalance;
}

export default credits;
