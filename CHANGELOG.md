# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-06-25

### Added
- **Framework integrations**: LangChain callback, CrewAI hook, OpenAI Agents SDK guardrail (`savants_guard.integrations`)
- **Risk scoring**: `RiskContext`, `file_risk()`, `change_size()`, `risk_aware_safety()` preset
- **Audit trail API**: `GET /api/v1/guard/export` (CSV/JSON), `GET /api/v1/guard/stats`
- **Analytics dashboard**: `savants.cloud/dashboard/guard-analytics` with metrics, top rules, daily chart
- **MCP Security Monitor**: `savants mcp audit`, `savants mcp stats`, MCP call logging
- **Cloud sync**: `savants guard sync` pushes local events to Savants Cloud
- **Enforcement log**: events written to `guard_enforcement_log` table for compliance

### Changed
- Guard profiles use soft actions: `suggest`, `rewrite`, `ask` alongside `block`
- Standard profile: `git push --force` rewrites to `--force-with-lease`, `chmod 777` suggests `755`
- PyPI README now includes full documentation (action types, presets, DSL operators, integrations)

## [0.3.0] - 2026-06-25

### Added
- **PyPI README**: comprehensive documentation visible on pypi.org/project/savants-guard
- Fixed GitHub repo URLs (savants-dev → savants)
- Fixed Cloudflare AI bot protection blocking docs access

## [0.2.0] - 2026-06-25

### Added
- **Soft actions**: `suggest`, `rewrite`, `ask` in addition to `block`
- **Session memory**: user approvals remembered for 8 hours
- **Cooloff**: rules escalate from block → ask after 3 blocks in 5 minutes
- **Pause/resume**: `savants guard off 10m`, `savants guard on`, `SAVANTS_GUARD=off`
- **Disable rules**: `savants guard disable 3` (by number or keyword)
- **Guard status**: `savants guard status` shows active/paused/inactive
- **Secret detection**: content-level scanning for API key patterns (sk_live_, ghp_, AKIA)
- **GuardResult.suggestion**: carries message from suggest/rewrite/ask actions

## [0.1.0] - 2026-06-20

### Added
- Initial release of savants-guard Python SDK
- DSL parser with 16 operators (eq, neq, gt, contains, starts_with, etc.)
- `create_guard()`, `guard.check()`, `guard.wrap()` decorator
- Presets: `production_safety()`, `spend_limit()`, `business_hours()`, `deploy_safety()`
- Managed mode with cloud rule sync and event batching
- 9 composable guard profiles (minimal, standard, paranoid, secrets, git-safe, infra-safe, publish-safe, k8s-safe, k8s-secrets)
- Claude Code PreToolUse hook integration
- Auto-install via `curl -fsSL savants.sh | sh`
