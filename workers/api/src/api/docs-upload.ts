/**
 * Private docs upload + indexing.
 *
 * Users upload their own docs (markdown, OpenAPI, raw text).
 * Parsed into sections, stored as graph nodes in their project.
 * Queryable via semantic_search alongside code and certified docs.
 *
 * Pricing:
 *   Upload/index: 3 credits per 100 pages
 *   Re-index: 2 credits per 100 pages (changed only)
 *   Query: 0 credits (free - it's a DB read)
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { deductCredits } from "./credits";
import { audit, requestMeta } from "../lib/audit";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const docsUpload = new Hono<HonoEnv>();

interface DocPage {
  title: string;
  path: string;
  content: string;
  format?: "markdown" | "openapi" | "text" | "html";
  url?: string;
}

// POST /api/v1/docs/upload - Upload private docs to a project
docsUpload.post("/upload", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{
    project_id: string;
    name: string;
    description?: string;
    pages: DocPage[];
    format?: string;
    replace?: boolean;
  }>();

  if (!body.project_id || !body.name || !body.pages?.length) {
    return c.json({ error: "project_id, name, and pages[] are required", status: 400 }, 400);
  }

  // Verify project access
  const project = await c.env.DB
    .prepare("SELECT id FROM projects WHERE id = ?1 AND org_id = ?2")
    .bind(body.project_id, auth.orgId)
    .first();

  if (!project) {
    return c.json({ error: "project_not_found", status: 404 }, 404);
  }

  // Calculate credits cost based on total tokens (1 credit per 10K tokens, min 1)
  const totalChars = body.pages.reduce((sum, p) => sum + (p.content?.length || 0), 0);
  const estimatedTokens = Math.ceil(totalChars / 4); // ~4 chars per token
  const creditCost = Math.max(1, Math.ceil(estimatedTokens / 10000));
  const creditResult = await deductCredits(c.env.DB, auth.orgId, "doc_upload");

  // Custom deduction for variable cost
  if (creditCost > 0) {
    const balance = await c.env.DB
      .prepare("SELECT balance FROM credit_balances WHERE org_id = ?1")
      .bind(auth.orgId)
      .first<{ balance: number }>();

    const currentBalance = balance?.balance ?? 0;
    const isEnterprise = false; // TODO: check org plan

    if (currentBalance < creditCost && !isEnterprise) {
      return c.json({
        error: "insufficient_credits",
        message: `Need ${creditCost} credits for ~${estimatedTokens.toLocaleString()} tokens (${body.pages.length} docs). Have ${currentBalance}.`,
        credits: { cost: creditCost, balance: currentBalance, tokens: estimatedTokens, docs: body.pages.length },
        status: 402,
      }, 402);
    }

    // Deduct
    await c.env.DB
      .prepare("UPDATE credit_balances SET balance = balance - ?1, updated_at = ?2 WHERE org_id = ?3")
      .bind(creditCost, Math.floor(Date.now() / 1000), auth.orgId)
      .run();

    await c.env.DB
      .prepare("INSERT INTO credit_transactions (id, org_id, type, amount, balance_after, description, project_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
      .bind(
        crypto.randomUUID(), auth.orgId, "usage", -creditCost,
        currentBalance - creditCost,
        `Doc upload: ${body.name} (${body.pages.length} pages)`,
        body.project_id,
        Math.floor(Date.now() / 1000)
      )
      .run();
  }

  const sourceId = crypto.randomUUID();
  const now = Math.floor(Date.now() / 1000);

  // If replace, delete existing doc source with same name
  if (body.replace) {
    // Get existing source ID
    const existing = await c.env.DB
      .prepare("SELECT id FROM project_sources WHERE project_id = ?1 AND source_type = 'docs_private' AND source_config LIKE ?2")
      .bind(body.project_id, `%"name":"${body.name}"%`)
      .first<{ id: string }>();

    if (existing) {
      await c.env.DB
        .prepare("DELETE FROM graph_nodes WHERE project_id = ?1 AND source_type = 'docs_private' AND source_id = ?2")
        .bind(body.project_id, existing.id)
        .run();
      await c.env.DB
        .prepare("DELETE FROM project_sources WHERE id = ?1")
        .bind(existing.id)
        .run();
    }
  }

  // Create project source
  await c.env.DB
    .prepare("INSERT INTO project_sources (id, project_id, source_type, source_config, last_synced_at, node_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
    .bind(
      sourceId, body.project_id, "docs_private",
      JSON.stringify({ name: body.name, description: body.description || "", page_count: body.pages.length, format: body.format || "markdown" }),
      now, body.pages.length
    )
    .run();

  // Parse pages into graph nodes
  let nodeCount = 0;
  for (const page of body.pages) {
    const sections = parseSections(page.content, page.format || "markdown");

    // Create a node for the page itself
    const pageNodeId = crypto.randomUUID();
    await c.env.DB
      .prepare(`INSERT INTO graph_nodes (id, project_id, type, name, qualified_name, file_path, content_summary, metadata, source_type, source_id, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)`)
      .bind(
        pageNodeId, body.project_id, "doc_page",
        page.title, `${body.name}/${page.path}`,
        page.path, page.content.slice(0, 500),
        JSON.stringify({ url: page.url || null, format: page.format || "markdown", section_count: sections.length }),
        "docs_private", sourceId, now
      )
      .run();
    nodeCount++;

    // Create nodes for each section (for granular search)
    for (const section of sections) {
      await c.env.DB
        .prepare(`INSERT INTO graph_nodes (id, project_id, type, name, qualified_name, file_path, content_summary, metadata, source_type, source_id, updated_at)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)`)
        .bind(
          crypto.randomUUID(), body.project_id, "doc_section",
          section.heading, `${body.name}/${page.path}#${slugify(section.heading)}`,
          page.path, section.content.slice(0, 1000),
          JSON.stringify({ parent_page: pageNodeId, url: page.url || null }),
          "docs_private", sourceId, now
        )
        .run();
      nodeCount++;
    }
  }

  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId: auth.orgId, actorId: auth.userId,
    action: "docs.upload", resourceType: "docs", resourceId: sourceId,
    metadata: { name: body.name, pages: body.pages.length, nodes: nodeCount, credits: creditCost },
    ...meta,
  });

  return c.json({
    id: sourceId,
    name: body.name,
    pages_uploaded: body.pages.length,
    nodes_created: nodeCount,
    credits_charged: creditCost,
    message: `${body.pages.length} pages indexed. Queryable via semantic_search.`,
  });
});

// GET /api/v1/docs/private - List private doc sources for a project
docsUpload.get("/private", async (c) => {
  const auth = c.get("auth");
  const projectId = c.req.query("project_id");

  if (!projectId) {
    return c.json({ error: "project_id query param required", status: 400 }, 400);
  }

  const sources = await c.env.DB
    .prepare("SELECT * FROM project_sources WHERE project_id = ?1 AND source_type = 'docs_private' ORDER BY created_at DESC")
    .bind(projectId)
    .all();

  return c.json({
    docs: (sources.results as unknown as any[]).map((s) => ({
      id: s.id,
      config: JSON.parse(s.source_config || "{}"),
      node_count: s.node_count,
      last_synced_at: s.last_synced_at,
      created_at: s.created_at,
    })),
  });
});

// DELETE /api/v1/docs/private/:sourceId - Remove a private doc source
docsUpload.delete("/private/:sourceId", async (c) => {
  const auth = c.get("auth");
  const sourceId = c.req.param("sourceId");

  // Verify ownership via project
  const source = await c.env.DB
    .prepare(`SELECT ps.id, ps.project_id FROM project_sources ps
      JOIN projects p ON p.id = ps.project_id
      WHERE ps.id = ?1 AND p.org_id = ?2`)
    .bind(sourceId, auth.orgId)
    .first();

  if (!source) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  // Delete graph nodes for this source
  await c.env.DB
    .prepare("DELETE FROM graph_nodes WHERE source_type = 'docs_private' AND source_id = ?1")
    .bind(sourceId)
    .run();

  // Delete the source
  await c.env.DB
    .prepare("DELETE FROM project_sources WHERE id = ?1")
    .bind(sourceId)
    .run();

  return c.json({ deleted: true });
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

function parseSections(content: string, format: string): Array<{ heading: string; content: string }> {
  const sections: Array<{ heading: string; content: string }> = [];

  if (format === "openapi") {
    // Parse OpenAPI endpoints as sections
    try {
      const spec = JSON.parse(content);
      const paths = spec.paths || {};
      for (const [path, methods] of Object.entries(paths)) {
        for (const [method, detail] of Object.entries(methods as Record<string, any>)) {
          if (typeof detail === "object" && detail.summary) {
            sections.push({
              heading: `${method.toUpperCase()} ${path}`,
              content: `${detail.summary}\n${detail.description || ""}`,
            });
          }
        }
      }
    } catch {
      // Not valid JSON, treat as text
      sections.push({ heading: "API Spec", content: content.slice(0, 2000) });
    }
    return sections;
  }

  // Markdown: split on headings
  const lines = content.split("\n");
  let currentHeading = "Introduction";
  let currentContent: string[] = [];

  for (const line of lines) {
    const headingMatch = line.match(/^#{1,3}\s+(.+)/);
    if (headingMatch) {
      if (currentContent.length > 0) {
        sections.push({ heading: currentHeading, content: currentContent.join("\n").trim() });
      }
      currentHeading = headingMatch[1];
      currentContent = [];
    } else {
      currentContent.push(line);
    }
  }

  if (currentContent.length > 0) {
    sections.push({ heading: currentHeading, content: currentContent.join("\n").trim() });
  }

  return sections;
}

function slugify(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/(^-|-$)/g, "");
}

export default docsUpload;
