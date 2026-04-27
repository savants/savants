import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { listGraphScopes } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const graphs = new Hono<HonoEnv>();

// GET /api/v1/graphs - List graphs for the org
graphs.get("/", async (c) => {
  const auth = c.get("auth");
  const scopes = await listGraphScopes(c.env.DB, auth.orgId);

  return c.json({
    org_id: auth.orgId,
    graphs: scopes.map((s) => ({
      id: s.id,
      name: s.graph_name,
      source_type: s.source_type,
      source_url: s.source_url,
      created_at: s.created_at,
    })),
  });
});

export default graphs;
