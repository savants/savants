# Strategy Decision History

This document preserves the strategic reasoning behind major direction changes, so future contributors and future-us can understand *why* decisions were made, not just *what* was decided.

## 2026-04-07: Initial direction — Open Core + Agentic SaaS

**Tag:** `v0.1.0-foss-checkpoint`
**Branch:** `foss-direction`
**File:** `BUSINESS.md` (preserved in the tag)

### Context
SynapCode was built as a local-first GraphRAG cognitive stack using FalkorDB, Temporal, Open WebUI Pipelines, and Tauri. By this checkpoint:

- 95/95 tests passing against real FalkorDB (zero mocks)
- All 10 CLI commands verified end-to-end
- Tauri desktop app builds to a 13MB binary, spawns FalkorDB sidecar correctly
- E2E headless test passes under Xvfb
- 4 user story golden paths verified (3AM on-call, new hire onboarding, risky refactor, daily incremental sync)

### Strategy proposed
**Open Core + Agentic SaaS:**
- Apache 2.0 licensed CLI + desktop app
- Paid tiers: Pro ($19/mo), Team ($39/user/mo), Enterprise ($500-$1k/seat/yr)
- Revenue from team collaboration, LLM agents, enterprise compliance
- Comparable to Sourcegraph ($100M ARR) / Temporal / GitLab playbooks
- Projected $3M ARR Y1, $30M Y2, $100M Y3

### Why we considered it
- FOSS drives viral adoption on HN, Reddit, dev Twitter
- Community contributions = free engineering
- "Read the code" trust narrative
- Proven path for dev infrastructure (uv, Ollama, Zed, Bun, Supabase)
- Lower capital requirement than closed-source model

### Why we reconsidered
1. **FOSS invites forks and commoditization.** Aider, Continue.dev, and others already occupy the free end of the market. A FOSS SynapCode would be one of many.
2. **FalkorDB Cloud already exists.** Reselling hosted FalkorDB isn't a differentiated business.
3. **The local-first pitch is stronger with closed source.** "We literally cannot see your code because we don't run servers" is a more powerful enterprise story than "trust our open code."
4. **BYO LLM API keys eliminate cloud infrastructure costs entirely.** No inference bills, no Temporal Cloud dependency, no FalkorDB hosting cost. Margin goes from ~40% to ~95%.
5. **Closed-source dev tools are profitable and acquirable.** Sublime, Tower, Dash, TablePlus, BBEdit all coexist with free alternatives. Windsurf sold for ~$2.4B, Cursor at $9B+.
6. **Direct revenue from day 1** provides stronger product signal than GitHub stars.
7. **Bootstrap-friendly:** Sovereign Closed can launch with ~$1,500 in annual fixed costs vs. needing $2-5M seed for the FOSS cloud play.

## 2026-04-07: Pivot — Sovereign Closed

**Direction:** Closed-source, code-signed, paid binary with BYO LLM keys.

### Strategy
- **Closed binary:** Tauri desktop app, signed on macOS (Apple Developer), Windows (EV cert), Linux (GPG + Sigstore)
- **License enforcement:** Keygen.sh + hardware fingerprint + periodic activation
- **Data model:** 100% local. Graph lives on user hardware. Code never uploaded.
- **Inference model:** User provides their own Anthropic/OpenAI/local model keys.
- **Pricing:**
  - Free 14-day trial
  - Individual: $99/year
  - Team (post month 6): $199/user/year
  - Enterprise (year 2): $800-$1,200/seat/year
- **Revenue target:** $100k ARR in 6 months, $1M ARR year 1 from individuals alone, $5M ARR year 2 with team tier

### Why this instead
1. **Unique wedge protection.** Nobody else ships local-first GraphRAG with durable agents. Closing the source keeps the wedge ours while we scale.
2. **Stronger privacy pitch.** Enterprises trust paid closed-source local tools (Dash, Tower, Kaleidoscope model) because architectural isolation beats inspectability.
3. **Zero inference cost.** BYO keys means users pay Anthropic/OpenAI directly. Our margin is ~95%.
4. **Bootstrappable.** No seed round required. $1,500 in year 1 fixed costs (Apple Developer, Windows EV cert, Keygen.sh, domain) gets us live.
5. **Velocity is the moat, not code visibility.** Sublime Text has been closed for 17 years and outshipped every FOSS clone.
6. **Clear exit optionality.** Closed dev tools get acquired (Windsurf → Google $2.4B). FOSS tools rarely do because the value is already distributed.
7. **FOSS clone resilience is empirically strong.** Zero closed dev tools in history have been killed by FOSS clones (Dash/Zeal, Tower/SourceTree, Sublime/Atom, Kaleidoscope/Meld, TablePlus/DBeaver all prove this).

### What we preserve from the FOSS direction
- The architecture (Tauri + FalkorDB + Temporal + Python backend)
- The test suite and golden paths
- The MCP server (for integration with Claude Code / Cursor)
- The Rust port plan (now more important — single-binary distribution for closed)
- The local-first promise (data never leaves the user's machine)

### Technical implications of the pivot
1. **Repo goes private** — no public GitHub source
2. **Rust port accelerated** — Python is too easy to decompile; single Rust binary is the shipping artifact
3. **License validation added** to Tauri boot sequence (before `setup()` spawns sidecars)
4. **Proprietary EULA** replaces Apache 2.0 `LICENSE`
5. **Public docs repo stays separate** for user-facing documentation
6. **CI/CD pivots** — internal only; no public workflows that leak architecture details

### Fallback plan
If the Sovereign Closed direction fails (no customers in first 90 days, strong market signal against closed tools, acquisition of a key dependency), we can return to the FOSS direction via:

```bash
git checkout foss-direction
# or
git reset --hard v0.1.0-foss-checkpoint
```

The FOSS checkpoint preserves all code, tests, and the original `BUSINESS.md` intact.

## Decision log format for future entries

When making a major strategic decision:

1. **Add a tag** at the current state: `git tag -a vX.Y-<name>-checkpoint`
2. **Create a branch** at that tag: `git branch <name>-direction <tag>`
3. **Document here** with: date, context, options considered, why we chose, what we preserve, fallback plan
4. **Link the tag + branch** so future us can return

This pattern keeps strategic optionality in version control.
