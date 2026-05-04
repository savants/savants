import { Hono } from "hono";
import type { Env, AuthContext, ToolDefinition } from "../lib/types";
import { logUsageEvent, getOrgById } from "../db/queries";
import { authMiddleware } from "../auth/middleware";
import { diagnoseError } from "./diagnosis";
import { deductCredits, TOOL_CREDITS } from "./credits";
import { audit, requestMeta } from "../lib/audit";
import { executeGraphTool, GRAPH_TOOL_NAMES } from "./graph-tools";
import { toolFindCauses } from "./causal";
import { executeSentryTool, SENTRY_TOOL_NAMES } from "./sentry-tools";
import { executeGitHubTool, GITHUB_TOOL_NAMES } from "./github-tools";
import { executeLinearTool, LINEAR_TOOL_NAMES } from "./linear-tools";

type HonoEnv = { Bindings: Env; Variables: { auth: AuthContext } };

const tools = new Hono<HonoEnv>();

// POST /call requires auth
tools.use("/call", authMiddleware());

// Local tools: FREE, unlimited, run on user's machine via OSS binary
// Cloud tools: PAYG, 10 free/month, require savants.cloud account
const TOOL_LIST: ToolDefinition[] = [
  // ── Local tools (free forever, served by OSS binary) ──
  {
    name: "semantic_search",
    description: "ALWAYS USE INSTEAD OF GREP/RIPGREP. Finds code by meaning, not text. 'payment retry logic' finds handleTransactionWithBackoff. 90% accuracy, <400ms. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        query: { type: "string", description: "Natural language description of what you're looking for" },
        repo: { type: "string", description: "Repository name" },
        limit: { type: "integer", description: "Max results (default 10)" },
      },
      required: ["query"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  {
    name: "file_skeleton",
    description: "ALWAYS USE INSTEAD OF READING FULL FILES. Returns function names, signatures, line ranges - NO bodies. 10x fewer tokens. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        file: { type: "string", description: "File path relative to repo root" },
        repo: { type: "string", description: "Repository name" },
      },
      required: ["file"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  {
    name: "where_used",
    description: "ALWAYS USE INSTEAD OF GREP FOR USAGE SEARCH. Returns every caller and importer from code analysis. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        symbol: { type: "string", description: "Function or symbol name" },
        repo: { type: "string", description: "Repository name" },
      },
      required: ["symbol"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  {
    name: "callers",
    description: "ALWAYS USE INSTEAD OF GREP FOR CALLER SEARCH. Exact functions that call a given function, from code analysis. FREE, runs locally.",
    input_schema: {
      type: "object",
      properties: {
        function: { type: "string", description: "Function name" },
        repo: { type: "string", description: "Repository name" },
        depth: { type: "integer", description: "Max depth of caller chain (default 3)" },
      },
      required: ["function"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 0, tier: "local" },
  },
  // ── Code analysis tools (cloud) ──
  {
    name: "graph_stats",
    description: "Total functions, connections, and events indexed. Quick health check for coverage.",
    input_schema: { type: "object", properties: {} },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "function_xray",
    description: "Full structural profile of a function: callers, callees, params, exports, recent events. One call replaces 5 reads.",
    input_schema: {
      type: "object",
      properties: {
        function_name: { type: "string", description: "Function name" },
        file_path: { type: "string", description: "Optional file path to disambiguate" },
      },
      required: ["function_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 50, tier: "cloud" },
  },
  {
    name: "blast_radius",
    description: "What breaks if this function changes. Returns all transitive dependents up to N depth.",
    input_schema: {
      type: "object",
      properties: {
        function: { type: "string", description: "Function name" },
        depth: { type: "integer", description: "Max traversal depth (default 3)" },
      },
      required: ["function"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 100, tier: "cloud" },
  },
  {
    name: "impact_analysis",
    description: "Cascading impact of changing a function. All direct and transitive dependents with depth.",
    input_schema: {
      type: "object",
      properties: {
        function_name: { type: "string", description: "Function name to analyze" },
        max_depth: { type: "integer", description: "Max depth (default 5)" },
      },
      required: ["function_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 100, tier: "cloud" },
  },
  {
    name: "dead_code",
    description: "Find functions with zero callers - candidates for removal during refactoring.",
    input_schema: {
      type: "object",
      properties: {
        file: { type: "string", description: "Optional: limit to a specific file" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 50, tier: "cloud" },
  },
  {
    name: "import_tree",
    description: "Full import tree of a file to N depth. Shows what each file imports recursively. One call replaces reading 5+ files.",
    input_schema: {
      type: "object",
      properties: {
        file: { type: "string", description: "File path relative to repo root" },
        depth: { type: "integer", description: "How many levels deep (default 2)" },
      },
      required: ["file"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 50, tier: "cloud" },
  },
  {
    name: "module_exports",
    description: "Public API surface of a file: exported function names with params. No bodies, no internals.",
    input_schema: {
      type: "object",
      properties: {
        file: { type: "string", description: "File path relative to repo root" },
      },
      required: ["file"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "search_code",
    description: "Search indexed code by name pattern. Finds functions, classes, types matching a substring.",
    input_schema: {
      type: "object",
      properties: {
        pattern: { type: "string", description: "Search pattern (substring match)" },
      },
      required: ["pattern"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "find_references",
    description: "Structural callers of a function with metadata. Replacement for grep 'Find References'.",
    input_schema: {
      type: "object",
      properties: {
        function_name: { type: "string", description: "Function name" },
        include_tests: { type: "boolean", description: "Include test files (default true)" },
      },
      required: ["function_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "dependency_chain",
    description: "Shortest dependency path between two files. Shows how they're connected through imports/calls.",
    input_schema: {
      type: "object",
      properties: {
        from_file: { type: "string", description: "Source file path" },
        to_file: { type: "string", description: "Target file path" },
      },
      required: ["from_file", "to_file"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 50, tier: "cloud" },
  },
  {
    name: "risk_score",
    description: "0-10 risk score for modifying a function. Combines blast radius, error history, hub status.",
    input_schema: {
      type: "object",
      properties: {
        function_name: { type: "string", description: "Function name" },
        file_path: { type: "string", description: "Optional file path" },
      },
      required: ["function_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "community_summary",
    description: "Most connected hub files/functions in the codebase. These are high-risk change targets.",
    input_schema: {
      type: "object",
      properties: {
        max_results: { type: "integer", description: "Number of results (default 10)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "decorated_with",
    description: "List all functions with a given decorator/annotation. e.g. 'app.route', 'lru_cache'.",
    input_schema: {
      type: "object",
      properties: {
        decorator_name: { type: "string", description: "Decorator name to search" },
      },
      required: ["decorator_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "pre_change_warning",
    description: "Before modifying a function, check structural and historical risk. Blast radius + risk score + warnings.",
    input_schema: {
      type: "object",
      properties: {
        function_name: { type: "string", description: "Function name" },
        file_path: { type: "string", description: "Optional file path" },
      },
      required: ["function_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 50, tier: "cloud" },
  },
  {
    name: "coupling_check",
    description: "Check if a new dependency between two modules violates existing architectural boundaries.",
    input_schema: {
      type: "object",
      properties: {
        from_module: { type: "string", description: "Source module path prefix" },
        to_module: { type: "string", description: "Target module path prefix" },
      },
      required: ["from_module", "to_module"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "co_change_partners",
    description: "Functions that historically change together with the target. Reveals hidden coupling.",
    input_schema: {
      type: "object",
      properties: {
        function_name: { type: "string", description: "Function name" },
        limit: { type: "integer", description: "Max results (default 10)" },
      },
      required: ["function_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "resolves_to",
    description: "Given a string/symbol, find matching functions/classes plus every function that mentions it.",
    input_schema: {
      type: "object",
      properties: {
        symbol: { type: "string", description: "Symbol or string to resolve" },
      },
      required: ["symbol"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  // ── K8s / Infrastructure tools (cloud, D1-backed) ──
  {
    name: "cluster_state",
    description: "Summary of a Kubernetes cluster: namespace count, pod count by status, deployments, services.",
    input_schema: {
      type: "object",
      properties: {
        cluster: { type: "string", description: "Cluster name (optional)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "list_pods",
    description: "List Kubernetes pods. Filter by namespace, status, or name substring.",
    input_schema: {
      type: "object",
      properties: {
        namespace: { type: "string", description: "Filter by namespace" },
        status: { type: "string", description: "Filter by status (Running, Pending, CrashLoopBackOff)" },
        name_contains: { type: "string", description: "Substring match on pod name" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "pod_story",
    description: "Narrative summary of pod events. The MTTR killer - shows what happened to a pod over time.",
    input_schema: {
      type: "object",
      properties: {
        pod: { type: "string", description: "Pod name (optional)" },
        namespace: { type: "string", description: "Namespace" },
        since_minutes: { type: "integer", description: "Look back N minutes (default 60)" },
        min_severity: { type: "string", description: "WARN or ERROR (default WARN)" },
        limit: { type: "integer", description: "Max events (default 15)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "host_state",
    description: "Host health snapshot: OS, CPU, memory, load, disk, failed systemd units.",
    input_schema: {
      type: "object",
      properties: {
        hostname: { type: "string", description: "Hostname (optional - omit for all hosts)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "host_story",
    description: "Narrative summary of host-level events and kernel events over time.",
    input_schema: {
      type: "object",
      properties: {
        hostname: { type: "string", description: "Hostname (optional)" },
        since_minutes: { type: "integer", description: "Look back N minutes (default 60)" },
        min_severity: { type: "string", description: "WARN or ERROR (default WARN)" },
        limit: { type: "integer", description: "Max events (default 15)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "deployment_info",
    description: "Full details for a K8s Deployment: replica status, image, labels, and all pods.",
    input_schema: {
      type: "object",
      properties: {
        namespace: { type: "string", description: "Namespace" },
        name: { type: "string", description: "Deployment name" },
      },
      required: ["namespace", "name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "pod_dependencies",
    description: "Every ConfigMap and Secret a Pod reads from. What config does this pod depend on?",
    input_schema: {
      type: "object",
      properties: {
        namespace: { type: "string", description: "Namespace" },
        pod: { type: "string", description: "Pod name" },
      },
      required: ["namespace", "pod"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "namespace_summary",
    description: "Everything in a namespace: deployments, pods by status, services, configmaps, secrets.",
    input_schema: {
      type: "object",
      properties: {
        namespace: { type: "string", description: "Namespace name" },
      },
      required: ["namespace"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  // ── Cloud tools (PAYG) ──
  {
    name: "diagnose_error",
    description: "Root cause file + line in 0.7s. Traces call chains through code + k8s + logs + Slack. Git blame context. Upstream trace.",
    input_schema: {
      type: "object",
      properties: {
        error_message: { type: "string", description: "The error message or stack trace" },
        file_path: { type: "string", description: "Optional file path for context" },
      },
      required: ["error_message"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 500, tier: "cloud" },
  },
  {
    name: "diagnose",
    description: "General error analysis with full context. Cross-layer diagnosis across code, infrastructure, and logs.",
    input_schema: {
      type: "object",
      properties: {
        error_message: { type: "string", description: "Error or symptom description" },
        min_severity: { type: "string", description: "Minimum severity (default WARN)" },
      },
      required: ["error_message"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 250, tier: "cloud" },
  },
  {
    name: "pr_risk",
    description: "8-check risk analysis per PR. Blast radius, affected downstream consumers, test coverage gaps, breaking change detection.",
    input_schema: {
      type: "object",
      properties: {
        diff: { type: "string", description: "Unified diff of the PR" },
        base_branch: { type: "string", description: "Base branch name" },
      },
      required: ["diff"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 200, tier: "cloud" },
  },
  {
    name: "diff_impact",
    description: "Blast radius per code change. What breaks if this code changes.",
    input_schema: {
      type: "object",
      properties: {
        diff: { type: "string", description: "Unified diff" },
      },
      required: ["diff"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 100, tier: "cloud" },
  },
  {
    name: "radar",
    description: "Personal what-did-I-miss digest. Surfaces drift between your code and production state.",
    input_schema: {
      type: "object",
      properties: {
        since_hours: { type: "number", description: "Look back N hours (default 24)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 100, tier: "cloud" },
  },
  {
    name: "unanswered_questions",
    description: "Surface unanswered questions and open issues from Slack, email, and communication channels. Finds what fell through the cracks.",
    input_schema: {
      type: "object",
      properties: {
        channel: { type: "string", description: "Channel or source to search (optional)" },
        since_hours: { type: "number", description: "Look back N hours (default 24)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 500, tier: "cloud" },
  },
  // ── Causal inference ──
  {
    name: "find_causes",
    description: "ROOT CAUSE ANALYSIS: Given an incident (crash, error, alert), traces the dependency graph backwards to find what caused it. Scores candidates by structural proximity, temporal correlation, and historical frequency. Returns ranked probable causes with confidence scores.",
    input_schema: {
      type: "object",
      properties: {
        node_name: { type: "string", description: "Name of the affected function, pod, or service" },
        event_type: { type: "string", description: "Type: error, pod_crash, alert, latency_spike (optional)" },
        lookback_minutes: { type: "integer", description: "How far back to search (default 60)" },
      },
      required: ["node_name"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 200, tier: "cloud" },
  },
  // ── GitHub tools (replaces GitHub MCP) ──
  ...GITHUB_TOOL_NAMES.map(name => ({
    name,
    description: {
      search_github_issues: "Search GitHub issues by query. Graph-enriched.",
      search_github_code: "Search code across GitHub repos.",
      search_github_prs: "Search pull requests.",
      list_github_issues: "List issues for a repo.",
      list_github_prs: "List pull requests for a repo. See open/closed PRs.",
      get_github_commit: "Get commit details with graph blast radius of changed files.",
      list_github_commits: "List recent commits for a repo or branch.",
      get_github_file: "Read a file from a GitHub repo.",
      add_github_comment: "Comment on an issue or PR.",
      create_github_pr: "Create a pull request.",
      merge_github_pr: "Merge a pull request.",
      list_github_actions: "List CI workflow runs. See which are failing.",
      get_github_action_run: "Get details of a specific CI run.",
      get_github_action_logs: "Get failed job details from a CI run.",
      list_github_releases: "List releases for a repo.",
      list_code_scanning_alerts: "List code scanning security alerts with graph context.",
    }[name] || name,
    input_schema: { type: "object", properties: { repo: { type: "string" }, query: { type: "string" } } },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" as const },
  })),
  // ── Linear tools (replaces Linear MCP) ──
  ...LINEAR_TOOL_NAMES.map(name => ({
    name,
    description: {
      search_linear_issues: "Search Linear issues by text query. Graph-enriched.",
      get_linear_issue: "Get full Linear issue with comments, relations, and code graph context.",
      create_linear_issue: "Create a new Linear issue.",
      update_linear_issue: "Update issue status, assignee, priority.",
      add_linear_comment: "Add a comment to a Linear issue.",
      list_linear_projects: "List all Linear projects with progress and leads.",
      list_linear_teams: "List teams and their members.",
      get_active_cycle: "Get the current active sprint/cycle with all issues.",
      list_linear_issues: "List issues filtered by team, status, or assignee.",
    }[name] || name,
    input_schema: { type: "object", properties: { query: { type: "string" }, issue_id: { type: "string" } } },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" as const },
  })),
  // ── Sentry tools (replaces Sentry MCP) ──
  {
    name: "get_sentry_issue",
    description: "Get full Sentry issue details: stack trace, breadcrumbs, tags, assigned user, occurrence count. Use with an issue ID or short ID.",
    input_schema: { type: "object", properties: { issue_id: { type: "string", description: "Sentry issue ID or short ID (e.g. PROJ-123)" } }, required: ["issue_id"] },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "search_sentry_issues",
    description: "Search Sentry issues by query. Supports Sentry search syntax: is:unresolved, assigned:me, level:error, etc.",
    input_schema: { type: "object", properties: { query: { type: "string", description: "Search query (e.g. 'is:unresolved TypeError')" }, project: { type: "string", description: "Project slug (optional)" } }, required: ["query"] },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "search_sentry_events",
    description: "Search raw Sentry events across all projects. Find specific error occurrences.",
    input_schema: { type: "object", properties: { query: { type: "string", description: "Event search query" }, project: { type: "string", description: "Project slug (optional)" } }, required: ["query"] },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "update_sentry_issue",
    description: "Resolve, unresolve, ignore, or assign a Sentry issue. Close the loop after fixing a bug.",
    input_schema: { type: "object", properties: { issue_id: { type: "string", description: "Issue ID" }, status: { type: "string", description: "resolved, unresolved, or ignored" }, assigned_to: { type: "string", description: "Username or email to assign to" } }, required: ["issue_id"] },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "find_sentry_releases",
    description: "List recent Sentry releases with deploy dates, new error counts, and commit counts. See what's deployed.",
    input_schema: { type: "object", properties: { project: { type: "string", description: "Project slug (optional)" } } },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  {
    name: "get_sentry_issue_tags",
    description: "Get tag value distribution for a Sentry issue. See which browsers, OS, releases, or users are affected.",
    input_schema: { type: "object", properties: { issue_id: { type: "string", description: "Issue ID" }, tag: { type: "string", description: "Tag name: browser, os, release, environment, user, etc." } }, required: ["issue_id", "tag"] },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 10, tier: "cloud" },
  },
  // ── Agent-backed infrastructure tools (queries remote agents) ──
  {
    name: "host_health",
    description: "Remote system health: CPU, memory, load, disk, failed services. Queries a savants agent running on the target machine. Use for production server health checks.",
    input_schema: {
      type: "object",
      properties: {
        agent: { type: "string", description: "Agent name or ID (optional - uses first online agent)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "pod_status",
    description: "Remote Kubernetes pod status from a savants agent with kubectl access. Structured output with namespace, status, restarts.",
    input_schema: {
      type: "object",
      properties: {
        agent: { type: "string", description: "Agent name or ID (optional)" },
        namespace: { type: "string", description: "Namespace filter (optional)" },
        name: { type: "string", description: "Pod name substring filter (optional)" },
        status: { type: "string", description: "Status filter: Running, CrashLoopBackOff, etc. (optional)" },
      },
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
  {
    name: "pod_logs",
    description: "Remote pod logs from a savants agent. Classified by severity (INFO/WARN/ERROR). Use when debugging why a pod is crashing.",
    input_schema: {
      type: "object",
      properties: {
        agent: { type: "string", description: "Agent name or ID (optional)" },
        pod: { type: "string", description: "Pod name or substring" },
        namespace: { type: "string", description: "Namespace (default: default)" },
        lines: { type: "integer", description: "Number of lines (default: 100)" },
        min_severity: { type: "string", description: "Minimum: INFO, WARN, ERROR (default: WARN)" },
      },
      required: ["pod"],
    },
    pricing: { free_monthly_calls: null, overage_per_call_cents: 30, tier: "cloud" },
  },
];

// Helper: resolve project by repo name, then fall back to most recent
async function resolveProjectId(db: Env["DB"], orgId: string, repoName?: string): Promise<string | null> {
  if (repoName) {
    const slug = repoName.toLowerCase().replace(/[^a-z0-9-]/g, "-");
    const byName = await db.prepare(
      "SELECT id FROM projects WHERE org_id = ?1 AND (slug = ?2 OR name = ?3) LIMIT 1"
    ).bind(orgId, slug, repoName).first<{ id: string }>();
    if (byName) return byName.id;
  }
  const row = await db.prepare(
    "SELECT id FROM projects WHERE org_id = ?1 ORDER BY updated_at DESC LIMIT 1"
  ).bind(orgId).first<{ id: string }>();
  return row?.id ?? null;
}

// GET /api/v1/tools - Return tool list
tools.get("/", async (c) => {
  return c.json({ tools: TOOL_LIST });
});

// POST /api/v1/tools/call - Proxy a tool call to astra
tools.post("/call", async (c) => {
  const auth = c.get("auth");
  const body = await c.req.json<{ tool: string; input: Record<string, unknown> }>();

  if (!body.tool || !body.input) {
    return c.json({ error: "invalid_request", message: "tool and input are required", status: 400 }, 400);
  }

  const toolDef = TOOL_LIST.find((t) => t.name === body.tool);
  if (!toolDef) {
    return c.json({ error: "unknown_tool", message: `Tool '${body.tool}' not found`, status: 404 }, 404);
  }

  // Deduct credits (local tools cost 0 credits, always pass)
  const creditResult = await deductCredits(c.env.DB, auth.orgId, body.tool);

  if (!creditResult.ok) {
    return c.json(
      {
        error: "insufficient_credits",
        message: creditResult.message,
        credits: {
          balance: creditResult.balance,
          cost: creditResult.cost,
          tool: body.tool,
        },
        purchase_url: "/api/v1/credits/purchase",
        status: 402,
      },
      402
    );
  }

  const org = await getOrgById(c.env.DB, auth.orgId);

  const startTime = Date.now();
  let proxyResult: Record<string, unknown>;

  // ── Causal inference ──
  if (body.tool === "find_causes") {
    try {
      const repoName = (body.input.repo as string) || (body.input.repo_name as string);
      const projectId = (body.input.project_id as string) || await resolveProjectId(c.env.DB, auth.orgId, repoName);
      if (!projectId) {
        proxyResult = { error: "no_project", message: "No project found. Run 'savants reindex' first." };
      } else {
        proxyResult = await toolFindCauses(c.env.DB, projectId, body.input);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : "Causal analysis failed";
      return c.json({ error: "causal_error", message, status: 500 }, 500);
    }
  }
  // ── Sentry tools (replaces Sentry MCP) ──
  else if (SENTRY_TOOL_NAMES.includes(body.tool)) {
    try {
      proxyResult = await executeSentryTool(c.env.DB, auth.orgId, body.tool, body.input);
    } catch (err) {
      const message = err instanceof Error ? err.message : "Sentry tool failed";
      return c.json({ error: "sentry_tool_error", message, status: 500 }, 500);
    }
  }
  // ── Linear tools (replaces Linear MCP) ──
  else if (LINEAR_TOOL_NAMES.includes(body.tool)) {
    try {
      proxyResult = await executeLinearTool(c.env.DB, auth.orgId, body.tool, body.input);
    } catch (err) {
      const message = err instanceof Error ? err.message : "Linear tool failed";
      return c.json({ error: "linear_tool_error", message, status: 500 }, 500);
    }
  }
  // ── GitHub tools (replaces GitHub MCP) ──
  else if (GITHUB_TOOL_NAMES.includes(body.tool)) {
    try {
      proxyResult = await executeGitHubTool(c.env.DB, auth.orgId, body.tool, body.input);
    } catch (err) {
      const message = err instanceof Error ? err.message : "GitHub tool failed";
      return c.json({ error: "github_tool_error", message, status: 500 }, 500);
    }
  }
  // ── Handle diagnose_error directly (uses all available sources) ──
  else if (body.tool === "diagnose_error" || body.tool === "diagnose") {
    try {
      const result = await diagnoseError(c.env, auth.orgId, {
        error_message: (body.input.error_message as string) || (body.input.error as string) || (body.input.query as string) || "",
        file_path: (body.input.file_path as string) || undefined,
        sentry_event_id: (body.input.sentry_event_id as string) || undefined,
        sentry_project: (body.input.sentry_project as string) || undefined,
      });
      proxyResult = result as unknown as Record<string, unknown>;
    } catch (err) {
      const message = err instanceof Error ? err.message : "Diagnosis failed";
      return c.json({ error: "diagnosis_error", message, status: 500 }, 500);
    }
  }
  // ── Code analysis tools (D1-backed) ──
  else if (GRAPH_TOOL_NAMES.includes(body.tool)) {
    try {
      // Resolve project by repo name or project_id
      const repoName = (body.input.repo as string) || (body.input.repo_name as string);
      const projectId = (body.input.project_id as string) || await resolveProjectId(c.env.DB, auth.orgId, repoName);
      if (!projectId) {
        return c.json({
          error: "no_project",
          message: "No project found. Run 'savants reindex' to index your codebase first.",
          status: 404,
        }, 404);
      }
      proxyResult = await executeGraphTool(c.env.DB, projectId, body.tool, body.input);
    } catch (err) {
      const message = err instanceof Error ? err.message : "Analysis failed";
      return c.json({ error: "graph_tool_error", message, status: 500 }, 500);
    }
  }
  // ── Agent-backed infra tools (route to remote agent) ──
  else if (["host_health", "pod_status", "pod_logs"].includes(body.tool)) {
    try {
      const agentName = (body.input.agent as string) || undefined;

      // Find an online agent
      let agentId: string | undefined;
      if (agentName) {
        const agent = await c.env.DB.prepare(
          "SELECT id FROM agents WHERE org_id = ?1 AND (name = ?2 OR id = ?2) AND status = 'online' LIMIT 1"
        ).bind(auth.orgId, agentName).first<{ id: string }>();
        agentId = agent?.id;
      } else {
        const agent = await c.env.DB.prepare(
          "SELECT id FROM agents WHERE org_id = ?1 AND status = 'online' ORDER BY last_heartbeat DESC LIMIT 1"
        ).bind(auth.orgId).first<{ id: string }>();
        agentId = agent?.id;
      }

      if (!agentId) {
        proxyResult = {
          tool: body.tool,
          status: "no_agent",
          message: "No online agents. Install savants on your server and run: savants agent start",
        };
      } else {
        // Create query and wait for agent to respond
        const queryId = crypto.randomUUID();
        await c.env.DB.prepare(
          "INSERT INTO agent_queries (id, org_id, agent_id, tool, input, status) VALUES (?1, ?2, ?3, ?4, ?5, 'pending')"
        ).bind(queryId, auth.orgId, agentId, body.tool, JSON.stringify(body.input || {})).run();

        // Long-poll for result (up to 30s)
        let result: { result: string; status: string } | null = null;
        const deadline = Date.now() + 30000;
        while (Date.now() < deadline) {
          result = await c.env.DB.prepare(
            "SELECT result, status FROM agent_queries WHERE id = ?1 AND status = 'completed'"
          ).bind(queryId).first();
          if (result) break;
          await new Promise(r => setTimeout(r, 500));
        }

        if (result) {
          proxyResult = JSON.parse(result.result || "{}");
        } else {
          await c.env.DB.prepare("UPDATE agent_queries SET status = 'timeout' WHERE id = ?1").bind(queryId).run();
          proxyResult = { tool: body.tool, status: "timeout", message: "Agent did not respond within 30s" };
        }
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : "Agent query failed";
      return c.json({ error: "agent_error", message, status: 500 }, 500);
    }
  }
  // ── Tools that need more context (pr_risk needs diff parsing, radar needs integrations) ──
  else if (["pr_risk", "radar", "unanswered_questions"].includes(body.tool)) {
    proxyResult = {
      tool: body.tool,
      status: "needs_graph",
      message: `${body.tool} requires your codebase to be indexed. Run 'savants reindex' in your repo first.`,
      hint: "Use blast_radius, impact_analysis, function_xray, dead_code for code analysis. Use diagnose_error for error diagnosis with Sentry.",
    };
  }
  else {
    return c.json(
      { error: "not_available", message: `Tool '${body.tool}' is not available. Run 'savants reindex' to index your codebase first.`, status: 503 },
      503
    );
  }

  const durationMs = Date.now() - startTime;

  // Audit: tool call
  const meta = requestMeta(c.req.raw);
  await audit(c.env.DB, {
    orgId: auth.orgId, actorId: auth.userId,
    action: "tool.call", resourceType: "tool", resourceId: body.tool,
    metadata: { credits_cost: creditResult.cost, duration_ms: durationMs },
    ...meta,
  });

  // Log usage
  await logUsageEvent(c.env.DB, {
    id: crypto.randomUUID(),
    orgId: auth.orgId,
    userId: auth.userId,
    toolName: body.tool,
    graphScopeId: null,
    tokensIn: (proxyResult.tokens_in as number) ?? 0,
    tokensOut: (proxyResult.tokens_out as number) ?? 0,
    durationMs,
  });

  return c.json({
    tool: body.tool,
    result: proxyResult,
    performance: {
      duration_ms: durationMs,
    },
    credits: {
      cost: creditResult.cost,
      balance: creditResult.balance,
    },
  });
});

export default tools;
