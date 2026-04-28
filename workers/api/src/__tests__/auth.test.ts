import { describe, it, expect } from "vitest";
import { workerFetch } from "./helpers";

describe("Device auth flow", () => {
  describe("POST /auth/device/code", () => {
    it("returns device_code, user_code, and verification_uri", async () => {
      const resp = await workerFetch("/auth/device/code", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      expect(resp.status).toBe(200);

      const body = await resp.json() as {
        device_code: string;
        user_code: string;
        verification_uri: string;
        verification_uri_complete: string;
        expires_in: number;
        interval: number;
      };

      expect(body).toHaveProperty("device_code");
      expect(body).toHaveProperty("user_code");
      expect(body).toHaveProperty("verification_uri");
      expect(body).toHaveProperty("verification_uri_complete");
      expect(body).toHaveProperty("expires_in");
      expect(body).toHaveProperty("interval");
    });

    it("user_code is 8 characters alphanumeric (uppercase + digits, no ambiguous chars)", async () => {
      const resp = await workerFetch("/auth/device/code", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      const body = await resp.json() as { user_code: string };
      expect(body.user_code).toHaveLength(8);
      // The character set is ABCDEFGHJKLMNPQRSTUVWXYZ23456789 (no I, O, 0, 1)
      expect(body.user_code).toMatch(/^[A-HJ-NP-Z2-9]{8}$/);
    });

    it("device_code is a valid UUID", async () => {
      const resp = await workerFetch("/auth/device/code", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      const body = await resp.json() as { device_code: string };
      expect(body.device_code).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
      );
    });

    it("verification_uri points to savants.cloud/activate", async () => {
      const resp = await workerFetch("/auth/device/code", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      const body = await resp.json() as {
        verification_uri: string;
        verification_uri_complete: string;
        user_code: string;
      };

      expect(body.verification_uri).toBe("https://savants.cloud/activate");
      expect(body.verification_uri_complete).toBe(
        `https://savants.cloud/activate?code=${body.user_code}`
      );
    });

    it("expires_in is 900 seconds and interval is 5", async () => {
      const resp = await workerFetch("/auth/device/code", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      const body = await resp.json() as { expires_in: number; interval: number };
      expect(body.expires_in).toBe(900);
      expect(body.interval).toBe(5);
    });
  });

  describe("POST /auth/device/token", () => {
    it("returns 400 with missing device_code", async () => {
      const resp = await workerFetch("/auth/device/token", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("invalid_request");
    });

    it("returns 428 (authorization_pending) for a valid pending device code", async () => {
      // First, create a device code
      const codeResp = await workerFetch("/auth/device/code", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });
      const codeBody = await codeResp.json() as { device_code: string };

      // Now poll for token - should be pending
      const tokenResp = await workerFetch("/auth/device/token", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ device_code: codeBody.device_code }),
      });

      expect(tokenResp.status).toBe(428);
      const tokenBody = await tokenResp.json() as { error: string; message: string };
      expect(tokenBody.error).toBe("authorization_pending");
      expect(tokenBody.message).toBe("User has not yet authorized");
    });

    it("returns 400 for a nonexistent device code", async () => {
      const resp = await workerFetch("/auth/device/token", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ device_code: "00000000-0000-0000-0000-000000000000" }),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("expired_token");
    });
  });

  describe("POST /auth/device/activate", () => {
    it("returns 400 with missing required fields", async () => {
      const resp = await workerFetch("/auth/device/activate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ user_code: "ABCD1234" }),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("invalid_request");
    });

    it("returns 400 with expired/nonexistent user_code", async () => {
      const resp = await workerFetch("/auth/device/activate", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          user_code: "ZZZZZZZZ",
          email: "test@example.com",
          name: "Test User",
          provider: "google",
          provider_id: "google-123",
        }),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("expired_token");
    });
  });
});
