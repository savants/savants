# HN Launch Post

## Title (options - pick one)

1. Show HN: Savants - Find code by describing what it does, not what it's called
2. Show HN: Savants - Semantic code search for AI agents (90% accuracy, <400ms)
3. Show HN: Savants - MCP tools that replace grep for AI coding agents

## Post body

We built Savants because AI coding agents waste 60% of their context window on grep noise.

When Claude or Cursor searches your codebase, it runs grep, gets 47 matches, reads each file, and burns thousands of tokens finding the right function. Savants replaces that entire loop with one call.

**How it works:**

1. `curl -fsSL savants.sh | sh` (5MB binary, zero deps)
2. `savants reindex` (parses your repo with tree-sitter, embeds with ONNX)
3. Your AI agent calls `semantic_search("payment retry logic")` instead of `grep -rn "retry"`

It finds `handleTransactionWithBackoff` on the first try. 90% accuracy on real codebases (tested on Fastify - 287 files, 2,152 functions).

**5 MCP tools, all local:**

- `semantic_search` - natural language code search (<400ms)
- `file_skeleton` - function signatures without bodies (10x fewer tokens)
- `where_used` - every caller and importer from the call graph
- `callers` - exact call chain, not text matches
- `reindex` - auto-detects stale indexes

**What it is not:**

- Not a cloud service (runs entirely on your machine)
- Not an LLM wrapper (uses ONNX embeddings locally)
- No API keys, no telemetry, no network calls
- FSL-1.1-Apache-2.0 (converts to Apache 2.0 after 2 years)

**Cloud tier** ($99/cluster/mo) adds production intelligence - error diagnosis, PR risk scoring, Slack bot. But the core search tools are free forever.

Built in Rust. 5.4MB binary. Works with Claude Code, Cursor, and Windsurf.

https://savants.dev

GitHub: https://github.com/savants-dev/savants

## Comments to prepare for

**"How is this different from GitHub code search?"**
GitHub searches text. We search meaning. "function that validates log level options" returns nothing on GitHub but finds `validateLogLevelOption` with Savants.

**"Why not just use embeddings with Chroma/Pinecone?"**
You could, but you'd need to set up a vector DB, write the ingestion pipeline, build the MCP server, handle incremental updates, and manage the embedding model. Savants is one binary.

**"90% accuracy sounds made up"**
Tested on Fastify (33K stars). 9 out of 10 natural language queries return the correct function as the top result. We published the test queries and results in the case study.

**"Why Rust?"**
Speed. Index 2,152 functions in 15 seconds. Cached searches in <400ms. The binary is 5.4MB with no runtime dependencies.

**"FSL license is not open source"**
Correct. It's source-available. You can read, modify, and self-host it. You can't resell it as a competing SaaS. It converts to Apache 2.0 after 2 years. This is the same model as MariaDB, Sentry, and GitLab.
