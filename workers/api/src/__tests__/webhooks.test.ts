import { describe, it, expect } from "vitest";
import { workerFetch } from "./helpers";

describe("Webhook endpoints", () => {
  describe("POST /webhooks/stripe", () => {
    it("returns 400 with missing stripe-signature header", async () => {
      const resp = await workerFetch("/webhooks/stripe", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "checkout.session.completed" }),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string; message: string };
      expect(body.error).toBe("missing_signature");
      expect(body.message).toContain("No Stripe signature");
    });

    it("returns 401 with invalid stripe-signature", async () => {
      const resp = await workerFetch("/webhooks/stripe", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "stripe-signature": "t=1234567890,v1=invalid_signature_value",
        },
        body: JSON.stringify({ type: "checkout.session.completed" }),
      });

      expect(resp.status).toBe(401);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("invalid_signature");
    });
  });

  describe("POST /webhooks/github", () => {
    it("returns 200 with basic event payload (signature verified against GITHUB_APP_TOKEN)", async () => {
      // When GITHUB_APP_TOKEN is set (which it is in test config), signature is verified.
      // We need to provide a valid signature. Let's compute one.
      const payload = JSON.stringify({
        action: "ping",
        repository: { full_name: "test/repo" },
      });

      // Compute HMAC-SHA256 signature using the test GITHUB_APP_TOKEN
      const ENCODER = new TextEncoder();
      const key = await crypto.subtle.importKey(
        "raw",
        ENCODER.encode("ghp_test_fake"),
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["sign"]
      );
      const sig = await crypto.subtle.sign("HMAC", key, ENCODER.encode(payload));
      const hexSig = Array.from(new Uint8Array(sig))
        .map((b) => b.toString(16).padStart(2, "0"))
        .join("");

      const resp = await workerFetch("/webhooks/github", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "x-hub-signature-256": `sha256=${hexSig}`,
          "x-github-event": "ping",
        },
        body: payload,
      });

      expect(resp.status).toBe(200);
      const body = await resp.json() as { received: boolean; event: string };
      expect(body.received).toBe(true);
      expect(body.event).toBe("ping");
    });

    it("returns 401 with invalid github signature", async () => {
      const resp = await workerFetch("/webhooks/github", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "x-hub-signature-256": "sha256=0000000000000000000000000000000000000000000000000000000000000000",
          "x-github-event": "push",
        },
        body: JSON.stringify({ action: "push" }),
      });

      expect(resp.status).toBe(401);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("invalid_signature");
    });
  });

  describe("POST /webhooks/slack", () => {
    it("returns challenge for url_verification event", async () => {
      const challengeValue = "test_challenge_string_abc123";

      const resp = await workerFetch("/webhooks/slack", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          type: "url_verification",
          challenge: challengeValue,
          token: "fake_token",
        }),
      });

      expect(resp.status).toBe(200);
      const body = await resp.json() as { challenge: string };
      expect(body.challenge).toBe(challengeValue);
    });

    it("returns ok for unrecognized event types", async () => {
      const resp = await workerFetch("/webhooks/slack", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          type: "unknown_event",
          event: {},
        }),
      });

      expect(resp.status).toBe(200);
      const body = await resp.json() as { ok: boolean };
      expect(body.ok).toBe(true);
    });

    it("handles form-urlencoded payload (Slack interactive components)", async () => {
      const innerPayload = JSON.stringify({
        type: "url_verification",
        challenge: "form_challenge_456",
      });

      const resp = await workerFetch("/webhooks/slack", {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        body: `payload=${encodeURIComponent(innerPayload)}`,
      });

      expect(resp.status).toBe(200);
      const body = await resp.json() as { challenge: string };
      expect(body.challenge).toBe("form_challenge_456");
    });
  });
});
