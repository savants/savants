import { describe, it, expect } from "vitest";
import { workerFetch } from "./helpers";

describe("Tools API", () => {
  describe("GET /api/v1/tools", () => {
    it("returns tool list without auth (public endpoint)", async () => {
      const resp = await workerFetch("/api/v1/tools");
      expect(resp.status).toBe(200);

      const body = await resp.json() as { tools: unknown[] };
      expect(body).toHaveProperty("tools");
      expect(Array.isArray(body.tools)).toBe(true);
      expect(body.tools.length).toBeGreaterThan(0);
    });

    it("contains semantic_search, file_skeleton, where_used, callers", async () => {
      const resp = await workerFetch("/api/v1/tools");
      const body = await resp.json() as {
        tools: Array<{ name: string }>;
      };

      const toolNames = body.tools.map((t) => t.name);
      expect(toolNames).toContain("semantic_search");
      expect(toolNames).toContain("file_skeleton");
      expect(toolNames).toContain("where_used");
      expect(toolNames).toContain("callers");
    });

    it("contains diagnose_error, pr_risk, refactor_impact, unanswered_questions", async () => {
      const resp = await workerFetch("/api/v1/tools");
      const body = await resp.json() as {
        tools: Array<{ name: string }>;
      };

      const toolNames = body.tools.map((t) => t.name);
      expect(toolNames).toContain("diagnose_error");
      expect(toolNames).toContain("pr_risk");
      expect(toolNames).toContain("refactor_impact");
      expect(toolNames).toContain("unanswered_questions");
    });

    it("each tool has name, description, input_schema, and pricing", async () => {
      const resp = await workerFetch("/api/v1/tools");
      const body = await resp.json() as {
        tools: Array<{
          name: string;
          description: string;
          input_schema: Record<string, unknown>;
          pricing: { free_monthly_calls: number; overage_per_call_cents: number };
        }>;
      };

      for (const tool of body.tools) {
        expect(typeof tool.name).toBe("string");
        expect(tool.name.length).toBeGreaterThan(0);

        expect(typeof tool.description).toBe("string");
        expect(tool.description.length).toBeGreaterThan(0);

        expect(tool.input_schema).toBeDefined();
        expect(typeof tool.input_schema).toBe("object");

        expect(tool.pricing).toBeDefined();
        expect(typeof tool.pricing.free_monthly_calls).toBe("number");
        expect(typeof tool.pricing.overage_per_call_cents).toBe("number");
      }
    });
  });

  describe("POST /api/v1/tools/call", () => {
    it("returns 401 without auth", async () => {
      const resp = await workerFetch("/api/v1/tools/call", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ tool: "semantic_search", input: { query: "test" } }),
      });

      expect(resp.status).toBe(401);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("unauthorized");
    });

    it("returns 400 with missing tool field", async () => {
      // We need auth for this to not 401 first. Import the helper.
      const { generateTestJwt } = await import("./helpers");
      const token = await generateTestJwt();

      const resp = await workerFetch("/api/v1/tools/call", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({ input: { query: "test" } }),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("invalid_request");
    });

    it("returns 400 with missing input field", async () => {
      const { generateTestJwt } = await import("./helpers");
      const token = await generateTestJwt();

      const resp = await workerFetch("/api/v1/tools/call", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify({ tool: "semantic_search" }),
      });

      expect(resp.status).toBe(400);
      const body = await resp.json() as { error: string };
      expect(body.error).toBe("invalid_request");
    });
  });
});
