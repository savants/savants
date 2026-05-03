/**
 * Causal inference engine for the savants graph.
 *
 * Given an event (error, crash, alert), traces the graph backwards to find
 * the most likely cause. Uses three signals:
 *
 *   1. Structural proximity - how close are the nodes in the graph (hops)
 *   2. Temporal decay - how close in time (exponential decay)
 *   3. Historical frequency - has this pattern happened before
 *
 * P(A caused B) = structural_proximity * temporal_decay * historical_frequency
 *
 * This is not Granger causality or Pearl's do-calculus in full generality.
 * It's a pragmatic scoring function that works well for infrastructure
 * root cause analysis where the graph structure encodes domain knowledge.
 */

import type { Env } from "../lib/types";

interface CausalCandidate {
  node_id: string;
  node_name: string;
  node_type: string;
  file_path: string | null;
  event_type: string;
  event_title: string;
  event_time: number;
  causal_score: number;
  structural_hops: number;
  temporal_distance_sec: number;
  historical_occurrences: number;
  explanation: string;
}

interface CausalResult {
  effect: { type: string; title: string; time: number };
  probable_causes: CausalCandidate[];
  confidence: number;
  reasoning: string;
}

// Temporal decay half-life: events within 5 minutes are strongly correlated,
// events 1 hour apart are weakly correlated
const TEMPORAL_HALFLIFE_SEC = 300; // 5 minutes

// Max hops to search for causes
const MAX_HOPS = 4;

// Minimum score to include as a candidate
const MIN_SCORE = 0.05;

export async function findCauses(
  db: Env["DB"],
  projectId: string,
  input: {
    event_type?: string;    // "pod_crash", "error", "alert"
    node_name?: string;     // "createJob", "cert-manager-cainjector"
    event_time?: number;    // unix timestamp (default: now)
    lookback_minutes?: number; // how far back to search (default: 60)
  }
): Promise<CausalResult> {
  const eventTime = input.event_time || Math.floor(Date.now() / 1000);
  const lookbackSec = (input.lookback_minutes || 60) * 60;
  const sinceTime = eventTime - lookbackSec;

  // Step 1: Find the effect node
  let effectNode: any = null;
  if (input.node_name) {
    effectNode = await db.prepare(
      "SELECT id, name, type, file_path FROM graph_nodes WHERE project_id = ?1 AND name = ?2 LIMIT 1"
    ).bind(projectId, input.node_name).first();
  }

  // Step 2: Find all events in the time window BEFORE the effect
  const recentEvents = await db.prepare(`
    SELECT e.id, e.type, e.title, e.severity, e.occurred_at, e.node_id, e.metadata,
           n.name as node_name, n.type as node_type, n.file_path
    FROM graph_events e
    LEFT JOIN graph_nodes n ON n.id = e.node_id
    WHERE e.project_id = ?1
      AND e.occurred_at >= ?2
      AND e.occurred_at <= ?3
    ORDER BY e.occurred_at DESC
    LIMIT 50
  `).bind(projectId, sinceTime, eventTime).all();

  // Step 3: Find recent audit log entries (deploys, config changes, agent findings)
  const recentAudit = await db.prepare(`
    SELECT id, action, resource_type, resource_id, metadata, created_at
    FROM audit_log
    WHERE org_id = (SELECT org_id FROM projects WHERE id = ?1)
      AND created_at >= ?2
      AND created_at <= ?3
      AND action IN ('agent.notify', 'tool.call')
    ORDER BY created_at DESC
    LIMIT 30
  `).bind(projectId, sinceTime, eventTime).all();

  const candidates: CausalCandidate[] = [];

  // Step 4: Score each event as a potential cause
  for (const event of recentEvents.results as any[]) {
    const timeDelta = eventTime - event.occurred_at;
    if (timeDelta <= 0) continue; // Skip events after the effect

    // Structural proximity: if we have the effect node, calculate hops
    let structuralScore = 0.3; // default if no graph path
    let hops = MAX_HOPS;

    if (effectNode && event.node_id) {
      const path = await findShortestPath(db, projectId, event.node_id, effectNode.id);
      if (path !== null) {
        hops = path;
        structuralScore = 1.0 / (1.0 + hops); // 1 hop = 0.5, 2 hops = 0.33, etc.
      }
    }

    // Same node type or direct relationship boosts score
    if (event.node_name === input.node_name) {
      structuralScore = 1.0;
      hops = 0;
    }

    // Temporal decay: exponential decay based on time difference
    const temporalScore = Math.exp(-timeDelta / TEMPORAL_HALFLIFE_SEC);

    // Historical frequency: how many times has this event type preceded the effect type
    let historicalScore = 0.1; // default
    const historicalCount = await db.prepare(`
      SELECT COUNT(*) as c FROM graph_events
      WHERE project_id = ?1 AND type = ?2 AND node_id = ?3
    `).bind(projectId, event.type, event.node_id || "").first<{ c: number }>();

    if (historicalCount && historicalCount.c > 1) {
      historicalScore = Math.min(historicalCount.c / 10.0, 1.0); // caps at 10 occurrences
    }

    // Combined causal score
    const causalScore = structuralScore * temporalScore * historicalScore;

    if (causalScore >= MIN_SCORE) {
      const minutesAgo = Math.round(timeDelta / 60);

      candidates.push({
        node_id: event.node_id || "",
        node_name: event.node_name || event.type,
        node_type: event.node_type || "event",
        file_path: event.file_path,
        event_type: event.type,
        event_title: event.title,
        event_time: event.occurred_at,
        causal_score: Math.round(causalScore * 1000) / 1000,
        structural_hops: hops,
        temporal_distance_sec: timeDelta,
        historical_occurrences: historicalCount?.c || 0,
        explanation: `${event.title} occurred ${minutesAgo}m before the incident. `
          + `Structural distance: ${hops} hops. `
          + `Similar pattern seen ${historicalCount?.c || 0} times before.`,
      });
    }
  }

  // Score audit events (deploys, config changes)
  for (const entry of recentAudit.results as any[]) {
    const meta = JSON.parse(entry.metadata || "{}");
    if (meta.category === "deploy" || meta.category === "security" || entry.action === "agent.notify") {
      const timeDelta = eventTime - entry.created_at;
      if (timeDelta <= 0) continue;

      const temporalScore = Math.exp(-timeDelta / TEMPORAL_HALFLIFE_SEC);
      const severityBoost = meta.severity === "critical" ? 1.5 : meta.severity === "warning" ? 1.0 : 0.5;
      const causalScore = 0.4 * temporalScore * severityBoost;

      if (causalScore >= MIN_SCORE) {
        const minutesAgo = Math.round(timeDelta / 60);
        candidates.push({
          node_id: entry.resource_id || "",
          node_name: meta.agent_name || meta.title || entry.action,
          node_type: meta.category || "audit",
          file_path: null,
          event_type: entry.action,
          event_title: meta.title || entry.action,
          event_time: entry.created_at,
          causal_score: Math.round(causalScore * 1000) / 1000,
          structural_hops: MAX_HOPS,
          temporal_distance_sec: timeDelta,
          historical_occurrences: 0,
          explanation: `${meta.title || entry.action} occurred ${minutesAgo}m before. Severity: ${meta.severity || "unknown"}.`,
        });
      }
    }
  }

  // Sort by causal score descending
  candidates.sort((a, b) => b.causal_score - a.causal_score);
  const topCandidates = candidates.slice(0, 10);

  // Overall confidence: based on the top candidate's score
  const confidence = topCandidates.length > 0 ? topCandidates[0].causal_score : 0;

  // Generate reasoning narrative
  let reasoning = "";
  if (topCandidates.length === 0) {
    reasoning = "No probable causes found in the time window. The issue may be external or the graph lacks relevant data.";
  } else {
    const top = topCandidates[0];
    reasoning = `Most probable cause: "${top.event_title}" (score: ${top.causal_score}). `;
    reasoning += `It occurred ${Math.round(top.temporal_distance_sec / 60)}m before the incident`;
    if (top.structural_hops < MAX_HOPS) {
      reasoning += `, is ${top.structural_hops} hop${top.structural_hops !== 1 ? "s" : ""} away in the dependency graph`;
    }
    if (top.historical_occurrences > 1) {
      reasoning += `, and this pattern has been seen ${top.historical_occurrences} times before`;
    }
    reasoning += ".";

    if (topCandidates.length > 1) {
      reasoning += ` ${topCandidates.length - 1} other potential causes identified with lower confidence.`;
    }
  }

  return {
    effect: {
      type: input.event_type || "unknown",
      title: input.node_name || "unknown",
      time: eventTime,
    },
    probable_causes: topCandidates,
    confidence: Math.round(confidence * 100) / 100,
    reasoning,
  };
}

/**
 * Find shortest path between two nodes using BFS on edges.
 * Returns hop count or null if no path within MAX_HOPS.
 */
async function findShortestPath(
  db: Env["DB"],
  projectId: string,
  fromNodeId: string,
  toNodeId: string
): Promise<number | null> {
  if (fromNodeId === toNodeId) return 0;

  // BFS using recursive CTE
  const result = await db.prepare(`
    WITH RECURSIVE path(node_id, depth) AS (
      SELECT ?1, 0
      UNION
      SELECT CASE
        WHEN e.source_node = path.node_id THEN e.target_node
        ELSE e.source_node
      END, path.depth + 1
      FROM graph_edges e
      JOIN path ON (e.source_node = path.node_id OR e.target_node = path.node_id)
      WHERE e.project_id = ?3 AND path.depth < ?4
    )
    SELECT MIN(depth) as hops FROM path WHERE node_id = ?2
  `).bind(fromNodeId, toNodeId, projectId, MAX_HOPS).first<{ hops: number | null }>();

  return result?.hops ?? null;
}

/**
 * MCP tool: find probable causes for an incident.
 */
export async function toolFindCauses(
  db: Env["DB"],
  projectId: string,
  input: Record<string, unknown>
): Promise<Record<string, unknown>> {
  const result = await findCauses(db, projectId, {
    event_type: input.event_type as string,
    node_name: input.node_name as string || input.function as string || input.pod as string,
    event_time: input.event_time as number,
    lookback_minutes: input.lookback_minutes as number,
  });

  return result as unknown as Record<string, unknown>;
}
