/**
 * Cloudflare tools - monitor Workers, D1, KV, tunnels, DNS.
 *
 * Uses the Cloudflare API with a stored API token.
 * Graph-enriched: Worker errors linked to code graph.
 */

import type { Env } from "../lib/types";
import { getIntegration } from "../db/queries";

async function getCFCreds(db: Env["DB"], orgId: string): Promise<{ token: string; accountId: string } | null> {
  const row = await getIntegration(db, orgId, "cloudflare");
  if (!row) return null;
  const config = JSON.parse(row.config || "{}");
  const creds = JSON.parse(row.credentials || "{}");
  const token = creds.api_token || creds.token || config.token;
  const accountId = config.account_id || creds.account_id;
  if (!token || !accountId) return null;
  return { token, accountId };
}

async function cfGet(token: string, url: string): Promise<any> {
  const res = await fetch(url, {
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    signal: AbortSignal.timeout(10000),
  });
  if (!res.ok) return null;
  const data = await res.json<any>();
  return data.success ? data.result : data;
}

// ── list_workers ──
async function listWorkers(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const creds = await getCFCreds(db, orgId);
  if (!creds) return { error: "Cloudflare not connected" };
  const data = await cfGet(creds.token, `https://api.cloudflare.com/client/v4/accounts/${creds.accountId}/workers/scripts`);
  if (!data) return { workers: [] };
  return {
    workers: (data as any[]).map((w: any) => ({
      name: w.id, modified: w.modified_on, created: w.created_on,
      routes: w.routes?.map((r: any) => r.pattern) || [],
    })),
  };
}

// ── get_worker_errors ──
async function getWorkerErrors(db: Env["DB"], orgId: string, input: { worker: string; minutes?: number }): Promise<any> {
  const creds = await getCFCreds(db, orgId);
  if (!creds) return { error: "Cloudflare not connected" };

  const minutes = input.minutes || 60;
  const since = new Date(Date.now() - minutes * 60000).toISOString();

  // Use GraphQL analytics API for worker invocation errors
  const query = `query {
    viewer {
      accounts(filter: {accountTag: "${creds.accountId}"}) {
        workersInvocationsAdaptive(
          filter: {scriptName: "${input.worker}", datetime_gt: "${since}"}
          limit: 100
          orderBy: [datetime_DESC]
        ) {
          sum { errors requests subrequests }
          dimensions { datetime status scriptName }
        }
      }
    }
  }`;

  const res = await fetch("https://api.cloudflare.com/client/v4/graphql", {
    method: "POST",
    headers: { Authorization: `Bearer ${creds.token}`, "Content-Type": "application/json" },
    body: JSON.stringify({ query }),
    signal: AbortSignal.timeout(10000),
  });

  if (!res.ok) {
    // Fallback: just list recent deployments
    const deployments = await cfGet(creds.token,
      `https://api.cloudflare.com/client/v4/accounts/${creds.accountId}/workers/scripts/${input.worker}/deployments`
    );
    return {
      worker: input.worker,
      analytics: "GraphQL analytics unavailable",
      recent_deployments: deployments?.items?.slice(0, 5)?.map((d: any) => ({
        id: d.id, created: d.created_on, version: d.versions?.[0]?.version_id,
      })) || [],
    };
  }

  const data = await res.json<any>();
  const invocations = data?.data?.viewer?.accounts?.[0]?.workersInvocationsAdaptive || [];

  let totalRequests = 0;
  let totalErrors = 0;
  for (const inv of invocations) {
    totalRequests += inv.sum?.requests || 0;
    totalErrors += inv.sum?.errors || 0;
  }

  return {
    worker: input.worker,
    period_minutes: minutes,
    total_requests: totalRequests,
    total_errors: totalErrors,
    error_rate: totalRequests > 0 ? Math.round(totalErrors / totalRequests * 10000) / 100 : 0,
    data_points: invocations.length,
  };
}

// ── list_d1_databases ──
async function listD1(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const creds = await getCFCreds(db, orgId);
  if (!creds) return { error: "Cloudflare not connected" };
  const data = await cfGet(creds.token, `https://api.cloudflare.com/client/v4/accounts/${creds.accountId}/d1/database`);
  if (!data) return { databases: [] };
  return {
    databases: (data as any[]).map((d: any) => ({
      name: d.name, id: d.uuid, version: d.version,
      file_size: d.file_size, num_tables: d.num_tables,
      created: d.created_at,
    })),
  };
}

// ── list_kv_namespaces ──
async function listKV(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const creds = await getCFCreds(db, orgId);
  if (!creds) return { error: "Cloudflare not connected" };
  const data = await cfGet(creds.token, `https://api.cloudflare.com/client/v4/accounts/${creds.accountId}/storage/kv/namespaces`);
  if (!data) return { namespaces: [] };
  return {
    namespaces: (data as any[]).map((ns: any) => ({
      title: ns.title, id: ns.id,
    })),
  };
}

// ── list_tunnels ──
async function listTunnels(db: Env["DB"], orgId: string, input: {}): Promise<any> {
  const creds = await getCFCreds(db, orgId);
  if (!creds) return { error: "Cloudflare not connected" };
  const data = await cfGet(creds.token, `https://api.cloudflare.com/client/v4/accounts/${creds.accountId}/cfd_tunnel`);
  if (!data) return { tunnels: [] };
  return {
    tunnels: (data as any[]).map((t: any) => ({
      name: t.name, id: t.id, status: t.status,
      created: t.created_at,
      connections: t.connections?.map((c: any) => ({
        colo: c.colo_name, is_pending: c.is_pending_reconnect,
        opened: c.opened_at, origin_ip: c.origin_ip,
      })) || [],
    })),
  };
}

// ── get_dns_records ──
async function getDNSRecords(db: Env["DB"], orgId: string, input: { zone: string }): Promise<any> {
  const creds = await getCFCreds(db, orgId);
  if (!creds) return { error: "Cloudflare not connected" };

  // Find zone ID
  const zones = await cfGet(creds.token, `https://api.cloudflare.com/client/v4/zones?name=${input.zone}`);
  if (!zones || !Array.isArray(zones) || zones.length === 0) return { error: "Zone not found" };
  const zoneId = zones[0].id;

  const records = await cfGet(creds.token, `https://api.cloudflare.com/client/v4/zones/${zoneId}/dns_records?per_page=50`);
  if (!records) return { records: [] };
  return {
    zone: input.zone,
    records: (records as any[]).map((r: any) => ({
      name: r.name, type: r.type, content: r.content, proxied: r.proxied, ttl: r.ttl,
    })),
  };
}

// ── Dispatcher ──
export const CLOUDFLARE_TOOL_NAMES = [
  "list_cf_workers", "get_cf_worker_errors", "list_cf_d1",
  "list_cf_kv", "list_cf_tunnels", "get_cf_dns",
];

export async function executeCloudfareTool(
  db: Env["DB"], orgId: string, tool: string, input: Record<string, unknown>
): Promise<any> {
  switch (tool) {
    case "list_cf_workers": return listWorkers(db, orgId, input as any);
    case "get_cf_worker_errors": return getWorkerErrors(db, orgId, input as any);
    case "list_cf_d1": return listD1(db, orgId, input as any);
    case "list_cf_kv": return listKV(db, orgId, input as any);
    case "list_cf_tunnels": return listTunnels(db, orgId, input as any);
    case "get_cf_dns": return getDNSRecords(db, orgId, input as any);
    default: return { error: `Unknown cloudflare tool: ${tool}` };
  }
}
