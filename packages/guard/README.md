# @savants/guard

**Deterministic guardrails for AI agents. Your agent follows your rules. Always.**

An AI agent deleted a production database in 9 seconds. Another bought $31 of eggs without asking. Guards are code, not prompts — the LLM cannot bypass them.

```
npm install @savants/guard
```

## 30 seconds to your first guardrail

```typescript
import { createGuard, GuardError } from '@savants/guard';

const guard = createGuard([
  "when action contains 'delete' and env eq 'production' then block",
  "when spend gt 100 then require_approval",
]);

// Check any action
guard.check({ action: 'delete_db', env: 'production' });
// → { blocked: true, rule: "when action contains 'delete'...", action: "block" }

guard.check({ action: 'read_logs', env: 'production' });
// → { allowed: true, blocked: false }
```

## Wrap any function

```typescript
const dangerousDelete = async (params) => {
  await db.drop(params.table);
};

const safeDelete = guard.wrap(dangerousDelete);

await safeDelete({ action: 'delete_table', env: 'staging' });   // ✓ executes
await safeDelete({ action: 'delete_table', env: 'production' }); // ✗ throws GuardError
```

## Works with Vercel AI SDK

```typescript
import { generateText, tool } from 'ai';
import { createGuard } from '@savants/guard';

const guard = createGuard([
  "when action contains 'delete' and env eq 'production' then block",
]);

const tools = guard.wrapTools({
  deleteUser: tool({
    description: 'Delete a user',
    parameters: z.object({ userId: z.string(), env: z.string() }),
    execute: async ({ userId }) => db.users.delete(userId),
  }),
});

// The LLM calls deleteUser with env: 'production'
// → GuardError thrown BEFORE execute() runs
// → db.users.delete() never executes
// → user is safe
```

## Presets — one line, instant protection

```typescript
import { productionSafety, spendLimit, deploySafety } from '@savants/guard';

// Block delete/terminate/drop/remove in production
const guard = productionSafety();

// Block spending over $50
const budget = spendLimit(50);

// No risky Friday deploys
const deploy = deploySafety();
```

## Write rules in plain English

```
when action contains 'delete' and env eq 'production' then block
when spend gt 100 then require_approval
when day_of_week eq 'Friday' and risk_score gt 0.7 then block_deploy
when restart_count gt 3 and namespace eq 'production' then alert
when tool starts_with 'send_' then require_approval
```

### All 16 operators

`eq` `neq` `gt` `gte` `lt` `lte` `contains` `not_contains` `starts_with` `ends_with` `matches` `in` `not_in` `is_true` `is_false` `is_empty`

Combinators: `and` `or`

## Why not just Zod?

Zod validates **shape** ("is this a string?"). Guards validate **safety** ("should this action execute?").

```typescript
// Zod: { action: "DROP TABLE users" } → ✓ valid string
// Guard: { action: "DROP TABLE users", env: "production" } → ✗ blocked
```

| | Zod | @savants/guard |
|---|---|---|
| Validates data types | ✓ | — |
| Blocks destructive actions | — | ✓ |
| Tracks state across calls | — | ✓ |
| Human-readable rules | — | ✓ |
| Audit log | — | ✓ |
| No eval(), no code injection | ✓ | ✓ |

**Use both.** Zod for shape. Guards for safety.

## How it works (mathematically)

Guards are evaluated by a closed AST engine with 7 formal guarantees:

1. **Totality** — evaluation always terminates (no loops, structural recursion)
2. **Purity** — no side effects (no fetch, no require, no global state)
3. **Mandatory** — guards always execute when present (no skip path in code)
4. **Isolation** — the LLM cannot modify guards (read-only in transition path)
5. **Closure** — 16 operators, fixed at load time, no runtime injection
6. **Completeness** — every transition path checks guards
7. **Monotonicity** — adding a guard only restricts, never expands behavior

This is not "we tested 102 cases." The code structure makes bypass impossible.

## License

MIT
