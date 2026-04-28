import { describe, it, expect } from "vitest";
import { SELF } from "cloudflare:test";
import { workerFetch } from "./helpers";

describe("Static routes and 404 handling", () => {
  describe("GET /activate", () => {
    it("returns 200 with HTML content", async () => {
      const resp = await workerFetch("/activate");

      expect(resp.status).toBe(200);
      const html = await resp.text();
      expect(html).toContain("<!DOCTYPE html>");
      expect(html).toContain("Activate savants");
      expect(html).toContain("Sign in with Google");
      expect(html).toContain("Sign in with GitHub");
    });

    it("shows the user code when ?code=TESTCODE is provided", async () => {
      const resp = await workerFetch("/activate?code=TESTCODE");

      expect(resp.status).toBe(200);
      const html = await resp.text();
      expect(html).toContain("TESTCODE");
      // Code should be in the code display div
      expect(html).toContain('class="code"');
      // Links should include the user_code parameter
      expect(html).toContain("?user_code=TESTCODE");
    });

    it("shows connected message when ?status=success", async () => {
      const resp = await workerFetch("/activate?status=success");

      expect(resp.status).toBe(200);
      const html = await resp.text();
      expect(html).toContain("Connected to savants.cloud");
      expect(html).toContain("Your CLI is now authenticated");
      expect(html).toContain("savants status");
    });
  });

  describe("GET /nonexistent (404 fallback)", () => {
    it("returns 404 JSON with error details", async () => {
      const resp = await workerFetch("/nonexistent");

      expect(resp.status).toBe(404);
      const body = await resp.json() as { error: string; message: string; status: number };
      expect(body.error).toBe("not_found");
      expect(body.message).toContain("/nonexistent");
      expect(body.message).toContain("GET");
      expect(body.status).toBe(404);
    });

    it("returns 404 for POST to unknown route", async () => {
      const resp = await workerFetch("/does/not/exist", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({}),
      });

      expect(resp.status).toBe(404);
      const body = await resp.json() as { error: string; message: string };
      expect(body.error).toBe("not_found");
      expect(body.message).toContain("POST");
    });
  });

  describe("Root on savants.cloud host redirects", () => {
    it("redirects savants.cloud to savants.dev", async () => {
      const resp = await SELF.fetch("https://savants.cloud/", { redirect: "manual" });

      expect(resp.status).toBe(302);
      expect(resp.headers.get("Location")).toBe("https://savants.dev");
    });

    it("redirects www.savants.cloud to savants.dev", async () => {
      const resp = await SELF.fetch("https://www.savants.cloud/", { redirect: "manual" });

      expect(resp.status).toBe(302);
      expect(resp.headers.get("Location")).toBe("https://savants.dev");
    });
  });

  describe("Dashboard and docs redirects", () => {
    it("GET /dashboard redirects to savants.dev", async () => {
      const resp = await workerFetch("/dashboard");

      // Follow-through depends on redirect mode; check for redirect
      // workerFetch uses default redirect mode which may follow
      // Let's test with manual redirect
      const manualResp = await SELF.fetch("https://api.savants.cloud/dashboard", {
        redirect: "manual",
      });
      expect(manualResp.status).toBe(302);
      expect(manualResp.headers.get("Location")).toBe("https://savants.dev");
    });

    it("GET /docs redirects to savants.dev", async () => {
      const resp = await SELF.fetch("https://api.savants.cloud/docs", {
        redirect: "manual",
      });
      expect(resp.status).toBe(302);
      expect(resp.headers.get("Location")).toBe("https://savants.dev");
    });
  });
});
