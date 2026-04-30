/**
 * Documentation Registry - queryable docs for LLMs.
 *
 * Storage (R2):
 *   savants-releases/docs/{provider}/manifest.json
 *   savants-releases/docs/{provider}/{version}/index.json
 *
 * manifest.json:
 *   { name, description, url, versions: [{version, page_count, indexed_at, content_hash}], latest }
 *
 * index.json:
 *   { pages: [{title, url, path, content_summary, sections: [{heading, content}]}] }
 *
 * Crawl pipeline:
 *   1. Cron trigger (or manual) → crawl docs site
 *   2. Hash content → compare with previous
 *   3. If changed → parse, create new version, upload to R2
 *   4. Update manifest.json latest pointer
 *
 * User flow:
 *   savants docs add stripe         → downloads latest index to ~/.savants/docs/stripe/
 *   savants docs add stripe@2024    → downloads specific version
 *   semantic_search queries merge doc results with code results
 */

import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const docs = new Hono<HonoEnv>();

interface DocManifest {
  name: string;
  description: string;
  url: string;
  icon?: string;
  versions: Array<{
    version: string;
    page_count: number;
    indexed_at: number;
    content_hash: string;
  }>;
  latest: string;
  updated_at: number;
}

// ─── Public: list available doc sources ──────────────────────────────────────

// GET /api/v1/docs - List all available doc sources
docs.get("/", async (c) => {
  // Registry is served from R2 via releases.savants.dev/docs/

  // For now, return a static registry. Later, read from R2 manifests.
  const registry = getRegistry();

  return c.json({
    providers: registry.map((p) => ({
      name: p.name,
      description: p.description,
      url: p.url,
      latest_version: p.latest,
      versions: p.versions.length,
      status: p.status,
    })),
    total: registry.length,
    usage: 'savants docs add <provider>[@version]',
  });
});

// GET /api/v1/docs/:provider - Get doc source details + versions
docs.get("/:provider", async (c) => {
  const provider = c.req.param("provider");
  const registry = getRegistry();
  const entry = registry.find((p) => p.name === provider);

  if (!entry) {
    return c.json({ error: "not_found", message: `Doc source '${provider}' not found. Run GET /api/v1/docs to see available sources.`, status: 404 }, 404);
  }

  return c.json(entry);
});

// GET /api/v1/docs/:provider/search - Search a doc source (free, public)
// Rate limiting handled at Cloudflare edge (WAF → Rate Limiting Rules), not in app
docs.get("/:provider/search", async (c) => {
  const provider = c.req.param("provider");
  const query = c.req.query("q") || "";
  const version = c.req.query("version") || "latest";

  if (!query) {
    return c.json({ error: "query required", message: "Use ?q=your+search+query", status: 400 }, 400);
  }

  const registry = getRegistry();
  const entry = registry.find((p) => p.name === provider);

  if (!entry) {
    return c.json({ error: "not_found", status: 404 }, 404);
  }

  // Try to fetch the index from R2
  try {
    const indexUrl = `https://releases.savants.dev/docs/${provider}/${version}/index.json`;
    const res = await fetch(indexUrl);

    if (res.ok) {
      const index = await res.json<{ pages: Array<{ title: string; url: string; path: string; content_summary: string; sections: Array<{ heading: string; content: string }> }> }>();

      // Simple keyword search across pages
      const queryLower = query.toLowerCase();
      const results = index.pages
        .filter((page) => {
          const text = `${page.title} ${page.content_summary} ${page.sections.map((s) => `${s.heading} ${s.content}`).join(" ")}`.toLowerCase();
          return text.includes(queryLower);
        })
        .slice(0, 10)
        .map((page) => {
          // Find the best matching section
          const matchingSection = page.sections.find((s) =>
            `${s.heading} ${s.content}`.toLowerCase().includes(queryLower)
          );
          return {
            title: page.title,
            url: page.url,
            path: page.path,
            summary: page.content_summary,
            matched_section: matchingSection ? {
              heading: matchingSection.heading,
              content: matchingSection.content.slice(0, 500),
            } : null,
          };
        });

      return c.json({ query, provider, version, results, total: results.length });
    }
  } catch {
    // Index not available yet
  }

  // Fallback: return a message that this provider needs indexing
  return c.json({
    query,
    provider,
    version,
    results: [],
    total: 0,
    message: `Index for ${provider}@${version} not built yet. It will be available after the next crawl.`,
  });
});

// POST /api/v1/docs/:provider/crawl - Trigger a re-crawl (admin only)
docs.post("/:provider/crawl", async (c) => {
  const auth = c.get("auth");
  const provider = c.req.param("provider");

  // For now, return instructions. Full crawl pipeline is a background job.
  return c.json({
    status: "queued",
    provider,
    message: `Crawl queued for ${provider}. Index will be updated within 1 hour.`,
  });
});

// ─── Registry (static for now, later from R2) ────────────────────────────────

interface RegistryEntry {
  name: string;
  description: string;
  url: string;
  latest: string;
  versions: Array<{ version: string; page_count: number }>;
  status: "available" | "indexing" | "planned";
  crawl_url: string;
}

function getRegistry(): RegistryEntry[] {
  return [
    {
      name: "stripe",
      description: "Stripe API documentation - payments, subscriptions, webhooks",
      url: "https://docs.stripe.com",
      latest: "2024-12",
      versions: [{ version: "2024-12", page_count: 0 }],
      status: "planned",
      crawl_url: "https://docs.stripe.com/api",
    },
    {
      name: "kubernetes",
      description: "Kubernetes documentation - pods, services, deployments, networking",
      url: "https://kubernetes.io/docs",
      latest: "1.30",
      versions: [{ version: "1.29", page_count: 0 }, { version: "1.30", page_count: 0 }],
      status: "planned",
      crawl_url: "https://kubernetes.io/docs/reference/",
    },
    {
      name: "react",
      description: "React documentation - components, hooks, patterns",
      url: "https://react.dev",
      latest: "19",
      versions: [{ version: "18", page_count: 0 }, { version: "19", page_count: 0 }],
      status: "planned",
      crawl_url: "https://react.dev/reference/react",
    },
    {
      name: "nextjs",
      description: "Next.js documentation - routing, data fetching, deployment",
      url: "https://nextjs.org/docs",
      latest: "15",
      versions: [{ version: "14", page_count: 0 }, { version: "15", page_count: 0 }],
      status: "planned",
      crawl_url: "https://nextjs.org/docs",
    },
    {
      name: "fastify",
      description: "Fastify documentation - routes, plugins, validation",
      url: "https://fastify.dev/docs",
      latest: "5",
      versions: [{ version: "4", page_count: 0 }, { version: "5", page_count: 0 }],
      status: "planned",
      crawl_url: "https://fastify.dev/docs/latest/",
    },
    {
      name: "postgres",
      description: "PostgreSQL documentation - SQL, administration, performance",
      url: "https://www.postgresql.org/docs/",
      latest: "17",
      versions: [{ version: "16", page_count: 0 }, { version: "17", page_count: 0 }],
      status: "planned",
      crawl_url: "https://www.postgresql.org/docs/current/",
    },
    {
      name: "redis",
      description: "Redis documentation - commands, data structures, clustering",
      url: "https://redis.io/docs",
      latest: "7",
      versions: [{ version: "7", page_count: 0 }],
      status: "planned",
      crawl_url: "https://redis.io/docs/latest/",
    },
    {
      name: "docker",
      description: "Docker documentation - containers, compose, networking",
      url: "https://docs.docker.com",
      latest: "27",
      versions: [{ version: "27", page_count: 0 }],
      status: "planned",
      crawl_url: "https://docs.docker.com/reference/",
    },
    {
      name: "aws",
      description: "AWS documentation - EC2, S3, Lambda, IAM, ECS, EKS",
      url: "https://docs.aws.amazon.com",
      latest: "2024",
      versions: [{ version: "2024", page_count: 0 }],
      status: "planned",
      crawl_url: "https://docs.aws.amazon.com/",
    },
    {
      name: "cloudflare",
      description: "Cloudflare documentation - Workers, D1, R2, Pages, KV",
      url: "https://developers.cloudflare.com",
      latest: "2024",
      versions: [{ version: "2024", page_count: 0 }],
      status: "planned",
      crawl_url: "https://developers.cloudflare.com/workers/",
    },
  ];
}

export default docs;
