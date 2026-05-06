/**
 * Documentation indexer.
 *
 * Fetches llms.txt / llms-full.txt from doc sites,
 * parses into sections, stores in R2 as a searchable index.
 *
 * Most major doc sites now publish llms.txt:
 *   developers.cloudflare.com/llms.txt  ✓
 *   docs.stripe.com/llms.txt            ✓
 *   react.dev/llms.txt                  ✓
 *   nextjs.org/llms.txt                 ✓
 *   fastify.dev/llms.txt                ✓
 *
 * We don't need to crawl HTML - providers already give us
 * LLM-optimized content. We just index and serve it.
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const indexer = new Hono<HonoEnv>();

interface DocSource {
  name: string;
  llms_txt_url: string;
  llms_full_url?: string;
  description: string;
}

const SOURCES: DocSource[] = [
  { name: "cloudflare", llms_txt_url: "https://developers.cloudflare.com/llms.txt", llms_full_url: "https://developers.cloudflare.com/d1/llms-full.txt", description: "Cloudflare Developer Docs" },
  { name: "stripe", llms_txt_url: "https://docs.stripe.com/llms.txt", description: "Stripe API Documentation" },
  { name: "react", llms_txt_url: "https://react.dev/llms.txt", description: "React Documentation" },
  { name: "nextjs", llms_txt_url: "https://nextjs.org/llms.txt", description: "Next.js Documentation" },
  { name: "fastify", llms_txt_url: "https://fastify.dev/llms.txt", llms_full_url: "https://fastify.dev/llms-full.txt", description: "Fastify Documentation" },
  { name: "mongodb", llms_txt_url: "https://www.mongodb.com/docs/llms.txt", llms_full_url: "https://www.mongodb.com/docs/llms-full.txt", description: "MongoDB & Atlas Documentation" },
];

// POST /api/v1/docs/index/:provider - Index a doc source from its llms.txt
indexer.post("/index/:provider", async (c) => {
  const provider = c.req.param("provider");
  const source = SOURCES.find((s) => s.name === provider);

  if (!source) {
    return c.json({ error: "unknown_provider", message: `Known providers: ${SOURCES.map(s => s.name).join(", ")}`, status: 404 }, 404);
  }

  const startTime = Date.now();

  // Fetch llms.txt
  let content: string;
  try {
    const res = await fetch(source.llms_txt_url, {
      headers: { "User-Agent": "Savants-Doc-Indexer/1.0" },
      signal: AbortSignal.timeout(30000),
    });
    if (!res.ok) {
      return c.json({ error: "fetch_failed", message: `${source.llms_txt_url} returned ${res.status}`, status: 502 }, 502);
    }
    content = await res.text();
  } catch (err) {
    return c.json({ error: "fetch_error", message: String(err), status: 502 }, 502);
  }

  // Parse into sections
  const sections = parseLlmsTxt(content);
  const contentHash = await hashContent(content);

  // Build the index
  const index = {
    provider: source.name,
    description: source.description,
    source_url: source.llms_txt_url,
    indexed_at: Math.floor(Date.now() / 1000),
    content_hash: contentHash,
    total_sections: sections.length,
    total_chars: content.length,
    estimated_tokens: Math.ceil(content.length / 4),
    pages: sections,
  };

  // Upload to R2
  const r2Key = `docs/${source.name}/latest/index.json`;
  const r2Api = `https://api.cloudflare.com/client/v4/accounts/4992fd600f9894326a82a0f8573a7c38/r2/buckets/savants-releases/objects/${r2Key}`;

  try {
    const uploadRes = await fetch(r2Api, {
      method: "PUT",
      headers: {
        "Authorization": `Bearer ${c.env.CF_API_TOKEN || ""}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify(index),
    });

    if (!uploadRes.ok) {
      // R2 upload failed, but we still have the index in memory
      console.error("R2 upload failed:", await uploadRes.text());
    }
  } catch {
    console.error("R2 upload error");
  }

  const durationMs = Date.now() - startTime;

  return c.json({
    provider: source.name,
    sections: sections.length,
    chars: content.length,
    estimated_tokens: Math.ceil(content.length / 4),
    content_hash: contentHash,
    duration_ms: durationMs,
    r2_key: r2Key,
    message: `Indexed ${sections.length} sections from ${source.name} llms.txt`,
  });
});

// POST /api/v1/docs/index-all - Index all known doc sources
indexer.post("/index-all", async (c) => {
  const results: Array<{ provider: string; sections: number; status: string }> = [];

  for (const source of SOURCES) {
    try {
      const res = await fetch(source.llms_txt_url, {
        headers: { "User-Agent": "Savants-Doc-Indexer/1.0" },
        signal: AbortSignal.timeout(30000),
      });

      if (!res.ok) {
        results.push({ provider: source.name, sections: 0, status: `fetch failed: ${res.status}` });
        continue;
      }

      const content = await res.text();
      const sections = parseLlmsTxt(content);
      const contentHash = await hashContent(content);

      const index = {
        provider: source.name,
        description: source.description,
        source_url: source.llms_txt_url,
        indexed_at: Math.floor(Date.now() / 1000),
        content_hash: contentHash,
        total_sections: sections.length,
        pages: sections,
      };

      // Upload to R2
      const r2Key = `docs/${source.name}/latest/index.json`;
      const r2Api = `https://api.cloudflare.com/client/v4/accounts/4992fd600f9894326a82a0f8573a7c38/r2/buckets/savants-releases/objects/${r2Key}`;

      await fetch(r2Api, {
        method: "PUT",
        headers: {
          "Authorization": `Bearer ${c.env.CF_API_TOKEN || ""}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify(index),
      }).catch(() => {});

      results.push({ provider: source.name, sections: sections.length, status: "indexed" });
    } catch (err) {
      results.push({ provider: source.name, sections: 0, status: `error: ${err}` });
    }
  }

  return c.json({ results, total_indexed: results.filter(r => r.status === "indexed").length });
});

// ─── Parser ──────────────────────────────────────────────────────────────────

function parseLlmsTxt(content: string): Array<{ title: string; url: string; path: string; content_summary: string; sections: Array<{ heading: string; content: string }> }> {
  const pages: Array<{ title: string; url: string; path: string; content_summary: string; sections: Array<{ heading: string; content: string }> }> = [];

  // llms.txt format is markdown with links
  // Split into logical sections based on headings
  const lines = content.split("\n");
  let currentPage: { title: string; url: string; path: string; content_summary: string; sections: Array<{ heading: string; content: string }> } | null = null;
  let currentSection: { heading: string; content: string[] } | null = null;

  for (const line of lines) {
    // Heading: ## Section or # Title
    const h1Match = line.match(/^#\s+(.+)/);
    const h2Match = line.match(/^##\s+(.+)/);
    const h3Match = line.match(/^###\s+(.+)/);

    if (h1Match || h2Match) {
      // Save previous page
      if (currentPage) {
        if (currentSection) {
          currentPage.sections.push({ heading: currentSection.heading, content: currentSection.content.join("\n").trim() });
        }
        if (currentPage.sections.length > 0 || currentPage.content_summary) {
          pages.push(currentPage);
        }
      }

      const title = (h1Match || h2Match)![1].trim();
      currentPage = { title, url: "", path: "", content_summary: "", sections: [] };
      currentSection = null;
      continue;
    }

    if (h3Match && currentPage) {
      if (currentSection) {
        currentPage.sections.push({ heading: currentSection.heading, content: currentSection.content.join("\n").trim() });
      }
      currentSection = { heading: h3Match[1].trim(), content: [] };
      continue;
    }

    // Link: - [Title](url): description
    const linkMatch = line.match(/^-\s+\[([^\]]+)\]\(([^)]+)\)(?::\s*(.+))?/);
    if (linkMatch && currentPage) {
      const [, title, url, desc] = linkMatch;
      currentPage.sections.push({
        heading: title,
        content: `${desc || ""}\nURL: ${url}`.trim(),
      });
      continue;
    }

    // Regular content
    if (currentSection) {
      currentSection.content.push(line);
    } else if (currentPage && line.trim()) {
      currentPage.content_summary += (currentPage.content_summary ? " " : "") + line.trim();
    }
  }

  // Save last page
  if (currentPage) {
    if (currentSection) {
      currentPage.sections.push({ heading: currentSection.heading, content: currentSection.content.join("\n").trim() });
    }
    if (currentPage.sections.length > 0 || currentPage.content_summary) {
      pages.push(currentPage);
    }
  }

  return pages;
}

async function hashContent(content: string): Promise<string> {
  const encoder = new TextEncoder();
  const data = encoder.encode(content);
  const hash = await crypto.subtle.digest("SHA-256", data);
  return Array.from(new Uint8Array(hash)).map(b => b.toString(16).padStart(2, "0")).join("").slice(0, 16);
}

export default indexer;
