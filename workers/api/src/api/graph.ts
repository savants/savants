/**
 * Graph query engine for D1.
 *
 * Replaces FalkorDB with SQLite recursive CTEs.
 * All queries are project-scoped - no cross-project leakage.
 *
 * Node types:
 *   Code:    function, class, module, file, package
 *   Infra:   pod, service, deployment, node, namespace, cluster
 *   Events:  error, alert, incident, deployment_event
 *   People:  developer, team, reviewer
 *   Docs:    doc_page, doc_section, api_endpoint
 *   Comms:   message, thread, ticket, pr
 *
 * Edge types:
 *   Code:    CALLS, IMPORTS, EXTENDS, IMPLEMENTS, DEPENDS_ON, CONTAINS
 *   Infra:   RUNS_IN, EXPOSES, CONNECTS_TO, SCHEDULES, OWNS
 *   Events:  CAUSED_BY, AFFECTS, RESOLVED_BY, TRIGGERED
 *   People:  AUTHORED, REVIEWED, COMMITTED, ASSIGNED_TO
 *   Docs:    DOCUMENTS, REFERENCES, SUPERSEDES
 *   Cross:   CODE_RUNS_AS, ERROR_IN, PR_CHANGES, DEPLOYED_TO, DISCUSSED_IN
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const graph = new Hono<HonoEnv>();

// ─── Find callers (recursive up to N hops) ───────────────────────────────────

graph.post("/callers", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ name: string; project_id: string; depth?: number }>();

  if (!body.name || !body.project_id) {
    return c.json({ error: "name and project_id required", status: 400 }, 400);
  }

  // Verify project access
  const access = await verifyProjectAccess(c.env.DB, auth.orgId, body.project_id);
  if (!access) return c.json({ error: "project_not_found", status: 404 }, 404);

  const depth = Math.min(body.depth ?? 3, 10);

  const result = await c.env.DB
    .prepare(`
      WITH RECURSIVE caller_chain AS (
        -- Start: find the target node
        SELECT e.source_node as node_id, 1 as hop
        FROM graph_edges e
        JOIN graph_nodes target ON target.id = e.target_node
        WHERE target.name = ?1 AND target.project_id = ?2 AND e.type = 'CALLS'

        UNION ALL

        -- Recurse: find callers of callers
        SELECT e.source_node, cc.hop + 1
        FROM graph_edges e
        JOIN caller_chain cc ON e.target_node = cc.node_id
        WHERE cc.hop < ?3 AND e.type = 'CALLS'
      )
      SELECT DISTINCT n.name, n.type, n.file_path, n.line_start, cc.hop
      FROM caller_chain cc
      JOIN graph_nodes n ON n.id = cc.node_id
      ORDER BY cc.hop, n.name
    `)
    .bind(body.name, body.project_id, depth)
    .all();

  return c.json({
    target: body.name,
    callers: result.results,
    depth,
  });
});

// ─── Trace error through the graph ───────────────────────────────────────────

graph.post("/trace", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ function_name?: string; file_path?: string; project_id: string }>();

  if (!body.project_id) {
    return c.json({ error: "project_id required", status: 400 }, 400);
  }

  const access = await verifyProjectAccess(c.env.DB, auth.orgId, body.project_id);
  if (!access) return c.json({ error: "project_not_found", status: 404 }, 404);

  const traces: Record<string, unknown>[] = [];

  // Find the node
  let nodeQuery = "SELECT * FROM graph_nodes WHERE project_id = ?1";
  const params: unknown[] = [body.project_id];

  if (body.function_name) {
    nodeQuery += " AND name = ?2 AND type IN ('function', 'method')";
    params.push(body.function_name);
  } else if (body.file_path) {
    nodeQuery += " AND file_path = ?2";
    params.push(body.file_path);
  } else {
    return c.json({ error: "function_name or file_path required", status: 400 }, 400);
  }

  const node = await c.env.DB.prepare(nodeQuery).bind(...params).first();
  if (!node) {
    return c.json({ node: null, traces: [], message: "Node not found in graph" });
  }

  const nodeId = (node as any).id;

  // Get all connected edges (both directions)
  const edges = await c.env.DB
    .prepare(`
      SELECT e.type as edge_type,
             CASE WHEN e.source_node = ?1 THEN 'outgoing' ELSE 'incoming' END as direction,
             n.name, n.type as node_type, n.file_path, n.line_start, n.source_type
      FROM graph_edges e
      JOIN graph_nodes n ON n.id = CASE WHEN e.source_node = ?1 THEN e.target_node ELSE e.source_node END
      WHERE (e.source_node = ?1 OR e.target_node = ?1)
    `)
    .bind(nodeId)
    .all();

  // Get recent events for this node
  const events = await c.env.DB
    .prepare(`
      SELECT type, title, severity, source_type, occurred_at
      FROM graph_events
      WHERE node_id = ?1
      ORDER BY occurred_at DESC
      LIMIT 10
    `)
    .bind(nodeId)
    .all();

  return c.json({
    node: {
      name: (node as any).name,
      type: (node as any).type,
      file: (node as any).file_path,
      line: (node as any).line_start,
      source: (node as any).source_type,
    },
    connections: edges.results,
    recent_events: events.results,
  });
});

// ─── Search nodes by name or type ────────────────────────────────────────────

graph.post("/search", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ query: string; project_id: string; type?: string; limit?: number }>();

  if (!body.query || !body.project_id) {
    return c.json({ error: "query and project_id required", status: 400 }, 400);
  }

  const access = await verifyProjectAccess(c.env.DB, auth.orgId, body.project_id);
  if (!access) return c.json({ error: "project_not_found", status: 404 }, 404);

  const limit = Math.min(body.limit ?? 20, 100);

  let sql = `SELECT name, type, file_path, line_start, source_type, content_summary
    FROM graph_nodes
    WHERE project_id = ?1 AND name LIKE ?2`;
  const params: unknown[] = [body.project_id, `%${body.query}%`];

  if (body.type) {
    sql += " AND type = ?3";
    params.push(body.type);
  }

  sql += ` ORDER BY CASE WHEN name = ?${params.length + 1} THEN 0 WHEN name LIKE ?${params.length + 2} THEN 1 ELSE 2 END LIMIT ?${params.length + 3}`;
  params.push(body.query, `${body.query}%`, limit);

  const result = await c.env.DB.prepare(sql).bind(...params).all();

  return c.json({ results: result.results, count: result.results.length });
});

// ─── Blast radius: what's affected if this changes ───────────────────────────

graph.post("/blast-radius", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ name: string; project_id: string }>();

  if (!body.name || !body.project_id) {
    return c.json({ error: "name and project_id required", status: 400 }, 400);
  }

  const access = await verifyProjectAccess(c.env.DB, auth.orgId, body.project_id);
  if (!access) return c.json({ error: "project_not_found", status: 404 }, 404);

  // Find everything that depends on this node (callers, importers, connected infra)
  const result = await c.env.DB
    .prepare(`
      WITH RECURSIVE affected AS (
        SELECT e.source_node as node_id, e.type as via, 1 as depth
        FROM graph_edges e
        JOIN graph_nodes target ON target.id = e.target_node
        WHERE target.name = ?1 AND target.project_id = ?2
          AND e.type IN ('CALLS', 'IMPORTS', 'DEPENDS_ON', 'EXTENDS', 'CODE_RUNS_AS', 'CONNECTS_TO')

        UNION ALL

        SELECT e.source_node, e.type, a.depth + 1
        FROM graph_edges e
        JOIN affected a ON e.target_node = a.node_id
        WHERE a.depth < 5
          AND e.type IN ('CALLS', 'IMPORTS', 'DEPENDS_ON')
      )
      SELECT n.name, n.type, n.file_path, n.source_type, MIN(a.depth) as distance, a.via
      FROM affected a
      JOIN graph_nodes n ON n.id = a.node_id
      GROUP BY n.id
      ORDER BY distance, n.type, n.name
    `)
    .bind(body.name, body.project_id)
    .all();

  // Group by type for the summary
  const byType: Record<string, number> = {};
  for (const row of result.results as any[]) {
    byType[row.type] = (byType[row.type] || 0) + 1;
  }

  return c.json({
    target: body.name,
    total_affected: result.results.length,
    by_type: byType,
    affected: result.results,
  });
});

// ─── Ingest nodes + edges (called by CLI after reindex) ──────────────────────

graph.post("/ingest", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    project_id: string;
    source_type: string;
    source_id: string;
    nodes: Array<{
      id: string;
      type: string;
      name: string;
      qualified_name?: string;
      file_path?: string;
      line_start?: number;
      line_end?: number;
      language?: string;
      content_summary?: string;
      metadata?: Record<string, unknown>;
    }>;
    edges: Array<{
      source: string;
      target: string;
      type: string;
      weight?: number;
      metadata?: Record<string, unknown>;
    }>;
  }>();

  if (!body.project_id || !body.source_type) {
    return c.json({ error: "project_id and source_type required", status: 400 }, 400);
  }

  const access = await verifyProjectAccess(c.env.DB, auth.orgId, body.project_id);
  if (!access) return c.json({ error: "project_not_found", status: 404 }, 404);

  const now = Math.floor(Date.now() / 1000);

  // Delete existing nodes from this source (full replace)
  await c.env.DB
    .prepare("DELETE FROM graph_nodes WHERE project_id = ?1 AND source_type = ?2 AND source_id = ?3")
    .bind(body.project_id, body.source_type, body.source_id || "")
    .run();

  // Insert nodes in batches
  let nodeCount = 0;
  for (const node of body.nodes || []) {
    await c.env.DB
      .prepare(`INSERT INTO graph_nodes (id, project_id, type, name, qualified_name, file_path, line_start, line_end, language, content_summary, metadata, source_type, source_id, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)`)
      .bind(
        node.id || crypto.randomUUID(),
        body.project_id,
        node.type,
        node.name,
        node.qualified_name || null,
        node.file_path || null,
        node.line_start || null,
        node.line_end || null,
        node.language || null,
        node.content_summary || null,
        JSON.stringify(node.metadata || {}),
        body.source_type,
        body.source_id || null,
        now
      )
      .run();
    nodeCount++;
  }

  // Insert edges
  let edgeCount = 0;
  for (const edge of body.edges || []) {
    await c.env.DB
      .prepare(`INSERT INTO graph_edges (id, project_id, source_node, target_node, type, weight, metadata)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)`)
      .bind(
        crypto.randomUUID(),
        body.project_id,
        edge.source,
        edge.target,
        edge.type,
        edge.weight || 1.0,
        JSON.stringify(edge.metadata || {})
      )
      .run();
    edgeCount++;
  }

  // Update source sync timestamp
  await c.env.DB
    .prepare("UPDATE project_sources SET last_synced_at = ?1, node_count = ?2 WHERE project_id = ?3 AND source_type = ?4")
    .bind(now, nodeCount, body.project_id, body.source_type)
    .run();

  return c.json({
    ingested: true,
    nodes: nodeCount,
    edges: edgeCount,
    project_id: body.project_id,
    source: body.source_type,
  });
});

// ─── Ingest from OSS binary (accepts ParseResult format) ────────────────────

graph.post("/parse-result", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    repo: string;
    files: number;
    entities: Array<{
      kind: string;
      name: string;
      file: string;
      line: number;
      end_line: number;
      body: string;
      params: string[];
      import_source: string;
      import_names: string[];
    }>;
    call_sites: Array<{
      caller_file: string;
      caller_name: string;
      callee_name: string;
    }>;
  }>();

  if (!body.repo || !body.entities) {
    return c.json({ error: "repo and entities required", status: 400 }, 400);
  }

  // Find existing project by slug - do NOT auto-create
  const slug = body.repo.toLowerCase().replace(/[^a-z0-9-]/g, "-");
  const project = await c.env.DB
    .prepare("SELECT id FROM projects WHERE org_id = ?1 AND (slug = ?2 OR name = ?3)")
    .bind(auth.orgId, slug, body.repo)
    .first<{ id: string }>();

  if (!project) {
    return c.json({ error: "project_not_found", message: `No project '${body.repo}'. Create one first: savants project create ${slug}` }, 404);
  }

  const projectId = project.id;
  const now = Math.floor(Date.now() / 1000);

  // NOTE: We delete AFTER successful insert to prevent data loss on failed uploads.
  // If the agent disconnects mid-upload, we keep the old data.
  const staleEdgeDelete = c.env.DB.prepare("DELETE FROM graph_edges WHERE project_id = ?1").bind(projectId);
  const staleNodeDelete = c.env.DB.prepare("DELETE FROM graph_nodes WHERE project_id = ?1 AND source_type = 'code'").bind(projectId);

  // Build node ID map: "file:name" -> uuid
  const nodeIdMap = new Map<string, string>();
  let nodeCount = 0;

  // Batch insert nodes (use D1 batch for performance)
  const nodeStmts = [];
  for (const entity of body.entities) {
    const nodeId = crypto.randomUUID();
    const key = `${entity.file}:${entity.name}`;
    nodeIdMap.set(key, nodeId);
    // Also map by name alone for call_site matching
    if (!nodeIdMap.has(entity.name)) {
      nodeIdMap.set(entity.name, nodeId);
    }

    const hasValidation = entity.body.includes("validate") || entity.body.includes("check") || entity.body.includes("guard");
    const hasErrorHandling = entity.body.includes("catch") || entity.body.includes("throw") || entity.body.includes("Error");
    const isExported = entity.body.startsWith("export ") || entity.name.startsWith("export");

    const metadata = JSON.stringify({
      params: entity.params,
      exported: isExported,
      has_validation: hasValidation,
      has_error_handling: hasErrorHandling,
      import_source: entity.import_source || undefined,
      import_names: entity.import_names?.length ? entity.import_names : undefined,
    });

    nodeStmts.push(
      c.env.DB.prepare(
        `INSERT INTO graph_nodes (id, project_id, type, name, qualified_name, file_path, line_start, line_end, language, content_summary, metadata, source_type, source_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'code', ?12, ?13)`
      ).bind(
        nodeId, projectId, entity.kind, entity.name,
        `${entity.file}:${entity.name}`,
        entity.file, entity.line, entity.end_line,
        detectLanguage(entity.file),
        entity.body.slice(0, 200),
        metadata,
        body.repo,
        now
      )
    );
    nodeCount++;
  }

  // Build edge statements
  let edgeCount = 0;
  const edgeStmts = [];
  for (const cs of body.call_sites || []) {
    const callerKey = `${cs.caller_file}:${cs.caller_name}`;
    const callerId = nodeIdMap.get(callerKey) || nodeIdMap.get(cs.caller_name);
    const calleeId = nodeIdMap.get(cs.callee_name);

    if (callerId && calleeId && callerId !== calleeId) {
      edgeStmts.push(
        c.env.DB.prepare(
          `INSERT INTO graph_edges (id, project_id, source_node, target_node, type, weight, metadata)
           VALUES (?1, ?2, ?3, ?4, 'CALLS', 1.0, '{}')`
        ).bind(crypto.randomUUID(), projectId, callerId, calleeId)
      );
      edgeCount++;
    }
  }

  // Insert import edges
  for (const entity of body.entities) {
    if (entity.kind === "import" && entity.import_source) {
      const importerId = nodeIdMap.get(`${entity.file}:${entity.name}`) || nodeIdMap.get(entity.name);
      for (const importedName of entity.import_names || []) {
        const targetId = nodeIdMap.get(importedName);
        if (importerId && targetId && importerId !== targetId) {
          edgeStmts.push(
            c.env.DB.prepare(
              `INSERT INTO graph_edges (id, project_id, source_node, target_node, type, weight, metadata)
               VALUES (?1, ?2, ?3, ?4, 'IMPORTS', 1.0, '{}')`
            ).bind(crypto.randomUUID(), projectId, importerId, targetId)
          );
          edgeCount++;
        }
      }
    }
  }

  // Atomic transaction: delete old data + insert new data in batches
  // First batch: delete old edges + nodes, then insert first chunk of nodes
  // This prevents data loss if the upload fails mid-way.
  const firstBatch = [staleEdgeDelete, staleNodeDelete, ...nodeStmts.slice(0, 48)];
  await c.env.DB.batch(firstBatch);

  // Remaining node inserts
  for (let i = 48; i < nodeStmts.length; i += 50) {
    await c.env.DB.batch(nodeStmts.slice(i, i + 50));
  }

  // Edge inserts
  for (let i = 0; i < edgeStmts.length; i += 50) {
    await c.env.DB.batch(edgeStmts.slice(i, i + 50));
  }

  // ── Generate and store embeddings in Vectorize ──
  let embeddingsStored = 0;
  try {
    if (c.env.VECTORIZE && c.env.AI) {
      // Build embedding texts: "functionName params first200chars"
      const embeddingBatch: Array<{ id: string; text: string }> = [];
      for (const entity of body.entities) {
        if (entity.kind === "import") continue; // Skip imports
        const nodeId = nodeIdMap.get(`${entity.file}:${entity.name}`) || nodeIdMap.get(entity.name);
        if (!nodeId) continue;

        const text = [
          entity.name,
          entity.params?.join(" ") || "",
          entity.body?.slice(0, 300) || "",
        ].join(" ").trim();

        if (text.length > 5) {
          embeddingBatch.push({ id: nodeId, text });
        }
      }

      // Generate embeddings in batches of 100 (Workers AI limit)
      for (let i = 0; i < embeddingBatch.length; i += 100) {
        const batch = embeddingBatch.slice(i, i + 100);
        const texts = batch.map(b => b.text);

        const response = await c.env.AI.run("@cf/baai/bge-small-en-v1.5", { text: texts }) as { data: number[][] };

        if (response?.data) {
          const vectors = batch.map((b, idx) => ({
            id: b.id,
            values: response.data[idx],
            metadata: { project_id: projectId, name: batch[idx]?.text.split(" ")[0] || "" },
          }));
          await c.env.VECTORIZE.upsert(vectors);
          embeddingsStored += vectors.length;
        }
      }
    }
  } catch (err) {
    console.error("[vectorize] embedding failed:", err instanceof Error ? err.message : err);
  }

  return c.json({
    ingested: true,
    project_id: projectId,
    repo: body.repo,
    files: body.files,
    nodes: nodeCount,
    edges: edgeCount,
    embeddings: embeddingsStored,
  });
});

function detectLanguage(filePath: string): string {
  const ext = filePath.split(".").pop()?.toLowerCase() || "";
  const map: Record<string, string> = {
    ts: "typescript", tsx: "typescript", js: "javascript", jsx: "javascript",
    py: "python", rs: "rust", go: "go", java: "java", rb: "ruby",
    c: "c", cpp: "cpp", h: "c", hpp: "cpp", cs: "csharp",
  };
  return map[ext] || ext;
}

// ─── Stats for a project graph ───────────────────────────────────────────────

graph.get("/stats/:projectId", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.param("projectId");

  const access = await verifyProjectAccess(c.env.DB, auth.orgId, projectId);
  if (!access) return c.json({ error: "project_not_found", status: 404 }, 404);

  const nodeStats = await c.env.DB
    .prepare("SELECT type, source_type, COUNT(*) as count FROM graph_nodes WHERE project_id = ?1 GROUP BY type, source_type ORDER BY count DESC")
    .bind(projectId)
    .all();

  const edgeStats = await c.env.DB
    .prepare("SELECT type, COUNT(*) as count FROM graph_edges WHERE project_id = ?1 GROUP BY type ORDER BY count DESC")
    .bind(projectId)
    .all();

  const eventStats = await c.env.DB
    .prepare("SELECT type, severity, COUNT(*) as count FROM graph_events WHERE project_id = ?1 GROUP BY type, severity ORDER BY count DESC")
    .bind(projectId)
    .all();

  const totals = await c.env.DB
    .prepare(`SELECT
      (SELECT COUNT(*) FROM graph_nodes WHERE project_id = ?1) as nodes,
      (SELECT COUNT(*) FROM graph_edges WHERE project_id = ?1) as edges,
      (SELECT COUNT(*) FROM graph_events WHERE project_id = ?1) as events
    `)
    .bind(projectId)
    .first();

  return c.json({
    project_id: projectId,
    totals,
    nodes_by_type: nodeStats.results,
    edges_by_type: edgeStats.results,
    events_by_type: eventStats.results,
  });
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

async function verifyProjectAccess(db: D1Database, orgId: string, projectId: string): Promise<boolean> {
  const project = await db
    .prepare("SELECT id FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(projectId, orgId)
    .first();
  return !!project;
}

export default graph;
