# SynapCode Strategy and Business Model

**Status:** Settled. Decided 2026-04-08. Do not re-litigate without strong new evidence.

This document captures the product, licensing, distribution, and pricing
strategy for SynapCode. It exists so future contributors (human or
agent) don't repeat the same strategic conversations.

---

## TL;DR

- **Closed source.** All of it. Parser, MCP server, CLI, schema, desktop app, cloud control plane. Source-available under NDA for enterprise audits, never publicly released.
- **Cloudflare-style pricing.** Free local tier ruthlessly complete, paid Team/Business/Enterprise tiers for cloud features that genuinely cost us money.
- **Trust comes from metadata-not-source, empirically verifiable.** Not from source availability.
- **Three killer use cases lead the marketing:** AI agent grounding, PR review (`diff_impact`), refactor safety. Skip the rest.
- **Secret scrubber is a hard prerequisite for the cloud tier.** Don't ship paid until it lands.

---

## 1. Licensing: closed source, not open core

**Decision:** SynapCode is closed source. All components: parser (Python and Rust), CLI, MCP server, graph schema, desktop launcher, cloud control plane.

**Why we considered open source and rejected it:**

1. **The parser details are real moats, not commodity.** The string-literal symbol heuristic that closes the registry-dispatch blind spot, the decorator extraction across `decorated_definition` wrappers, the CALLS edge file-path disambiguation, the METHOD_OF emission ordering, the config file flattening rules — each of these took multiple iterations to get right. A competitor with our open source code skips months of dead-ends.

2. **The MCP tool surface IS the product.** `function_xray`, `find_references_structured`, `decorated_with`, `resolves_to`, `diff_impact`, `risk_score` aren't thin wrappers — they're carefully composed multi-query operations. Open-sourcing them lets a competitor copy the interface verbatim and ship a clone.

3. **Trust no longer requires open source in this category.** Cursor, Claude Code, Codeium, GitHub Copilot, and Continue are all closed source and developers install them every day with zero hesitation. The trust mechanism is brand and the empirical metadata-not-source story, not source availability. Anthropic shipping Claude Code as a closed binary even though they're an OSS-friendly research org is the strongest signal that closed is correct for AI coding tools in 2026.

4. **The "open source for distribution" argument is weaker than it looks.** PyPI accepts closed-source wheels. Personal Homebrew taps don't require OSS. Curl-pipe-bash installers don't care about license. The only place open source is genuinely required is the official Homebrew core repo and the official Nixpkgs repo — both have workarounds.

**For enterprise procurement audits:** offer source-available under NDA, not public OSS. Customer signs NDA, gets read-only access to a specific commit, can audit, can't redistribute, can't self-host without a separate license. Standard pattern (Sourcegraph, Mattermost, Confluent, MongoDB Enterprise).

---

## 2. Pricing tiers

```
┌───────────────────────────────────────────────────────────────┐
│ Free (Local)                                  $0              │
│ ─────────────────────────────────────────────────────────────  │
│ • Local CLI + desktop app + launcher                           │
│ • Unlimited repos, files, queries, history                     │
│ • Full graph + all 15 MCP tools                                │
│ • Single user, single machine                                  │
│ • Closed binary, runs offline, never phones home               │
└───────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────┐
│ Team                              $20/dev/month                │
│ ─────────────────────────────────────────────────────────────  │
│ Everything in Free, plus:                                      │
│ • Hosted shared graph (we run FalkorDB)                        │
│ • GitHub/GitLab webhooks (always-on indexing)                  │
│ • Team SSO (Google / GitHub / Slack)                           │
│ • Web UI for the shared graph                                  │
│ • Slack integration (basic — channel mentions → graph)         │
│ • Email support, 24h response                                  │
└───────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────┐
│ Business                          $50/dev/month                │
│ ─────────────────────────────────────────────────────────────  │
│ Everything in Team, plus:                                      │
│ • Linear / Jira / Notion integrations                          │
│ • Meeting transcript integration (Granola/Otter)               │
│ • Per-user audit logs                                          │
│ • SAML SSO                                                     │
│ • 99.9% SLA                                                    │
└───────────────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────────────────┐
│ Enterprise                       custom (~$50k+/yr)            │
│ ─────────────────────────────────────────────────────────────  │
│ Everything in Business, plus:                                  │
│ • Self-hosted in customer VPC / on-prem                        │
│ • TEE / Confidential Compute mode                              │
│ • SCIM provisioning, SIEM export                               │
│ • Dedicated support, custom integrations                       │
│ • SOC 2 / ISO 27001 attestations                               │
└───────────────────────────────────────────────────────────────┘
```

### Pricing rules

**Never cripple the free tier.** No "free up to N repos." No "free up to N queries." No "free history limited to 30 commits." The free tier must feel COMPLETE for the solo-dev problem it solves. People don't upgrade from a hobbled tool — they leave.

**Charge for things that genuinely cost us money to deliver.** The cloud tier is what you pay for because:
- Single instance can't share an in-memory graph across two laptops (coordination problem, not capacity)
- Webhooks need a 24/7 endpoint, laptops sleep
- Always-on MCP for Slack bots, CI runners, agent integrations
- Compliance / audit / SSO are real per-customer costs

**The cloud tier solves a different problem than the local tier**, not the same problem with a price tag. This is the Cloudflare / Sentry / Sourcegraph (good era) playbook.

### Unit economics

- 100-developer customer hits ~550K API calls/month
- Single $60/month VM serves ~500-1000 such customers
- **Per-customer infrastructure cost: ~$0.06-$0.12/month**
- Customer pays $2,400/month (100 devs × $24)
- **Gross margin: 99%+**

Compare to the competition:
- **Cursor:** LLM inference is 70%+ of revenue
- **Datadog:** petabyte storage, ~30% of revenue
- **SynapCode:** ~0.001% of revenue

This is structural, not aspirational. We store relationships not content (~450x compression vs raw inputs), so per-customer cost is essentially zero regardless of customer size. The full math lives in `docs/scaling-and-throughput.md`.

### Revenue targets

- **Pessimistic (1 year):** ~$50k ARR. Side project that pays for hosting.
- **Realistic (18 months):** **~$1-2M ARR.** 500 paid orgs averaging 12 devs at $20/dev/month + 5 enterprise contracts. Plan for this.
- **Aggressive (3 years):** ~$20M+ ARR. 2,500 paid orgs + 50 enterprise contracts.

---

## 3. Trust story: metadata, not source

**Pitch:** SynapCode stores structural metadata about code, not the code itself. ~450x compression vs raw inputs. Verifiable with a packet capture in 30 seconds.

**What we store:**
- Function/Class names + file paths + line numbers
- CALLS / METHOD_OF / REFERENCES_SYMBOL / CONTAINS / CHANGES edges
- Decorator names
- ConfigKey dotted paths + values from YAML/TOML/JSON
- EnvVar names + default values
- Git author + commit timestamp + commit SHA
- Docstrings (truncated to 500 chars)
- String literals that look like symbol names

**What we don't store:**
- Function bodies / implementation
- Full file contents
- Runtime values of env vars
- The actual diff content of commits
- PII beyond git author email (which is itself a known compliance issue we'll need to handle)

**Why this is a stronger trust argument than open source:**
- Empirical: customer can `tcpdump` and verify zero outbound traffic from the local tier
- Quantitative: ~450x compression is a measurable claim, not a vibe
- Universal: works for customers who don't read code (which is most of them)
- Doesn't require giving away the moat

### The 20% catch (don't forget this)

Metadata can still leak meaningful information:

1. **ConfigKey values can be secrets.** A customer's checked-in `mongod.conf` contains `database.password = hunter2`, and we'd index it as a ConfigKey value.
2. **String literals can be API keys** if they pass the `_looks_like_symbol` filter.
3. **EnvVar names alone are recon.** `STRIPE_LIVE_SECRET_KEY` tells an attacker the service touches Stripe.
4. **The graph shape is an attack-surface map.** "There's a `bypass_2fa_for_admins` function reachable from `internal_admin_route`" is intel even without the source.
5. **Git author emails are PII** under GDPR.

**Therefore: secret scrubbing is a hard prerequisite for the cloud tier.** See section 5.

### What we still need (cheap because the data is small)

- TLS in transit (table stakes)
- Encryption at rest (one EBS checkbox)
- Tenant isolation (every API call enforces org boundary)
- Authn/authz (per-org API tokens, scoped, revocable)
- Audit logs (who queried what, when)
- SOC 2 / ISO 27001 compliance (for enterprise tier)
- BYOK (year-2 problem)

### What we don't need (that competitors do)

- Per-line ACLs — we store edges, not lines
- TEE / Confidential Compute except as enterprise upsell
- Petabyte-scale key rotation
- Residency-aware sharding
- Embedding-based redaction (we don't run LLMs)

---

## 4. Killer use cases (lead the marketing with these)

**Three use cases sell the product. Skip the rest in the homepage and demos.**

1. **AI agent grounding.** Every Cursor / Claude Code / Continue context query today uses embeddings — fuzzy, hallucinatory, unaware of registries or dynamic dispatch. SynapCode answers structurally and correctly via MCP. Pitch: *"Your AI assistant stops making things up about your codebase."* Largest market.

2. **PR review at the org level (`diff_impact`).** Replace the human guess-the-blast-radius with structured facts: "This PR touches 14 entry points, 3 of which are Temporal workflows, modifies `operationProfiling.mode`, and is tested by 8 functions." Worth $20/dev/month by itself because it prevents one 1am incident per quarter.

3. **Refactor safety.** *"I want to delete `HandleTsCoinTransfer`. What breaks?"* Takes a senior engineer 20 minutes today. SynapCode answers in milliseconds with verifiable receipts (string-literal references close the registry-dispatch blind spot). The "oh shit" moment in every sales call.

**Use cases to deprioritize in marketing** (real but not the wedge):
- Incident response (only valuable during incidents)
- Compliance / audit (enterprise-only, not daily-use driver)
- Architecture review (staff-eng niche)
- Documentation generation
- Test coverage gap detection
- Code search for humans (Sourcegraph owns this market)

---

## 5. Secret scrubber (cloud tier prerequisite)

**Hard prerequisite: secret scrubber must exist in the parser ingest path BEFORE the cloud tier launches.**

If we ship cloud without it, the first customer with a checked-in `mongod.conf` or `.env` file leaks credentials into our graph. That's a breach disclosure.

**Where to scrub:**
- ConfigKey values during `_flatten_config.push_leaf` (biggest leak vector)
- string_refs filter in `_looks_like_symbol` (or right before capture)
- EnvVar default values in `_extract_env_var_call`

**Patterns to detect:**
- AWS access keys: `AKIA[0-9A-Z]{16}`
- OpenAI/Anthropic keys: `sk-[a-zA-Z0-9-_]{20,}`, `sk-ant-...`
- GitHub tokens: `gh[psaur]_[A-Za-z0-9]{36,}`
- JWTs: `eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+`
- Postgres URI with password: `postgres(?:ql)?://[^:]+:[^@]+@`
- MongoDB URI with password: `mongodb(?:\+srv)?://[^:]+:[^@]+@`
- Generic high-entropy: long base64-ish strings (32+ chars, high entropy)
- Private keys: `-----BEGIN [A-Z ]+ PRIVATE KEY-----`

**Use existing battle-tested rules** from `detect-secrets`, `gitleaks`, or `trufflehog`. Don't invent new regexes.

**Implementation:** new module `src/savants/security/secret_scrub.py`. Single function `scrub(value: str) -> tuple[str, bool]` returns `(possibly-redacted-value, was_secret)`. Mirror to Rust.

---

## 6. Distribution (the "one command install" problem)

**Phase 1 (1 day):** Linux x86_64 wheel via PyPI.
- Move binaries into `src/savants/binaries/` for `package_data` inclusion
- Update `_find_bundled_binary()` to check `importlib.resources` first
- `pip install savants` works on Linux x86_64 zero-config

**Phase 2 (1 week):** Cross-platform CI build matrix via `cibuildwheel`.
- Linux x86_64 + arm64
- macOS x86_64 + arm64
- (Maybe Windows; FalkorDB on Windows is painful, probably ship WSL guidance instead)
- Per-tag GitHub Actions release publishing to PyPI

**Phase 3 (parallel to 2):** Universal `curl -fsSL get.savants.dev | sh` installer.
- Detects platform via `uname -ms`
- Downloads matching tarball from GitHub release
- Drops binaries in `~/.local/share/savants/`
- Symlinks `savants` into `~/.local/bin/`
- This is what `uv`, `bun`, `mise`, `rustup` all do

**Phase 4 (weeks):** Tauri desktop launcher.
- DMG / .deb / AppImage / .msi
- Spacebar launcher UI
- Real product, not just a wrapper

**What we deliberately skip:**
- Snap / Flatpak (Linux-only, niche)
- Conda (its own packaging hellscape)
- APT/RPM repos (overkill for now)
- AUR (do after curl installer exists)

---

## 7. Deferred items

These are real and worth doing eventually, but explicitly **not** part of the initial scope. Don't propose them as "next steps" without flagging the effort scale:

1. **Web UI / launcher** — week-scale. The single biggest legibility win. Build after distribution + secret scrubber + cloud MVP.
2. **Semantic diff** — needs a snapshot store. PR reviewers see "added 3 methods, removed 1 class, decorator @workflow.defn added to 2 functions." Days to weeks.
3. **Cross-language call resolution** — Python calling TS via HTTP is invisible today. Needs per-language import resolver + type flow. Weeks.

---

## 8. Architecture: hide internal stack from customers

**External surfaces (errors, --help, MCP tool descriptions, doctor output) should NOT mention:**
- FalkorDB
- Redis
- Cypher
- Tree-sitter

**Why:** the customer should see "SynapCode" everywhere. From their perspective they get a structural code intelligence service, not a database. This:
- Reduces vendor lock-in concerns
- Lets us swap the underlying graph DB later if we want
- Avoids "oh, FalkorDB, isn't that just Redis?" objections in sales conversations
- Matches what every other dev tool company does (Cursor doesn't say "powered by Anthropic API" in the UI)

**Internal code, comments, and dependency files can keep the names** — engineers reading the source obviously need to know what's underneath. The hiding is for user-facing strings only.

---

## 9. What this document does NOT cover

- **Specific feature roadmap** — see `docs/roadmap-other-features.md`
- **Detailed architecture** — see `docs/architecture-layered-graphs.md`
- **Throughput math** — see `docs/scaling-and-throughput.md`
- **Real fastapi findings (proof of value)** — see `docs/fastapi-analysis.md`

This document is **the why behind the what.** It exists so the strategic decisions don't have to be re-litigated every session. If you find yourself debating whether to open source the parser or whether to add free-tier limits, read this file first.
