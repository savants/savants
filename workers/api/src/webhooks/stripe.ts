import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { bufToHex, hmacSign } from "../lib/crypto";
import { updateOrgPlan, logBillingEvent, getOrgByStripeSubscription } from "../db/queries";
import { addCredits, CREDIT_PACKAGES } from "../api/credits";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const stripe = new Hono<HonoEnv>();

async function verifyStripeSignature(
  payload: string,
  sigHeader: string,
  secret: string
): Promise<boolean> {
  const parts = sigHeader.split(",");
  let timestamp = "";
  const signatures: string[] = [];

  for (const part of parts) {
    const [key, value] = part.split("=");
    if (key === "t") timestamp = value;
    if (key === "v1") signatures.push(value);
  }

  if (!timestamp || signatures.length === 0) return false;

  // Check timestamp is within 5 minutes
  const ts = parseInt(timestamp, 10);
  const now = Math.floor(Date.now() / 1000);
  if (Math.abs(now - ts) > 300) return false;

  const signedPayload = `${timestamp}.${payload}`;
  const expectedSig = await hmacSign(secret, signedPayload);
  const expectedHex = bufToHex(expectedSig);

  return signatures.some((sig) => {
    if (sig.length !== expectedHex.length) return false;
    let result = 0;
    for (let i = 0; i < sig.length; i++) {
      result |= sig.charCodeAt(i) ^ expectedHex.charCodeAt(i);
    }
    return result === 0;
  });
}

// POST /webhooks/stripe
stripe.post("/", async (c) => {
  const sigHeader = c.req.header("stripe-signature");
  if (!sigHeader) {
    return c.json({ error: "missing_signature", message: "No Stripe signature header", status: 400 }, 400);
  }

  const rawBody = await c.req.text();

  const valid = await verifyStripeSignature(rawBody, sigHeader, c.env.STRIPE_WEBHOOK_SECRET);
  if (!valid) {
    return c.json({ error: "invalid_signature", message: "Stripe signature verification failed", status: 401 }, 401);
  }

  const event = JSON.parse(rawBody) as {
    id: string;
    type: string;
    data: {
      object: Record<string, unknown>;
    };
  };

  switch (event.type) {
    case "checkout.session.completed": {
      const session = event.data.object as {
        customer: string;
        subscription?: string;
        mode: string;
        metadata: { org_id: string; package?: string; credits?: string };
      };

      const orgId = session.metadata?.org_id;
      if (!orgId) break;

      // Credit package purchase (one-time payment)
      if (session.mode === "payment" && session.metadata?.package) {
        const pkgName = session.metadata.package as keyof typeof CREDIT_PACKAGES;
        const pkg = CREDIT_PACKAGES[pkgName];
        const totalCredits = parseInt(session.metadata.credits || "0") || (pkg ? pkg.credits + pkg.bonus : 0);

        if (totalCredits > 0) {
          await addCredits(
            c.env.DB,
            orgId,
            totalCredits,
            `Purchased ${pkg?.name || pkgName} package (${totalCredits} credits)`,
            event.id
          );
        }
      }

      // Subscription (monthly plan)
      if (session.subscription) {
        await updateOrgPlan(c.env.DB, orgId, "cloud", session.customer, session.subscription);
      }

      await logBillingEvent(c.env.DB, {
        id: crypto.randomUUID(),
        orgId,
        eventType: "checkout.session.completed",
        stripeEventId: event.id,
        amountCents: null,
        currency: "usd",
        metadata: JSON.stringify({
          customer: session.customer,
          subscription: session.subscription,
          package: session.metadata?.package,
          credits: session.metadata?.credits,
        }),
      });

      break;
    }

    case "customer.subscription.deleted": {
      const subscription = event.data.object as {
        id: string;
        customer: string;
        metadata: { org_id?: string };
      };

      let orgId = subscription.metadata?.org_id;

      if (!orgId) {
        const org = await getOrgByStripeSubscription(c.env.DB, subscription.id);
        orgId = org?.id;
      }

      if (orgId) {
        await updateOrgPlan(c.env.DB, orgId, "free", null, null);

        await logBillingEvent(c.env.DB, {
          id: crypto.randomUUID(),
          orgId,
          eventType: "customer.subscription.deleted",
          stripeEventId: event.id,
          amountCents: null,
          currency: "usd",
          metadata: JSON.stringify({ subscription: subscription.id }),
        });
      }

      break;
    }

    case "invoice.payment_succeeded": {
      const invoice = event.data.object as {
        subscription: string;
        amount_paid: number;
        currency: string;
        metadata?: { org_id?: string };
      };

      let orgId = invoice.metadata?.org_id;
      if (!orgId) {
        const org = await getOrgByStripeSubscription(c.env.DB, invoice.subscription);
        orgId = org?.id;
      }

      if (orgId) {
        await logBillingEvent(c.env.DB, {
          id: crypto.randomUUID(),
          orgId,
          eventType: "invoice.payment_succeeded",
          stripeEventId: event.id,
          amountCents: invoice.amount_paid,
          currency: invoice.currency,
          metadata: null,
        });
      }

      break;
    }

    case "invoice.payment_failed": {
      const invoice = event.data.object as {
        subscription: string;
        amount_due: number;
        currency: string;
        metadata?: { org_id?: string };
      };

      let orgId = invoice.metadata?.org_id;
      if (!orgId) {
        const org = await getOrgByStripeSubscription(c.env.DB, invoice.subscription);
        orgId = org?.id;
      }

      if (orgId) {
        await logBillingEvent(c.env.DB, {
          id: crypto.randomUUID(),
          orgId,
          eventType: "invoice.payment_failed",
          stripeEventId: event.id,
          amountCents: invoice.amount_due,
          currency: invoice.currency,
          metadata: null,
        });
      }

      break;
    }
  }

  return c.json({ received: true });
});

export default stripe;
