import { describe, it, expect } from "vitest";
import { workerFetch } from "./helpers";

describe("Auth middleware - protected routes", () => {
  const protectedRoutes = [
    { method: "GET" as const, path: "/api/v1/org" },
    { method: "GET" as const, path: "/api/v1/usage" },
    { method: "GET" as const, path: "/api/v1/billing" },
    { method: "GET" as const, path: "/api/v1/graphs" },
  ];

  describe("rejects requests without Authorization header", () => {
    for (const route of protectedRoutes) {
      it(`${route.method} ${route.path} returns 401 without auth`, async () => {
        const resp = await workerFetch(route.path, { method: route.method });

        expect(resp.status).toBe(401);
        const body = await resp.json() as { error: string; message: string; status: number };
        expect(body.error).toBe("unauthorized");
        expect(body.message).toBe("Missing Authorization header");
        expect(body.status).toBe(401);
      });
    }
  });

  describe("rejects requests with invalid token", () => {
    for (const route of protectedRoutes) {
      it(`${route.method} ${route.path} returns 401 with garbage token`, async () => {
        const resp = await workerFetch(route.path, {
          method: route.method,
          headers: { Authorization: "Bearer invalid-token-garbage" },
        });

        expect(resp.status).toBe(401);
        const body = await resp.json() as { error: string };
        expect(body.error).toBe("unauthorized");
      });
    }
  });

  describe("rejects requests with empty Bearer token", () => {
    it("GET /api/v1/org returns 401 with empty Bearer", async () => {
      const resp = await workerFetch("/api/v1/org", {
        method: "GET",
        headers: { Authorization: "Bearer " },
      });

      expect(resp.status).toBe(401);
      const body = await resp.json() as { error: string; message: string };
      expect(body.error).toBe("unauthorized");
      expect(body.message).toBe("Empty token");
    });
  });

  describe("rejects requests with expired JWT", () => {
    it("GET /api/v1/org returns 401 with expired token", async () => {
      const { generateTestJwt } = await import("./helpers");
      // Create a JWT that expired 1 hour ago
      const expiredToken = await generateTestJwt({
        exp: Math.floor(Date.now() / 1000) - 3600,
      });

      const resp = await workerFetch("/api/v1/org", {
        method: "GET",
        headers: { Authorization: `Bearer ${expiredToken}` },
      });

      expect(resp.status).toBe(401);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("unauthorized");
    });
  });

  describe("rejects requests with wrong JWT secret", () => {
    it("GET /api/v1/org returns 401 with token signed by wrong secret", async () => {
      // Manually create a JWT signed with a different secret
      const ENCODER = new TextEncoder();
      const now = Math.floor(Date.now() / 1000);
      const payload = { sub: "usr_1", org: "org_1", email: "x@x.com", iat: now, exp: now + 3600 };
      const header = { alg: "HS256", typ: "JWT" };

      function b64url(data: ArrayBuffer | Uint8Array): string {
        const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
        let binary = "";
        for (let i = 0; i < bytes.length; i++) {
          binary += String.fromCharCode(bytes[i]);
        }
        return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
      }

      const headerB64 = b64url(ENCODER.encode(JSON.stringify(header)));
      const payloadB64 = b64url(ENCODER.encode(JSON.stringify(payload)));
      const signingInput = `${headerB64}.${payloadB64}`;

      const wrongKey = await crypto.subtle.importKey(
        "raw",
        ENCODER.encode("completely-wrong-secret"),
        { name: "HMAC", hash: "SHA-256" },
        false,
        ["sign"]
      );
      const signature = await crypto.subtle.sign("HMAC", wrongKey, ENCODER.encode(signingInput));
      const signatureB64 = b64url(signature);
      const badToken = `${signingInput}.${signatureB64}`;

      const resp = await workerFetch("/api/v1/org", {
        method: "GET",
        headers: { Authorization: `Bearer ${badToken}` },
      });

      expect(resp.status).toBe(401);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("unauthorized");
    });
  });
});
