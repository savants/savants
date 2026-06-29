# savants-guard

Deterministic guardrails for AI agents. Block dangerous actions, suggest alternatives, rewrite commands, enforce spend limits.

```
pip install savants-guard
```

## Quick start

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

## Action types

Rules end with an action. Four actions available, from soft to hard:

```python
guard = create_guard([
    "when command contains 'chmod 777' then suggest 'Use chmod 755 for directories'",
    "when command contains 'git push --force' then rewrite 'git push --force-with-lease'",
    "when command contains 'npm publish' then ask 'Publishing is permanent'",
    "when command contains 'rm -rf /' then block",
])
```

| Action | `result.blocked` | `result.allowed` | `result.suggestion` |
|--------|-------------------|-------------------|---------------------|
| `suggest 'msg'` | `False` | `False` | The alternative suggestion |
| `rewrite 'cmd'` | `False` | `False` | The replacement command |
| `ask 'reason'` | `False` | `False` | The reason for approval |
| `block` | `True` | `False` | `None` |
| *(no match)* | `False` | `True` | `None` |

## GuardResult fields

`guard.check()` returns a `GuardResult` with:

- **`blocked`** — `True` for `block` and `require_approval`
- **`allowed`** — `True` only when no rule matched
- **`action`** — `"block"`, `"suggest"`, `"rewrite"`, `"ask"`, `"require_approval"`, or `None`
- **`suggestion`** — message from `suggest`, replacement from `rewrite`, or reason from `ask`
- **`rule`** — the DSL rule that matched, or `None`
- **`context`** — the context dict you passed in

## Presets

```python
from savants_guard import production_safety, spend_limit, business_hours, deploy_safety

guard = production_safety()       # blocks delete/terminate/drop in production
guard = spend_limit(100)          # blocks amount/spend/cost over 100
guard = business_hours()          # blocks actions on Saturday/Sunday
guard = deploy_safety()           # blocks risky Friday deploys
```

## Wrap decorator

Protect functions with `@guard.wrap` — raises `GuardError` when blocked:

```python
from savants_guard import create_guard, GuardError

guard = create_guard(["when action contains 'delete' then block"])

@guard.wrap
def dangerous_action(**kwargs):
    return "executed"

try:
    dangerous_action(action="delete_db")
except GuardError as e:
    print(e.rule)          # "when action contains 'delete' then block"
    print(e.guard_action)  # "block"
```

## Runtime rule management

```python
guard = create_guard([])

guard.add_rule("when action contains 'delete' then block")
print(guard.list_rules())  # ["when action contains 'delete' then block"]

guard.check({"action": "delete"})
guard.check({"action": "read"})
print(guard.get_log())     # [{timestamp, context, result}, ...]
```

## Rule evaluation

**First match wins.** Rules evaluate in order. Put softer rules before harder ones:

```python
guard = create_guard([
    "when action eq 'deploy' then suggest 'Use staging first'",  # fires first
    "when action eq 'deploy' then block",                        # never reached
])
```

## DSL operators

`eq`, `neq`, `gt`, `gte`, `lt`, `lte`, `contains`, `not_contains`, `starts_with`, `ends_with`, `matches`, `in`, `not_in`, `is_true`, `is_false`, `is_empty`, `is_not_empty`

Combine with `and` / `or`:

```python
"when action contains 'delete' and env eq 'production' then block"
"when env eq 'staging' or env eq 'development' then allow"
```

## Framework integrations

Same guard, any framework. Install the framework, import the integration:

### LangChain

```python
from savants_guard import create_guard
from savants_guard.integrations import langchain_callback

guard = create_guard(["when action contains 'delete' then block"])
handler = langchain_callback(guard)

# Add to any LangChain agent
agent.invoke(input, config={"callbacks": [handler]})
```

### CrewAI

```python
from savants_guard import create_guard
from savants_guard.integrations import crewai_hook

guard = create_guard(["when action contains 'delete' then block"])
crewai_hook(guard)  # registers globally as @before_tool_call
```

### OpenAI Agents SDK

```python
from savants_guard import create_guard
from savants_guard.integrations import openai_tool_guardrail

guard = create_guard(["when action contains 'delete' then block"])
guardrail = openai_tool_guardrail(guard)

@function_tool(tool_input_guardrails=[guardrail])
def delete_user(user_id: str) -> str:
    ...
```

No framework dependency required. Integrations import the framework only when called. If the framework isn't installed, you get a clear error message.

## Links

- [Documentation](https://savants.dev/docs/getting-started)
- [GitHub](https://github.com/savants/savants)
- [PyPI](https://pypi.org/project/savants-guard/)

## License

MIT
