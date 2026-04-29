import { Hono } from "hono";
import type { Env, AuthContext } from "../lib/types";
import { bufToHex, hmacSign } from "../lib/crypto";
import { logUsageEvent, getIntegrationsByType } from "../db/queries";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const github = new Hono<HonoEnv>();

async function verifyGitHubSignature(
  payload: string,
  sigHeader: string,
  secret: string
): Promise<boolean> {
  if (!sigHeader.startsWith("sha256=")) return false;

  const receivedSig = sigHeader.slice(7);
  const expectedSig = bufToHex(await hmacSign(secret, payload));

  if (receivedSig.length !== expectedSig.length) return false;
  let result = 0;
  for (let i = 0; i < receivedSig.length; i++) {
    result |= receivedSig.charCodeAt(i) ^ expectedSig.charCodeAt(i);
  }
  return result === 0;
}

// POST /webhooks/github
github.post("/", async (c) => {
  const sigHeader = c.req.header("x-hub-signature-256") ?? "";
  const eventType = c.req.header("x-github-event") ?? "";
  const rawBody = await c.req.text();

  // Verify signature if GITHUB_APP_TOKEN is set (used as webhook secret)
  if (c.env.GITHUB_APP_TOKEN) {
    const valid = await verifyGitHubSignature(rawBody, sigHeader, c.env.GITHUB_APP_TOKEN);
    if (!valid) {
      return c.json({ error: "invalid_signature", message: "GitHub signature verification failed", status: 401 }, 401);
    }
  }

  const payload = JSON.parse(rawBody);

  if (eventType === "pull_request") {
    const action = payload.action as string;

    if (action === "opened" || action === "synchronize") {
      const pr = payload.pull_request as {
        number: number;
        title: string;
        body: string | null;
        diff_url: string;
        base: { ref: string; repo: { full_name: string } };
        head: { ref: string; sha: string };
      };

      const repo = payload.repository as { full_name: string; id: number };

      // Fetch the diff
      let diff = "";
      try {
        const diffRes = await fetch(pr.diff_url, {
          headers: {
            Accept: "application/vnd.github.v3.diff",
            "User-Agent": "Savants-Cloud-API",
          },
        });
        if (diffRes.ok) {
          diff = await diffRes.text();
          // Truncate large diffs to 100KB to stay within reasonable limits
          if (diff.length > 102400) {
            diff = diff.substring(0, 102400) + "\n... (truncated)";
          }
        }
      } catch {
        // If we cannot fetch the diff, proceed with empty diff
      }

      // Proxy pr-risk analysis to astra
      try {
        const proxyRes = await fetch(`${c.env.GRAPH_PROXY_URL}/api/v1/tools/call`, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "X-Webhook-Source": "github",
            "X-Repo": repo.full_name,
          },
          body: JSON.stringify({
            tool: "pr_risk",
            input: {
              diff,
              base_branch: pr.base.ref,
              pr_number: pr.number,
              pr_title: pr.title,
              repo: repo.full_name,
              head_sha: pr.head.sha,
            },
          }),
        });

        if (proxyRes.ok) {
          const analysis = await proxyRes.json<{ summary?: string; risk_score?: number; details?: string }>();

          // Post analysis as PR comment
          const commentBody = formatPrComment(analysis);

          await fetch(
            `https://api.github.com/repos/${repo.full_name}/issues/${pr.number}/comments`,
            {
              method: "POST",
              headers: {
                Authorization: `Bearer ${c.env.GITHUB_APP_TOKEN}`,
                "Content-Type": "application/json",
                "User-Agent": "Savants-Cloud-API",
              },
              body: JSON.stringify({ body: commentBody }),
            }
          );
        }
      } catch {
        // Non-fatal: if proxy fails, we just do not comment
      }
    }
  }

  // ── Push events: auto-index changed files ──
  if (eventType === "push") {
    const repo = payload.repository as { full_name: string; id: number; private: boolean };
    const ref = payload.ref as string; // refs/heads/main
    const branch = ref.replace("refs/heads/", "");
    const commits = payload.commits as Array<{
      id: string;
      message: string;
      added: string[];
      modified: string[];
      removed: string[];
    }>;

    if (commits && commits.length > 0) {
      // Collect all changed files
      const changedFiles = new Set<string>();
      for (const commit of commits) {
        for (const f of [...(commit.added || []), ...(commit.modified || [])]) {
          // Only index code files
          if (f.match(/\.(ts|js|tsx|jsx|py|rs|go|java|rb|php|swift|kt|c|cpp|h|hpp|cs)$/)) {
            changedFiles.add(f);
          }
        }
      }

      if (changedFiles.size > 0) {
        // Store the index event in D1
        // In the future, fetch + parse these files and store in the code graph
        // For now, log the event for billing and track what changed

        // Find which org owns this repo (via GitHub integrations)
        const integrations = await getIntegrationsByType(c.env.DB, "github");
        let orgId: string | null = null;
        for (const integ of integrations) {
          try {
            const config = JSON.parse(integ.config);
            if (config.repos?.includes(repo.full_name) || config.org === repo.full_name.split("/")[0]) {
              orgId = integ.org_id;
              break;
            }
          } catch { /* skip */ }
        }

        if (orgId) {
          // Log as a reindex-diff usage event ($0.25)
          await logUsageEvent(c.env.DB, {
            id: crypto.randomUUID(),
            orgId,
            userId: null,
            toolName: "reindex_diff",
            graphScopeId: null,
            tokensIn: 0,
            tokensOut: changedFiles.size,
            durationMs: 0,
          });
        }

        return c.json({
          received: true,
          event: eventType,
          repo: repo.full_name,
          branch,
          files_changed: changedFiles.size,
          indexed: !!orgId,
        });
      }
    }
  }

  return c.json({ received: true, event: eventType });
});

function formatPrComment(analysis: { summary?: string; risk_score?: number; details?: string }): string {
  const score = analysis.risk_score ?? 0;
  const emoji = score >= 7 ? "!!!" : score >= 4 ? "!!" : "OK";
  const level = score >= 7 ? "High" : score >= 4 ? "Medium" : "Low";

  let comment = `## Savants PR Risk Analysis\n\n`;
  comment += `**Risk Level:** ${level} (${score}/10) ${emoji}\n\n`;

  if (analysis.summary) {
    comment += `### Summary\n${analysis.summary}\n\n`;
  }

  if (analysis.details) {
    comment += `### Details\n${analysis.details}\n\n`;
  }

  comment += `---\n*Automated by [Savants](https://savants.cloud)*`;
  return comment;
}

export default github;
