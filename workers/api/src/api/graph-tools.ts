/**
 * D1-backed graph tools - ported from FalkorDB Cypher queries.
 *
 * These tools query graph_nodes, graph_edges, graph_events in D1
 * using recursive CTEs for transitive traversals.
 */

import type { Env } from "../lib/types";

type D1 = Env["DB"];

interface ToolResult {
  [key: string]: unknown;
}

// ── graph_stats ──
export async function graphStats(db: D1, projectId: string): Promise<ToolResult> {
  const [nodes, edges, events] = await Promise.all([
    db.prepare("SELECT COUNT(*) as c, type FROM graph_nodes WHERE project_id = ?1 GROUP BY type").bind(projectId).all(),
    db.prepare("SELECT COUNT(*) as c, type FROM graph_edges WHERE project_id = ?1 GROUP BY type").bind(projectId).all(),
    db.prepare("SELECT COUNT(*) as c FROM graph_events WHERE project_id = ?1").bind(projectId).first<{ c: number }>(),
  ]);

  const nodesByType: Record<string, number> = {};
  for (const r of nodes.results as any[]) nodesByType[r.type] = r.c;
  const edgesByType: Record<string, number> = {};
  for (const r of edges.results as any[]) edgesByType[r.type] = r.c;

  const totalNodes = Object.values(nodesByType).reduce((a, b) => a + b, 0);
  const totalEdges = Object.values(edgesByType).reduce((a, b) => a + b, 0);

  return {
    total_nodes: totalNodes,
    total_edges: totalEdges,
    total_events: events?.c ?? 0,
    nodes_by_type: nodesByType,
    edges_by_type: edgesByType,
  };
}

// ── function_xray ──
export async function functionXray(db: D1, projectId: string, input: { function_name: string; file_path?: string }): Promise<ToolResult> {
  let query = "SELECT * FROM graph_nodes WHERE project_id = ?1 AND name = ?2";
  const params: unknown[] = [projectId, input.function_name];
  if (input.file_path) {
    query += " AND file_path LIKE ?3";
    params.push(`%${input.file_path}`);
  }
  query += " LIMIT 1";

  const node = await db.prepare(query).bind(...params).first<{
    id: string; name: string; type: string; file_path: string; line_start: number; line_end: number;
    language: string; content_summary: string; metadata: string;
  }>();

  if (!node) return { error: `Function '${input.function_name}' not found in graph` };

  const [callers, callees, events] = await Promise.all([
    db.prepare(`
      SELECT n.name, n.type, n.file_path, n.line_start
      FROM graph_edges e JOIN graph_nodes n ON n.id = e.source_node
      WHERE e.target_node = ?1 AND e.type = 'CALLS'
      ORDER BY n.name LIMIT 20
    `).bind(node.id).all(),
    db.prepare(`
      SELECT n.name, n.type, n.file_path, n.line_start
      FROM graph_edges e JOIN graph_nodes n ON n.id = e.target_node
      WHERE e.source_node = ?1 AND e.type = 'CALLS'
      ORDER BY n.name LIMIT 20
    `).bind(node.id).all(),
    db.prepare(`
      SELECT type, title, severity, occurred_at
      FROM graph_events WHERE node_id = ?1
      ORDER BY occurred_at DESC LIMIT 10
    `).bind(node.id).all(),
  ]);

  let meta: Record<string, unknown> = {};
  try { meta = JSON.parse(node.metadata || "{}"); } catch {}

  return {
    name: node.name,
    type: node.type,
    file: node.file_path,
    lines: { start: node.line_start, end: node.line_end },
    language: node.language,
    summary: node.content_summary,
    params: (meta as any).params || [],
    exported: !!(meta as any).exported,
    callers: (callers.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
    })),
    callees: (callees.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
    })),
    recent_events: (events.results as any[]).map(r => ({
      type: r.type, title: r.title, severity: r.severity, at: r.occurred_at,
    })),
    caller_count: callers.results.length,
    callee_count: callees.results.length,
  };
}

// ── blast_radius ──
export async function blastRadius(db: D1, projectId: string, input: { function: string; depth?: number }): Promise<ToolResult> {
  const maxDepth = input.depth ?? 3;

  const node = await db.prepare(
    "SELECT id, name, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
  ).bind(projectId, input.function).first<{ id: string; name: string; file_path: string; line_start: number }>();

  if (!node) return { error: `Function '${input.function}' not found in graph` };

  // Recursive CTE: find all transitive callers (who depends on this function)
  const result = await db.prepare(`
    WITH RECURSIVE blast(node_id, depth) AS (
      SELECT ?1, 0
      UNION
      SELECT e.source_node, blast.depth + 1
      FROM graph_edges e
      JOIN blast ON blast.node_id = e.target_node
      WHERE e.type = 'CALLS' AND blast.depth < ?2
    )
    SELECT DISTINCT n.name, n.type, n.file_path, n.line_start, b.depth
    FROM blast b
    JOIN graph_nodes n ON n.id = b.node_id
    WHERE b.node_id != ?1
    ORDER BY b.depth, n.name
    LIMIT 50
  `).bind(node.id, maxDepth).all();

  const affected = (result.results as any[]).map(r => ({
    name: r.name, type: r.type, file: r.file_path, line: r.line_start, depth: r.depth,
  }));

  const affectedFiles = [...new Set(affected.map(a => a.file).filter(Boolean))];

  return {
    function: input.function,
    file: node.file_path,
    line: node.line_start,
    blast_radius: affected.length,
    affected_files: affectedFiles.length,
    affected,
    risk: affected.length > 20 ? "high" : affected.length > 5 ? "medium" : "low",
  };
}

// ── impact_analysis ──
export async function impactAnalysis(db: D1, projectId: string, input: { function_name: string; max_depth?: number }): Promise<ToolResult> {
  return blastRadius(db, projectId, { function: input.function_name, depth: input.max_depth ?? 5 });
}

// ── dead_code ──
export async function deadCode(db: D1, projectId: string, input: { file?: string }): Promise<ToolResult> {
  let query = `
    SELECT n.name, n.type, n.file_path, n.line_start, n.line_end
    FROM graph_nodes n
    WHERE n.project_id = ?1
      AND n.type IN ('function', 'method')
      AND NOT EXISTS (
        SELECT 1 FROM graph_edges e
        WHERE e.target_node = n.id AND e.type = 'CALLS'
      )
  `;
  const params: unknown[] = [projectId];
  if (input.file) {
    query += " AND n.file_path LIKE ?2";
    params.push(`%${input.file}`);
  }
  query += " ORDER BY n.file_path, n.line_start LIMIT 50";

  const result = await db.prepare(query).bind(...params).all();
  const candidates = (result.results as any[]).map(r => ({
    name: r.name, type: r.type, file: r.file_path, line: r.line_start,
  }));

  return {
    dead_code_candidates: candidates.length,
    candidates,
    note: "Functions with zero callers in the graph. Exported functions may still be used externally.",
  };
}

// ── import_tree ──
export async function importTree(db: D1, projectId: string, input: { file: string; depth?: number }): Promise<ToolResult> {
  const maxDepth = input.depth ?? 2;

  // Find the file's node (or use file_path matching)
  const result = await db.prepare(`
    WITH RECURSIVE tree(file_path, depth) AS (
      SELECT DISTINCT n.file_path, 0
      FROM graph_nodes n
      WHERE n.project_id = ?1 AND n.file_path LIKE ?2
      UNION
      SELECT DISTINCT tn.file_path, tree.depth + 1
      FROM tree
      JOIN graph_nodes sn ON sn.file_path = tree.file_path AND sn.project_id = ?1
      JOIN graph_edges e ON e.source_node = sn.id AND e.type = 'IMPORTS'
      JOIN graph_nodes tn ON tn.id = e.target_node
      WHERE tree.depth < ?3
    )
    SELECT DISTINCT file_path, depth FROM tree ORDER BY depth, file_path
  `).bind(projectId, `%${input.file}`, maxDepth).all();

  const tree: Record<number, string[]> = {};
  for (const r of result.results as any[]) {
    if (!tree[r.depth]) tree[r.depth] = [];
    tree[r.depth].push(r.file_path);
  }

  return {
    root: input.file,
    depth: maxDepth,
    total_files: result.results.length,
    tree,
  };
}

// ── module_exports ──
export async function moduleExports(db: D1, projectId: string, input: { file: string }): Promise<ToolResult> {
  const result = await db.prepare(`
    SELECT name, type, line_start, metadata
    FROM graph_nodes
    WHERE project_id = ?1 AND file_path LIKE ?2
      AND type IN ('function', 'method', 'class', 'interface', 'type')
    ORDER BY line_start
  `).bind(projectId, `%${input.file}`).all();

  const exports = (result.results as any[]).filter(r => {
    try {
      const meta = JSON.parse(r.metadata || "{}");
      return meta.exported;
    } catch { return false; }
  }).map(r => {
    let params: string[] = [];
    try { params = JSON.parse(r.metadata || "{}").params || []; } catch {}
    return { name: r.name, type: r.type, line: r.line_start, params };
  });

  const internal = (result.results as any[]).length - exports.length;

  return {
    file: input.file,
    exports,
    internal_count: internal,
    total: result.results.length,
  };
}

// ── search_code ──
export async function searchCode(db: D1, projectId: string, input: { pattern: string }): Promise<ToolResult> {
  const result = await db.prepare(`
    SELECT name, type, file_path, line_start, qualified_name
    FROM graph_nodes
    WHERE project_id = ?1 AND name LIKE ?2
    ORDER BY name LIMIT 30
  `).bind(projectId, `%${input.pattern}%`).all();

  return {
    pattern: input.pattern,
    results: (result.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
      qualified_name: r.qualified_name,
    })),
    count: result.results.length,
  };
}

// ── find_references_structured ──
export async function findReferences(db: D1, projectId: string, input: { function_name: string; include_tests?: boolean }): Promise<ToolResult> {
  let query = `
    SELECT n.name, n.type, n.file_path, n.line_start, e.type as edge_type
    FROM graph_edges e
    JOIN graph_nodes n ON n.id = e.source_node
    JOIN graph_nodes t ON t.id = e.target_node
    WHERE t.project_id = ?1 AND t.name = ?2
  `;
  const params: unknown[] = [projectId, input.function_name];
  if (input.include_tests === false) {
    query += " AND n.file_path NOT LIKE '%test%' AND n.file_path NOT LIKE '%spec%'";
  }
  query += " ORDER BY n.file_path, n.line_start LIMIT 50";

  const result = await db.prepare(query).bind(...params).all();

  const refs = (result.results as any[]).map(r => ({
    name: r.name, type: r.type, file: r.file_path, line: r.line_start,
    relationship: r.edge_type,
  }));

  // Group by file
  const byFile: Record<string, typeof refs> = {};
  for (const ref of refs) {
    const f = ref.file || "unknown";
    if (!byFile[f]) byFile[f] = [];
    byFile[f].push(ref);
  }

  return {
    symbol: input.function_name,
    total_references: refs.length,
    files_count: Object.keys(byFile).length,
    by_file: byFile,
  };
}

// ── dependency_chain ──
export async function dependencyChain(db: D1, projectId: string, input: { from_file: string; to_file: string }): Promise<ToolResult> {
  // BFS via recursive CTE to find shortest path between two files
  const result = await db.prepare(`
    WITH RECURSIVE chain(file_path, path, depth) AS (
      SELECT DISTINCT n.file_path, n.file_path, 0
      FROM graph_nodes n
      WHERE n.project_id = ?1 AND n.file_path LIKE ?2
      UNION
      SELECT DISTINCT tn.file_path, chain.path || ' -> ' || tn.file_path, chain.depth + 1
      FROM chain
      JOIN graph_nodes sn ON sn.file_path = chain.file_path AND sn.project_id = ?1
      JOIN graph_edges e ON e.source_node = sn.id AND e.type IN ('IMPORTS', 'CALLS')
      JOIN graph_nodes tn ON tn.id = e.target_node
      WHERE chain.depth < 6 AND tn.file_path != chain.file_path
    )
    SELECT path, depth FROM chain
    WHERE file_path LIKE ?3
    ORDER BY depth LIMIT 1
  `).bind(projectId, `%${input.from_file}`, `%${input.to_file}`).all();

  if (result.results.length === 0) {
    return { from: input.from_file, to: input.to_file, connected: false, message: "No dependency path found within 6 hops" };
  }

  const r = result.results[0] as any;
  return {
    from: input.from_file,
    to: input.to_file,
    connected: true,
    hops: r.depth,
    path: r.path.split(" -> "),
  };
}

// ── risk_score ──
export async function riskScore(db: D1, projectId: string, input: { function_name: string; file_path?: string }): Promise<ToolResult> {
  let nodeQuery = "SELECT id, name, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND name = ?2";
  const params: unknown[] = [projectId, input.function_name];
  if (input.file_path) {
    nodeQuery += " AND file_path LIKE ?3";
    params.push(`%${input.file_path}`);
  }
  nodeQuery += " LIMIT 1";

  const node = await db.prepare(nodeQuery).bind(...params).first<{ id: string; name: string; file_path: string; line_start: number }>();
  if (!node) return { error: `Function '${input.function_name}' not found` };

  const [callerCount, calleeCount, eventCount] = await Promise.all([
    db.prepare("SELECT COUNT(*) as c FROM graph_edges WHERE target_node = ?1 AND type = 'CALLS'").bind(node.id).first<{ c: number }>(),
    db.prepare("SELECT COUNT(*) as c FROM graph_edges WHERE source_node = ?1 AND type = 'CALLS'").bind(node.id).first<{ c: number }>(),
    db.prepare("SELECT COUNT(*) as c FROM graph_events WHERE node_id = ?1 AND severity IN ('error', 'critical')").bind(node.id).first<{ c: number }>(),
  ]);

  const callers = callerCount?.c ?? 0;
  const callees = calleeCount?.c ?? 0;
  const errors = eventCount?.c ?? 0;

  // Score: 0-10
  let score = 0;
  score += Math.min(callers * 0.5, 3);       // blast radius: up to 3
  score += Math.min(callees * 0.3, 2);       // complexity: up to 2
  score += Math.min(errors * 1.5, 3);        // error history: up to 3
  score += callers > 10 ? 1 : 0;            // hub penalty
  score += errors > 3 ? 1 : 0;              // reliability penalty
  score = Math.min(Math.round(score * 10) / 10, 10);

  return {
    function: input.function_name,
    file: node.file_path,
    risk_score: score,
    risk_level: score >= 7 ? "high" : score >= 4 ? "medium" : "low",
    factors: {
      callers,
      callees,
      recent_errors: errors,
      is_hub: callers > 10,
    },
  };
}

// ── community_summary ──
export async function communitySummary(db: D1, projectId: string, input: { max_results?: number }): Promise<ToolResult> {
  const limit = input.max_results ?? 10;

  const result = await db.prepare(`
    SELECT n.name, n.type, n.file_path, n.line_start,
      (SELECT COUNT(*) FROM graph_edges e WHERE (e.source_node = n.id OR e.target_node = n.id)) as connections
    FROM graph_nodes n
    WHERE n.project_id = ?1 AND n.type IN ('function', 'method', 'class')
    ORDER BY connections DESC
    LIMIT ?2
  `).bind(projectId, limit).all();

  return {
    hubs: (result.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
      connections: r.connections,
    })),
    note: "Most connected nodes in the codebase. These are high-risk change targets.",
  };
}

// ── decorated_with ──
export async function decoratedWith(db: D1, projectId: string, input: { decorator_name: string }): Promise<ToolResult> {
  const result = await db.prepare(`
    SELECT name, type, file_path, line_start, metadata
    FROM graph_nodes
    WHERE project_id = ?1 AND metadata LIKE ?2
    ORDER BY file_path, line_start LIMIT 30
  `).bind(projectId, `%${input.decorator_name}%`).all();

  return {
    decorator: input.decorator_name,
    functions: (result.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
    })),
    count: result.results.length,
  };
}

// ── pre_change_warning ──
export async function preChangeWarning(db: D1, projectId: string, input: { function_name: string; file_path?: string }): Promise<ToolResult> {
  const [risk, blast] = await Promise.all([
    riskScore(db, projectId, input),
    blastRadius(db, projectId, { function: input.function_name, depth: 3 }),
  ]);

  const warnings: string[] = [];
  const riskLevel = (risk as any).risk_score ?? 0;
  const blastCount = (blast as any).blast_radius ?? 0;

  if (riskLevel >= 7) warnings.push(`HIGH RISK (${riskLevel}/10): This function has significant blast radius and/or error history`);
  if (blastCount > 10) warnings.push(`${blastCount} functions transitively depend on this - test thoroughly`);
  if ((risk as any).factors?.recent_errors > 0) warnings.push(`${(risk as any).factors.recent_errors} recent errors on this function`);
  if ((risk as any).factors?.is_hub) warnings.push("This is a hub function (>10 callers) - consider backward compatibility");

  return {
    function: input.function_name,
    risk_score: riskLevel,
    blast_radius: blastCount,
    warnings: warnings.length > 0 ? warnings : ["Low risk - safe to modify"],
    risk_details: risk,
    blast_details: blast,
  };
}

// ── coupling_check ──
export async function couplingCheck(db: D1, projectId: string, input: { from_module: string; to_module: string }): Promise<ToolResult> {
  // Find existing edges between the two modules
  const result = await db.prepare(`
    SELECT sn.name as from_name, sn.file_path as from_file,
           tn.name as to_name, tn.file_path as to_file,
           e.type as edge_type
    FROM graph_edges e
    JOIN graph_nodes sn ON sn.id = e.source_node
    JOIN graph_nodes tn ON tn.id = e.target_node
    WHERE sn.project_id = ?1
      AND sn.file_path LIKE ?2
      AND tn.file_path LIKE ?3
    ORDER BY sn.file_path, tn.file_path LIMIT 50
  `).bind(projectId, `%${input.from_module}%`, `%${input.to_module}%`).all();

  const edges = (result.results as any[]).map(r => ({
    from: `${r.from_name} (${r.from_file})`,
    to: `${r.to_name} (${r.to_file})`,
    type: r.edge_type,
  }));

  return {
    from_module: input.from_module,
    to_module: input.to_module,
    existing_coupling: edges.length,
    edges,
    verdict: edges.length > 5
      ? "High coupling already exists between these modules"
      : edges.length > 0
        ? "Some coupling exists - adding more is acceptable"
        : "No existing coupling - adding a dependency creates a new architectural boundary crossing",
  };
}

// ── co_change_partners ──
export async function coChangePartners(db: D1, projectId: string, input: { function_name: string; limit?: number }): Promise<ToolResult> {
  // Functions that share the same callers/callees (proxy for co-change without git history)
  const node = await db.prepare(
    "SELECT id FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
  ).bind(projectId, input.function_name).first<{ id: string }>();

  if (!node) return { error: `Function '${input.function_name}' not found` };

  const maxResults = input.limit ?? 10;

  // Find functions called by the same callers (sibling functions)
  const result = await db.prepare(`
    SELECT n.name, n.file_path, n.line_start, COUNT(*) as shared_callers
    FROM graph_edges e1
    JOIN graph_edges e2 ON e2.source_node = e1.source_node AND e2.target_node != ?1
    JOIN graph_nodes n ON n.id = e2.target_node
    WHERE e1.target_node = ?1 AND e1.type = 'CALLS' AND e2.type = 'CALLS'
    GROUP BY n.id
    ORDER BY shared_callers DESC
    LIMIT ?2
  `).bind(node.id, maxResults).all();

  return {
    function: input.function_name,
    partners: (result.results as any[]).map(r => ({
      name: r.name, file: r.file_path, line: r.line_start,
      shared_callers: r.shared_callers,
    })),
    note: "Functions that share callers with the target - they likely change together.",
  };
}

// ── resolves_to ──
export async function resolvesTo(db: D1, projectId: string, input: { symbol: string }): Promise<ToolResult> {
  const [exactMatch, mentions] = await Promise.all([
    db.prepare(`
      SELECT name, type, file_path, line_start, qualified_name
      FROM graph_nodes WHERE project_id = ?1 AND name = ?2
      LIMIT 10
    `).bind(projectId, input.symbol).all(),
    db.prepare(`
      SELECT name, type, file_path, line_start
      FROM graph_nodes WHERE project_id = ?1 AND content_summary LIKE ?2
      LIMIT 20
    `).bind(projectId, `%${input.symbol}%`).all(),
  ]);

  return {
    symbol: input.symbol,
    exact_matches: (exactMatch.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
    })),
    mentions: (mentions.results as any[]).map(r => ({
      name: r.name, type: r.type, file: r.file_path, line: r.line_start,
    })),
  };
}

// ── cluster_state ──
export async function clusterState(db: D1, projectId: string, input: { cluster?: string }): Promise<ToolResult> {
  const cluster = input.cluster || "%";
  const [pods, deployments, services, namespaces] = await Promise.all([
    db.prepare(`
      SELECT json_extract(metadata, '$.status') as status, COUNT(*) as c
      FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_pod' AND name LIKE ?2
      GROUP BY status
    `).bind(projectId, `%${cluster}%`).all(),
    db.prepare("SELECT COUNT(*) as c FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_deployment'").bind(projectId).first<{ c: number }>(),
    db.prepare("SELECT COUNT(*) as c FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_service'").bind(projectId).first<{ c: number }>(),
    db.prepare("SELECT DISTINCT json_extract(metadata, '$.namespace') as ns FROM graph_nodes WHERE project_id = ?1 AND type IN ('k8s_pod', 'k8s_deployment')").bind(projectId).all(),
  ]);

  const podsByStatus: Record<string, number> = {};
  let totalPods = 0;
  for (const r of pods.results as any[]) {
    podsByStatus[r.status || "Unknown"] = r.c;
    totalPods += r.c;
  }

  return {
    pods: { total: totalPods, by_status: podsByStatus },
    deployments: deployments?.c ?? 0,
    services: services?.c ?? 0,
    namespaces: (namespaces.results as any[]).map(r => r.ns).filter(Boolean),
  };
}

// ── list_pods ──
export async function listPods(db: D1, projectId: string, input: { namespace?: string; status?: string; name_contains?: string }): Promise<ToolResult> {
  let query = "SELECT name, file_path, metadata FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_pod'";
  const params: unknown[] = [projectId];
  let idx = 2;

  if (input.namespace) {
    query += ` AND json_extract(metadata, '$.namespace') = ?${idx}`;
    params.push(input.namespace);
    idx++;
  }
  if (input.status) {
    query += ` AND json_extract(metadata, '$.status') = ?${idx}`;
    params.push(input.status);
    idx++;
  }
  if (input.name_contains) {
    query += ` AND name LIKE ?${idx}`;
    params.push(`%${input.name_contains}%`);
    idx++;
  }
  query += " ORDER BY name LIMIT 50";

  const result = await db.prepare(query).bind(...params).all();
  return {
    pods: (result.results as any[]).map(r => {
      let meta: any = {};
      try { meta = JSON.parse(r.metadata || "{}"); } catch {}
      return { name: r.name, namespace: meta.namespace, status: meta.status, image: meta.image };
    }),
    count: result.results.length,
  };
}

// ── pod_story ──
export async function podStory(db: D1, projectId: string, input: { pod?: string; namespace?: string; since_minutes?: number; min_severity?: string; limit?: number }): Promise<ToolResult> {
  const sinceMinutes = input.since_minutes ?? 60;
  const minSeverity = input.min_severity ?? "WARN";
  const maxResults = input.limit ?? 15;

  const sevRank: Record<string, number> = { info: 0, warn: 1, error: 2, critical: 3, fatal: 3 };
  const minRank = sevRank[minSeverity.toLowerCase()] ?? 1;
  const allowedSeverities = Object.entries(sevRank).filter(([, v]) => v >= minRank).map(([k]) => k);

  let query = `
    SELECT e.title, e.description, e.severity, e.occurred_at, n.name as node_name
    FROM graph_events e
    LEFT JOIN graph_nodes n ON n.id = e.node_id
    WHERE e.project_id = ?1
  `;
  const params: unknown[] = [projectId];
  let idx = 2;

  if (sinceMinutes > 0) {
    const since = Math.floor(Date.now() / 1000) - sinceMinutes * 60;
    query += ` AND e.occurred_at >= ?${idx}`;
    params.push(since);
    idx++;
  }

  if (input.pod) {
    query += ` AND n.name LIKE ?${idx}`;
    params.push(`%${input.pod}%`);
    idx++;
  }

  // Filter severity in SQL
  const sevPlaceholders = allowedSeverities.map((_, i) => `?${idx + i}`).join(", ");
  query += ` AND e.severity IN (${sevPlaceholders})`;
  params.push(...allowedSeverities);

  query += ` ORDER BY e.occurred_at DESC LIMIT ?${idx + allowedSeverities.length}`;
  params.push(maxResults);

  const result = await db.prepare(query).bind(...params).all();

  return {
    events: (result.results as any[]).map(r => ({
      pod: r.node_name, title: r.title, description: r.description,
      severity: r.severity, at: r.occurred_at,
    })),
    count: result.results.length,
  };
}

// ── host_state ──
export async function hostState(db: D1, projectId: string, input: { hostname?: string }): Promise<ToolResult> {
  let query = "SELECT name, metadata FROM graph_nodes WHERE project_id = ?1 AND type = 'host'";
  const params: unknown[] = [projectId];
  if (input.hostname) {
    query += " AND name = ?2";
    params.push(input.hostname);
  }
  query += " ORDER BY name LIMIT 10";

  const result = await db.prepare(query).bind(...params).all();
  if (result.results.length === 0) {
    return { error: input.hostname ? `Host '${input.hostname}' not found` : "No hosts in graph. Run the host ingestor." };
  }

  return {
    hosts: (result.results as any[]).map(r => {
      let meta: any = {};
      try { meta = JSON.parse(r.metadata || "{}"); } catch {}
      return {
        hostname: r.name,
        os: meta.os, kernel: meta.kernel,
        cpu_percent: meta.cpu_percent, memory_used_mb: meta.memory_used_mb,
        memory_total_mb: meta.memory_total_mb, load_1m: meta.load_1m,
        uptime_seconds: meta.uptime_seconds,
        failed_units: meta.failed_units || [],
      };
    }),
  };
}

// ── host_story ──
export async function hostStory(db: D1, projectId: string, input: { hostname?: string; since_minutes?: number; min_severity?: string; limit?: number }): Promise<ToolResult> {
  return podStory(db, projectId, {
    pod: input.hostname,
    since_minutes: input.since_minutes,
    min_severity: input.min_severity,
    limit: input.limit,
  });
}

// ── deployment_info ──
export async function deploymentInfo(db: D1, projectId: string, input: { namespace: string; name: string }): Promise<ToolResult> {
  const deploy = await db.prepare(`
    SELECT id, name, metadata FROM graph_nodes
    WHERE project_id = ?1 AND type IN ('k8s_deployment', 'k8s_statefulset', 'k8s_daemonset')
      AND name = ?2 AND json_extract(metadata, '$.namespace') = ?3
    LIMIT 1
  `).bind(projectId, input.name, input.namespace).first<{ id: string; name: string; metadata: string }>();

  if (!deploy) return { error: `No deployment '${input.name}' in namespace '${input.namespace}'` };

  let meta: any = {};
  try { meta = JSON.parse(deploy.metadata || "{}"); } catch {}

  // Find pods owned by this deployment
  const pods = await db.prepare(`
    SELECT n.name, json_extract(n.metadata, '$.status') as status
    FROM graph_edges e
    JOIN graph_nodes n ON n.id = e.target_node
    WHERE e.source_node = ?1 AND e.type = 'OWNS' AND n.type = 'k8s_pod'
    ORDER BY n.name
  `).bind(deploy.id).all();

  return {
    name: deploy.name,
    namespace: input.namespace,
    replicas: { desired: meta.replicas_desired, ready: meta.replicas_ready, available: meta.replicas_available },
    image: meta.image,
    labels: meta.labels,
    pods: (pods.results as any[]).map(r => ({ name: r.name, status: r.status })),
  };
}

// ── pod_dependencies ──
export async function podDependencies(db: D1, projectId: string, input: { namespace: string; pod: string }): Promise<ToolResult> {
  const podNode = await db.prepare(`
    SELECT id FROM graph_nodes
    WHERE project_id = ?1 AND type = 'k8s_pod' AND name = ?2
      AND json_extract(metadata, '$.namespace') = ?3
    LIMIT 1
  `).bind(projectId, input.pod, input.namespace).first<{ id: string }>();

  if (!podNode) return { error: `Pod '${input.pod}' not found in namespace '${input.namespace}'` };

  const deps = await db.prepare(`
    SELECT n.name, n.type, json_extract(n.metadata, '$.namespace') as ns
    FROM graph_edges e
    JOIN graph_nodes n ON n.id = e.target_node
    WHERE e.source_node = ?1 AND e.type IN ('READS_CONFIG', 'READS_SECRET', 'DEPENDS_ON')
    ORDER BY n.type, n.name
  `).bind(podNode.id).all();

  return {
    pod: input.pod,
    namespace: input.namespace,
    dependencies: (deps.results as any[]).map(r => ({
      name: r.name, type: r.type, namespace: r.ns,
    })),
    count: deps.results.length,
  };
}

// ── namespace_summary ──
export async function namespaceSummary(db: D1, projectId: string, input: { namespace: string }): Promise<ToolResult> {
  const [deployments, pods, services, configmaps, secrets] = await Promise.all([
    db.prepare(`SELECT name, json_extract(metadata, '$.replicas_ready') as ready FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_deployment' AND json_extract(metadata, '$.namespace') = ?2`).bind(projectId, input.namespace).all(),
    db.prepare(`SELECT name, json_extract(metadata, '$.status') as status FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_pod' AND json_extract(metadata, '$.namespace') = ?2`).bind(projectId, input.namespace).all(),
    db.prepare(`SELECT COUNT(*) as c FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_service' AND json_extract(metadata, '$.namespace') = ?2`).bind(projectId, input.namespace).first<{ c: number }>(),
    db.prepare(`SELECT COUNT(*) as c FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_configmap' AND json_extract(metadata, '$.namespace') = ?2`).bind(projectId, input.namespace).first<{ c: number }>(),
    db.prepare(`SELECT COUNT(*) as c FROM graph_nodes WHERE project_id = ?1 AND type = 'k8s_secret' AND json_extract(metadata, '$.namespace') = ?2`).bind(projectId, input.namespace).first<{ c: number }>(),
  ]);

  const podsByStatus: Record<string, string[]> = {};
  for (const r of pods.results as any[]) {
    const s = r.status || "Unknown";
    if (!podsByStatus[s]) podsByStatus[s] = [];
    podsByStatus[s].push(r.name);
  }

  return {
    namespace: input.namespace,
    deployments: (deployments.results as any[]).map(r => ({ name: r.name, ready: r.ready })),
    pods: { total: pods.results.length, by_status: podsByStatus },
    services: services?.c ?? 0,
    configmaps: configmaps?.c ?? 0,
    secrets: secrets?.c ?? 0,
  };
}

// ── Dispatcher ──
// ── Core code tools (D1-backed) ──

async function callersD1(db: D1, projectId: string, input: { function: string; depth?: number }): Promise<ToolResult> {
  const fname = input.function;
  const depth = input.depth || 5;

  // Find the target function
  const target = await db.prepare(
    "SELECT id, name, file_path, line_start FROM graph_nodes WHERE project_id = ?1 AND name = ?2 AND type = 'function' LIMIT 1"
  ).bind(projectId, fname).first<{ id: string; name: string; file_path: string; line_start: number }>();

  if (!target) return { error: `Function '${fname}' not found in graph` };

  // Recursive caller walk
  const chain: string[] = [];
  let currentId = target.id;
  for (let i = 0; i < depth; i++) {
    const caller = await db.prepare(`
      SELECT n.id, n.name, n.file_path, n.line_start
      FROM graph_edges e JOIN graph_nodes n ON n.id = e.source_node
      WHERE e.target_node = ?1 AND e.type = 'CALLS'
      ORDER BY n.name LIMIT 1
    `).bind(currentId).first<{ id: string; name: string; file_path: string; line_start: number }>();
    if (!caller) break;
    chain.push(`${caller.name} (${caller.file_path}:${caller.line_start})`);
    currentId = caller.id;
  }

  // All direct callers
  const allCallers = await db.prepare(`
    SELECT n.name, n.file_path, n.line_start
    FROM graph_edges e JOIN graph_nodes n ON n.id = e.source_node
    WHERE e.target_node = ?1 AND e.type = 'CALLS'
    ORDER BY n.name LIMIT 20
  `).bind(target.id).all();

  const callerList = (allCallers.results as any[]).map(r => `${r.name} (${r.file_path}:${r.line_start})`);
  let text = `=== Callers of ${fname} (${callerList.length}) ===\n`;
  text += callerList.map(c => `  ${c}`).join("\n");
  if (chain.length > 0) {
    text += `\n\nCaller chain (${chain.length} levels up):\n  ${chain.join("\n  -> ")}`;
  }
  return { text };
}

async function whereUsedD1(db: D1, projectId: string, input: { symbol: string }): Promise<ToolResult> {
  const symbol = input.symbol;
  const node = await db.prepare(
    "SELECT id, name, file_path FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
  ).bind(projectId, symbol).first<{ id: string; name: string; file_path: string }>();

  if (!node) return { error: `Symbol '${symbol}' not found in graph` };

  const callers = await db.prepare(`
    SELECT n.name, n.file_path, n.line_start FROM graph_edges e
    JOIN graph_nodes n ON n.id = e.source_node
    WHERE e.target_node = ?1 ORDER BY n.file_path, n.name LIMIT 30
  `).bind(node.id).all();

  const importers = await db.prepare(`
    SELECT n.name, n.file_path FROM graph_edges e
    JOIN graph_nodes n ON n.id = e.source_node
    WHERE e.target_node = ?1 AND e.type = 'IMPORTS' LIMIT 20
  `).bind(node.id).all();

  let text = `=== Where '${symbol}' is used ===\n\nCallers (${callers.results.length}):\n`;
  text += (callers.results as any[]).map(r => `  ${r.name} (${r.file_path}:${r.line_start})`).join("\n");
  if ((importers.results as any[]).length > 0) {
    text += `\n\nImported by (${importers.results.length}):\n`;
    text += (importers.results as any[]).map(r => `  ${r.name} (${r.file_path})`).join("\n");
  }
  return { text };
}

async function fileSkeletonD1(db: D1, projectId: string, input: { file: string }): Promise<ToolResult> {
  const file = input.file;
  const nodes = await db.prepare(
    "SELECT name, type, line_start, line_end FROM graph_nodes WHERE project_id = ?1 AND file_path LIKE ?2 AND type = 'function' ORDER BY line_start"
  ).bind(projectId, `%${file}`).all();

  if (nodes.results.length === 0) return { error: `No functions found in '${file}'` };

  let text = `=== ${file} ===\nFunctions:\n`;
  text += (nodes.results as any[]).map(n => `  ${n.name}() (line ${n.line_start})`).join("\n");
  return { text };
}

async function semanticSearchD1(db: D1, projectId: string, input: { query?: string; pattern?: string }): Promise<ToolResult> {
  const query = input.query || input.pattern || "";
  // D1 doesn't have vector search - fall back to LIKE matching on function names
  const keywords = query.toLowerCase().split(/\s+/).filter(w => w.length > 3);
  if (keywords.length === 0) return { error: "Query too short" };

  // Search by name LIKE for each keyword
  const results = await db.prepare(`
    SELECT name, file_path, line_start, type FROM graph_nodes
    WHERE project_id = ?1 AND type = 'function'
    AND (${keywords.map((_, i) => `LOWER(name) LIKE ?${i + 2}`).join(" OR ")})
    ORDER BY name LIMIT 15
  `).bind(projectId, ...keywords.map(k => `%${k}%`)).all();

  if (results.results.length === 0) return { text: `No results for '${query}'` };

  let text = `=== Search: '${query}' (${results.results.length} results) ===\n`;
  text += (results.results as any[]).map(r => `  ${r.file_path}:${r.line_start} ${r.name}()`).join("\n");
  return { text };
}

export async function executeGraphTool(
  db: D1,
  projectId: string,
  tool: string,
  input: Record<string, unknown>
): Promise<ToolResult> {
  switch (tool) {
    case "graph_stats":
      return graphStats(db, projectId);
    case "function_xray":
      return functionXray(db, projectId, input as any);
    case "blast_radius":
      return blastRadius(db, projectId, input as any);
    case "impact_analysis":
      return impactAnalysis(db, projectId, input as any);
    case "dead_code":
      return deadCode(db, projectId, input as any);
    case "import_tree":
      return importTree(db, projectId, input as any);
    case "module_exports":
      return moduleExports(db, projectId, input as any);
    case "search_code":
      return searchCode(db, projectId, input as any);
    case "find_references":
      return findReferences(db, projectId, input as any);
    case "dependency_chain":
      return dependencyChain(db, projectId, input as any);
    case "risk_score":
      return riskScore(db, projectId, input as any);
    case "community_summary":
      return communitySummary(db, projectId, input as any);
    case "decorated_with":
      return decoratedWith(db, projectId, input as any);
    case "pre_change_warning":
      return preChangeWarning(db, projectId, input as any);
    case "coupling_check":
      return couplingCheck(db, projectId, input as any);
    case "co_change_partners":
      return coChangePartners(db, projectId, input as any);
    case "resolves_to":
      return resolvesTo(db, projectId, input as any);
    // K8s / infra tools
    case "cluster_state":
      return clusterState(db, projectId, input as any);
    case "list_pods":
      return listPods(db, projectId, input as any);
    case "pod_story":
      return podStory(db, projectId, input as any);
    case "host_state":
      return hostState(db, projectId, input as any);
    case "host_story":
      return hostStory(db, projectId, input as any);
    case "deployment_info":
      return deploymentInfo(db, projectId, input as any);
    case "pod_dependencies":
      return podDependencies(db, projectId, input as any);
    case "namespace_summary":
      return namespaceSummary(db, projectId, input as any);
    // Core code tools (callers, where_used, etc)
    case "callers":
      return callersD1(db, projectId, input as any);
    case "where_used":
      return whereUsedD1(db, projectId, input as any);
    case "file_skeleton":
      return fileSkeletonD1(db, projectId, input as any);
    case "semantic_search":
      return semanticSearchD1(db, projectId, input as any);
    default:
      return { error: `Unknown graph tool: ${tool}` };
  }
}

// List of all graph tool names for routing
export const GRAPH_TOOL_NAMES = [
  // Code graph
  "graph_stats", "function_xray", "blast_radius", "impact_analysis",
  "dead_code", "import_tree", "module_exports", "search_code",
  "find_references", "dependency_chain", "risk_score", "community_summary",
  "decorated_with", "pre_change_warning", "coupling_check",
  "co_change_partners", "resolves_to",
  // Core tools (also available via cloud D1)
  "callers", "where_used", "file_skeleton", "semantic_search",
  // K8s / infra
  "cluster_state", "list_pods", "pod_story", "host_state", "host_story",
  "deployment_info", "pod_dependencies", "namespace_summary",
];
