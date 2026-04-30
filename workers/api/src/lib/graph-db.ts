/**
 * Per-org graph database manager.
 *
 * Each org gets an isolated D1 database for their graph data.
 * The shared DB stores the mapping: org_id → graph_database_id.
 *
 * Architecture:
 *   savants-cloud (shared D1):     users, orgs, billing, credits
 *   savants-graph-{slug} (per-org): graph_nodes, graph_edges, graph_events
 *
 * Why per-org:
 *   - No cross-tenant data leakage (impossible, not just ACL'd)
 *   - Each org's queries only touch their data (fast)
 *   - D1 10GB limit per database is per-org, not global
 *   - One org's large graph doesn't slow down others
 *   - Can geo-locate a org's DB near their team
 *
 * Cloudflare D1 supports this natively:
 *   - Unlimited databases per account
 *   - Create via API in milliseconds
 *   - Each DB scales independently
 */

import type { Env } from "./types";

const GRAPH_SCHEMA = `
CREATE TABLE IF NOT EXISTS graph_nodes (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  type TEXT NOT NULL,
  name TEXT NOT NULL,
  qualified_name TEXT,
  file_path TEXT,
  line_start INTEGER,
  line_end INTEGER,
  language TEXT,
  content_summary TEXT,
  metadata TEXT DEFAULT '{}',
  source_type TEXT NOT NULL,
  source_id TEXT,
  content_hash TEXT,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_nodes_project ON graph_nodes(project_id, type);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON graph_nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_file ON graph_nodes(project_id, file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_source ON graph_nodes(source_type, source_id);

CREATE TABLE IF NOT EXISTS graph_edges (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  source_node TEXT NOT NULL,
  target_node TEXT NOT NULL,
  type TEXT NOT NULL,
  weight REAL DEFAULT 1.0,
  metadata TEXT DEFAULT '{}',
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_edges_project ON graph_edges(project_id);
CREATE INDEX IF NOT EXISTS idx_edges_source ON graph_edges(source_node, type);
CREATE INDEX IF NOT EXISTS idx_edges_target ON graph_edges(target_node, type);

CREATE TABLE IF NOT EXISTS graph_events (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL,
  type TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT,
  severity TEXT DEFAULT 'info',
  node_id TEXT,
  source_type TEXT NOT NULL,
  source_ref TEXT,
  metadata TEXT DEFAULT '{}',
  occurred_at INTEGER NOT NULL DEFAULT (unixepoch()),
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_events_project ON graph_events(project_id, occurred_at);
CREATE INDEX IF NOT EXISTS idx_events_node ON graph_events(node_id);
`;

/**
 * Get or create the graph database for an org.
 *
 * For now, uses the shared DB (single-tenant mode).
 * When scaling, this will create per-org D1 databases via API.
 *
 * The switch is transparent to callers - they just call getGraphDb()
 * and get back a D1Database instance.
 */
export async function getGraphDb(env: Env, orgId: string): Promise<D1Database> {
  // Phase 1 (current): use the shared DB
  // All graph tables have project_id for isolation within the shared DB
  return env.DB;

  // Phase 2 (at scale): per-org databases
  // Uncomment when ready to scale:
  //
  // // Check if org has a dedicated graph DB
  // const org = await env.DB
  //   .prepare("SELECT graph_db_id FROM orgs WHERE id = ?1")
  //   .bind(orgId)
  //   .first<{ graph_db_id: string | null }>();
  //
  // if (org?.graph_db_id) {
  //   // Return the org's dedicated DB
  //   return getD1ById(env, org.graph_db_id);
  // }
  //
  // // Create a new DB for this org
  // const slug = await getOrgSlug(env.DB, orgId);
  // const dbName = `savants-graph-${slug}`;
  // const dbId = await createD1Database(env, dbName);
  //
  // // Initialize schema
  // const db = getD1ById(env, dbId);
  // await initGraphSchema(db);
  //
  // // Store the mapping
  // await env.DB
  //   .prepare("UPDATE orgs SET graph_db_id = ?1 WHERE id = ?2")
  //   .bind(dbId, orgId)
  //   .run();
  //
  // return db;
}

/**
 * Initialize the graph schema on a new database.
 */
export async function initGraphSchema(db: D1Database): Promise<void> {
  const statements = GRAPH_SCHEMA
    .split(";")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);

  for (const stmt of statements) {
    await db.prepare(stmt + ";").run();
  }
}

// Phase 2 helpers (for per-org DB creation via Cloudflare API):
//
// async function createD1Database(env: Env, name: string): Promise<string> {
//   const res = await fetch(
//     `https://api.cloudflare.com/client/v4/accounts/${env.CF_ACCOUNT_ID}/d1/database`,
//     {
//       method: "POST",
//       headers: {
//         Authorization: `Bearer ${env.CF_API_TOKEN}`,
//         "Content-Type": "application/json",
//       },
//       body: JSON.stringify({ name }),
//     }
//   );
//   const data = await res.json<{ result: { uuid: string } }>();
//   return data.result.uuid;
// }
//
// function getD1ById(env: Env, dbId: string): D1Database {
//   // Cloudflare Workers can dynamically bind to D1 databases
//   // via the D1 API. This requires the database ID.
//   // Implementation depends on Cloudflare's dynamic binding API.
// }
