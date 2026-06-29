# Savants

Execution-layer guardrails for AI agents. Block dangerous actions, enforce spend limits, protect production.

```
curl -fsSL savants.sh | sh
```

## What it does

AI agents (Claude Code, Cursor, LangChain, CrewAI) can run commands, edit files, call APIs. Savants enforces rules on **what they do**, not what they say.

```bash
# Guard your IDE — one command, 30 seconds
savants guard preset standard

# Or guard your agent code — Python
pip install savants-guard

# Or TypeScript
npm install @savants.dev/guard
```

## Guard DSL

Plain English rules that block, suggest alternatives, rewrite commands, or escalate to the user:

```
when command contains 'rm -rf /' then block
when command contains 'git push --force' then rewrite 'git push --force-with-lease'
when command contains 'chmod 777' then suggest 'Use chmod 755 for directories or 644 for files'
when command contains 'npm publish' then ask 'Publishing to npm is public and permanent'
when file_path contains '.env' then ask 'Verify .env is in .gitignore before proceeding'
```

## Guard profiles

Composable presets you can stack:

```bash
savants guard preset minimal              # 10 rules — catastrophic only
savants guard preset standard             # 25 rules — recommended
savants guard preset standard+secrets     # + credential protection
savants guard preset standard+k8s-safe    # + Kubernetes protection
savants guard profiles                    # see all available
```

## Python SDK

```python
from savants_guard import create_guard

guard = create_guard([
    "when action contains 'delete' and env eq 'production' then block",
    "when spend gt 100 then require_approval",
])

result = guard.check({"action": "delete_database", "env": "production"})
print(result.blocked)  # True
print(result.rule)     # "when action contains 'delete'..."
```

Works with any LLM framework:

```python
# Anthropic SDK
result = guard.check({"action": tool_name, "tool": tool_name, **tool_input})
if result.blocked:
    return f"Blocked: {result.rule}"

# LangChain / CrewAI / custom agents
@guard.wrap
def dangerous_action(action, env="production"):
    ...  # raises GuardError if blocked
```

## TypeScript SDK

```typescript
import { createGuard } from '@savants.dev/guard';

const guard = createGuard([
  "when action contains 'delete' and env eq 'production' then block",
]);

const result = guard.check({ action: "delete_database", env: "production" });
console.log(result.blocked); // true
```

## How it works

**IDE guard (Claude Code / Cursor / Windsurf):**

The installer registers a `PreToolUse` hook that intercepts every tool call before it executes. Rules are evaluated locally in ~1ms. No network calls. No cloud required.

**Agent code guard (Python / TypeScript):**

The SDK evaluates rules in-process. Zero latency. Import, create a guard, check actions. Rules are deterministic -- same input always gives same output.

## Action types

| Action | Behavior | Agent continues? |
|--------|----------|-----------------|
| `block` | Hard stop | No |
| `suggest 'message'` | Deny + suggestion | Yes, tries alternative |
| `rewrite 'command'` | Silently swap command | Yes, runs safe version |
| `ask 'reason'` | Prompt user to approve | After approval |

## Disable / pause

```bash
savants guard off 10m          # pause for 10 minutes
savants guard off              # pause until re-enabled
savants guard on               # resume
savants guard disable 3        # disable rule #3 only
SAVANTS_GUARD=off claude       # disable for one session
```

## Code intelligence

37 MCP tools for semantic search, error diagnosis, blast radius analysis, and infrastructure monitoring:

```bash
savants up              # index your repo
savants mcp install     # register with your editor
```

## Links

- [Documentation](https://savants.dev/docs/getting-started)
- [Guard profiles](https://savants.dev/docs/guard-dsl)
- [Python SDK on PyPI](https://pypi.org/project/savants-guard/)
- [TypeScript SDK on npm](https://www.npmjs.com/package/@savants.dev/guard)

## License

FSL-1.1-Apache-2.0 (converts to Apache 2.0 after two years)
