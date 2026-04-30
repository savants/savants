/**
 * Live docs search - fetches content on-demand from provider sites.
 *
 * Architecture:
 *   1. llms.txt index tells us WHICH pages exist (stored in R2 or fetched)
 *   2. When user searches, we find matching page URLs from the index
 *   3. Fetch the actual .md page from the provider (cached in KV for 24h)
 *   4. Search within the page content
 *   5. Return the relevant section
 *
 * No docs stored in R2. Content lives at the source. Always fresh.
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const docsSearch = new Hono<HonoEnv>();

interface PageEntry {
  title: string;
  url: string;
  description: string;
}

// Provider configs: where to find their docs
const PROVIDERS: Record<string, { name: string; llms_txt: string; base_url: string }> = {
  stripe: { name: "Stripe", llms_txt: "https://docs.stripe.com/llms.txt", base_url: "https://docs.stripe.com" },
  react: { name: "React", llms_txt: "https://react.dev/llms.txt", base_url: "https://react.dev" },
  nextjs: { name: "Next.js", llms_txt: "https://nextjs.org/llms.txt", base_url: "https://nextjs.org" },
  fastify: { name: "Fastify", llms_txt: "https://fastify.dev/llms.txt", base_url: "https://fastify.dev" },
  cloudflare: { name: "Cloudflare", llms_txt: "https://developers.cloudflare.com/llms.txt", base_url: "https://developers.cloudflare.com" },
  docker: { name: "Docker", llms_txt: "https://docs.docker.com/llms.txt", base_url: "https://docs.docker.com" },
};

// GET /api/v1/docs/search/:provider - Live search
docsSearch.get("/search/:provider", async (c) => {
  const provider = c.req.param("provider");
  const query = c.req.query("q") || "";
  const limit = Math.min(parseInt(c.req.query("limit") || "5"), 20);

  if (!query) {
    return c.json({ error: "q parameter required", status: 400 }, 400);
  }

  const config = PROVIDERS[provider];
  if (!config) {
    return c.json({ error: "unknown_provider", providers: Object.keys(PROVIDERS), status: 404 }, 404);
  }

  // Step 1: Get the page index (cached in KV for 24h)
  const indexCacheKey = `docs_index:${provider}`;
  let pages: PageEntry[];

  const cached = await c.env.KV.get(indexCacheKey);
  if (cached) {
    pages = JSON.parse(cached);
  } else {
    pages = await fetchAndParseIndex(config.llms_txt);
    await c.env.KV.put(indexCacheKey, JSON.stringify(pages), { expirationTtl: 86400 });
  }

  // Step 2: Find pages that likely match the query (keyword match on title + description)
  const queryWords = query.toLowerCase().split(/\s+/);
  const candidates = pages
    .map((page) => {
      const text = `${page.title} ${page.description}`.toLowerCase();
      const score = queryWords.filter((w) => text.includes(w)).length / queryWords.length;
      return { page, score };
    })
    .filter((c) => c.score > 0)
    .sort((a, b) => b.score - a.score)
    .slice(0, Math.min(5, limit));

  if (candidates.length === 0) {
    return c.json({ query, provider, results: [], total: 0, message: "No matching pages found in index." });
  }

  // Step 3: Fetch the top candidate pages and search within content
  const results: Array<{
    title: string;
    url: string;
    section: string;
    content: string;
    score: number;
  }> = [];

  for (const { page } of candidates) {
    const content = await fetchPage(c.env.KV, page.url);
    if (!content) continue;

    // Find the most relevant section
    const sections = splitSections(content);
    const queryLower = query.toLowerCase();

    for (const section of sections) {
      const sectionText = `${section.heading} ${section.content}`.toLowerCase();
      const matchCount = queryWords.filter((w) => sectionText.includes(w)).length;

      if (matchCount >= Math.ceil(queryWords.length * 0.5)) {
        results.push({
          title: page.title,
          url: page.url.replace(".md", ""),
          section: section.heading,
          content: section.content.slice(0, 500),
          score: matchCount / queryWords.length,
        });
      }
    }
  }

  // Sort by relevance and limit
  results.sort((a, b) => b.score - a.score);
  const topResults = results.slice(0, limit);

  return c.json({
    query,
    provider: config.name,
    results: topResults,
    total: topResults.length,
    source: config.base_url,
  });
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

async function fetchAndParseIndex(llmsTxtUrl: string): Promise<PageEntry[]> {
  const res = await fetch(llmsTxtUrl, {
    headers: { "User-Agent": "Savants-Docs/1.0" },
    signal: AbortSignal.timeout(15000),
  });

  if (!res.ok) return [];
  const text = await res.text();

  const pages: PageEntry[] = [];
  const lines = text.split("\n");

  for (const line of lines) {
    // Match: - [Title](url): description
    const match = line.match(/^-\s+\[([^\]]+)\]\(([^)]+)\)(?::\s*(.+))?/);
    if (match) {
      pages.push({
        title: match[1],
        url: match[2],
        description: match[3] || "",
      });
    }
  }

  return pages;
}

async function fetchPage(kv: KVNamespace, url: string): Promise<string | null> {
  // Cache in KV for 24 hours
  const cacheKey = `docs_page:${url}`;
  const cached = await kv.get(cacheKey);
  if (cached) return cached;

  try {
    const res = await fetch(url, {
      headers: { "User-Agent": "Savants-Docs/1.0" },
      signal: AbortSignal.timeout(10000),
    });

    if (!res.ok) return null;
    const content = await res.text();

    // Cache for 24 hours
    await kv.put(cacheKey, content, { expirationTtl: 86400 });
    return content;
  } catch {
    return null;
  }
}

function splitSections(markdown: string): Array<{ heading: string; content: string }> {
  const sections: Array<{ heading: string; content: string }> = [];
  const lines = markdown.split("\n");
  let currentHeading = "Overview";
  let currentContent: string[] = [];

  for (const line of lines) {
    const headingMatch = line.match(/^#{1,3}\s+(.+)/);
    if (headingMatch) {
      if (currentContent.length > 0) {
        const text = currentContent.join("\n").trim();
        if (text.length > 10) {
          sections.push({ heading: currentHeading, content: text });
        }
      }
      currentHeading = headingMatch[1].trim();
      currentContent = [];
    } else {
      currentContent.push(line);
    }
  }

  if (currentContent.length > 0) {
    const text = currentContent.join("\n").trim();
    if (text.length > 10) {
      sections.push({ heading: currentHeading, content: text });
    }
  }

  return sections;
}

export default docsSearch;
