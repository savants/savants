import { describe, it, expect } from "vitest";
import { workerFetch } from "./helpers";

describe("Health endpoints", () => {
  describe("GET /", () => {
    it("returns JSON with name, version, and status for non-savants.cloud host", async () => {
      // Using a non-savants.cloud host so it returns the JSON info response
      const res = await fetch("http://localhost/", {
        method: "GET",
        headers: {},
      }).catch(() => null);

      // Use workerFetch with a different hostname to avoid redirect
      const resp = await workerFetch("/");
      // api.savants.cloud is not savants.cloud exactly, so it should return JSON
      const body = await resp.json() as Record<string, unknown>;

      expect(resp.status).toBe(200);
      expect(body).toHaveProperty("name", "savants-cloud-api");
      expect(body).toHaveProperty("version", "1.0.0");
      expect(body).toHaveProperty("status", "ok");
    });
  });

  describe("GET /health", () => {
    it("returns ok status with a timestamp", async () => {
      const before = Math.floor(Date.now() / 1000);
      const resp = await workerFetch("/health");
      const after = Math.floor(Date.now() / 1000);

      expect(resp.status).toBe(200);

      const body = await resp.json() as { status: string; timestamp: number };
      expect(body.status).toBe("ok");
      expect(typeof body.timestamp).toBe("number");
      expect(body.timestamp).toBeGreaterThanOrEqual(before);
      expect(body.timestamp).toBeLessThanOrEqual(after);
    });

    it("includes X-Request-Id header", async () => {
      const resp = await workerFetch("/health");
      const requestId = resp.headers.get("X-Request-Id");
      expect(requestId).toBeTruthy();
      // UUID v4 format check
      expect(requestId).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
      );
    });
  });
});
