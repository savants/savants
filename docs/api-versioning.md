# Mazkir API Versioning Contract

**Status:** Settled, decided 2026-04-08. Stolen from Kubernetes.

This document defines how Mazkir's MCP tool surface evolves over
time without breaking customers who built integrations against
older versions. This contract is **non-negotiable** because it is
the precondition for the standards-capture moat described in
`docs/competitive-defense.md`. Without versioning, every Mazkir
update breaks every customer integration. With it, customers pin
their version and upgrade on their schedule.

If we get this wrong, we will spend our entire engineering capacity
on customer-support tickets about "you broke my Claude Code
integration" instead of building features. K8s won the orchestration
war partly because the API surface stayed compatible across versions
for years. Mazkir adopts the same playbook.

---

## The contract in one paragraph

Every MCP tool call accepts an optional `api_version` parameter
formatted as `YYYY-MM-DD`. The Mazkir server maintains backwards
compatibility for at least **4 quarterly releases (~1 year)** of
older API versions simultaneously. **Schema additions are
non-breaking** within a version (new fields can be added without
bumping the version). **Schema removals or renames require a new
version** and the old version keeps working for the full
deprecation period. **Deprecation is announced via warnings**, not
breakage — old API versions return a `_deprecation_notice` field
in every response telling the caller when the version will be
removed and how to migrate.

Stolen verbatim from `kubectl` and `kubernetes/api`.

---

## Version naming

Versions are dates: `2026-04-08`, `2026-07-15`, `2026-10-22`.

Why dates instead of semver:
- Dates are unambiguous and totally-ordered
- They convey "when this version was frozen"
- They make deprecation timelines obvious ("this is from 2 years
  ago, of course you should upgrade")
- They avoid the bikeshedding around what counts as a major vs
  minor change (which K8s used to fight about)

This is the same convention used by Stripe, AWS, and several other
serious API platforms.

---

## How customers pin a version

Every MCP tool accepts an optional `api_version` argument:

```python
{
  "method": "tools/call",
  "params": {
    "name": "function_xray",
    "arguments": {
      "function_name": "payment_handler",
      "api_version": "2026-04-08"
    }
  }
}
```

If `api_version` is omitted, the server uses the **latest stable**
version. Customers who care about stability pin explicitly. Tools
generated automatically from MCP discovery (Claude Code's tool
registration) include the `api_version` parameter in the schema.

For convenience, customers can also pin at the connection level via
an environment variable read by the MCP server on startup:

```
MAZKIR_API_VERSION=2026-04-08
```

This makes every tool call use that version unless explicitly
overridden in the args.

---

## What "backwards compatible" means concretely

These changes are **always allowed** within a version (no version
bump required):

1. **Adding a new field to a response object.** New field is
   present in the new version, absent in the old (gracefully
   ignored by old clients).
2. **Adding a new optional input parameter.** Old clients don't
   pass it, server uses the documented default.
3. **Adding a new MCP tool.** Old clients don't know about it, new
   clients can use it.
4. **Adding a new value to an open-ended enum.** Clients should
   handle unknown values gracefully (forward compatibility on the
   client side).
5. **Performance improvements with identical observable behavior.**
6. **Adding new examples or documentation.**
7. **Internal refactors that don't change wire format.**

These changes **require a new API version**:

1. **Removing or renaming any field in a response.**
2. **Removing or renaming any tool.**
3. **Removing or renaming any input parameter.**
4. **Changing the type of a field** (e.g., string → int, scalar →
   array).
5. **Changing the meaning of a field** (e.g., "duration in seconds"
   → "duration in milliseconds").
6. **Tightening validation** on an input (e.g., "now requires
   non-empty" when it used to accept empty).
7. **Removing a value from a closed enum.**
8. **Changing default behavior** of an optional parameter.

When in doubt: **bump the version**. The cost of an extra version
is one more entry in the deprecation registry; the cost of breaking
compatibility within a version is angry customers and a damaged
trust relationship.

---

## Deprecation policy

When a new version supersedes an older one:

1. The old version **continues to work** for at least **4 quarterly
   releases** (~12 months) after the new version is published.
2. Every response from the old version includes a
   `_deprecation_notice` field:

```json
{
  "result": { ... actual response ... },
  "_deprecation_notice": {
    "current_version": "2025-04-15",
    "deprecated_in": "2025-07-22",
    "will_be_removed_on": "2026-07-22",
    "migration_guide": "https://docs.mazkir.io/migration/2025-07-22",
    "breaking_changes": [
      "Field 'callers' renamed to 'direct_callers'",
      "Field 'callees' renamed to 'direct_callees'",
      "New required field: 'depth' (defaults to 1 if not provided)"
    ]
  }
}
```

3. The server logs every old-version request with the org_id and
   IP source. We use this to **proactively reach out** to customers
   on old versions before the removal date.
4. The migration guide explains how to update each affected call
   site, with copy-paste-able before/after examples.
5. **Removal happens on the announced date, not earlier.** Even if
   the old version is "broken" or "ugly" by the team's standards,
   removing it before the deprecation date is a contract violation.

---

## Multi-version coexistence

The Mazkir server holds **all supported versions in memory
simultaneously**. The router for incoming MCP calls inspects the
`api_version` header and routes to the appropriate handler chain.

Implementation pattern:

```python
class MazkirMCPServer:
    def __init__(self):
        self.tools_by_version = {
            "2025-04-15": ToolsV1.build(),
            "2025-07-22": ToolsV2.build(),
            "2025-10-30": ToolsV3.build(),
            "2026-01-15": ToolsV4.build(),
            "2026-04-08": ToolsV5.build(),  # current latest
        }
        self.latest = "2026-04-08"
        self.deprecated_after = {
            "2025-04-15": "2026-04-15",  # ~12 months after 2025-04-15
            "2025-07-22": "2026-07-22",
            # ...
        }

    def call_tool(self, tool_name, args, requested_version=None):
        version = requested_version or self.latest
        if version not in self.tools_by_version:
            return error("Unknown api_version. Supported: ...")

        tools = self.tools_by_version[version]
        result = tools[tool_name].call(args)

        # Attach deprecation notice if applicable
        if version != self.latest:
            removal_date = self.deprecated_after[version]
            result["_deprecation_notice"] = {
                "current_version": version,
                "latest_version": self.latest,
                "will_be_removed_on": removal_date,
                # ...
            }
        return result
```

Each version's `Tools` class is implemented in its own module
(`src/mazkir/api/v_2025_04_15/`, etc.) so that updates to one
version's logic can't accidentally affect another. Old version
modules become read-only after publication.

---

## How versions get added

When the team wants to make a breaking change:

1. **Open an RFC** with the proposed change, the breaking impact,
   and the migration path.
2. **Create the new version module** alongside the existing ones.
   Most code is copy-pasted from the previous version with only the
   changed pieces edited.
3. **Update the migration guide** with before/after examples for
   every affected tool.
4. **Test against a synthetic customer** that uses both old and new
   versions in parallel. Both must work.
5. **Publish the new version** as the new `latest` and announce in
   the changelog.
6. **Update the deprecation timer** for the previous version (~12
   months from today).
7. **Email customers** who are on the now-deprecated version
   (identifiable from server logs).

This is a slow, deliberate process. **That's the point.** Breaking
changes should be expensive enough that we think twice before
making them.

---

## Versioning across the live infrastructure layer

The MCP tool surface for K8s + AWS + GCP + Azure is the **most
volatile** part of the API because cloud providers add new resource
types constantly. The versioning contract applies *especially*
strictly here.

When AWS adds a new service (say, `aws-quantum-thing-2027`),
Mazkir's response is:

1. Add support for the new resource type as a non-breaking field
   addition in the **current** API version. Customers using the
   current version see the new field automatically; customers on
   older versions don't see it (and that's fine — they pinned to
   an older version for stability, not for forward compatibility).
2. If supporting the new type requires breaking schema changes
   (e.g., a new required field), that goes in the **next** API
   version on the next quarterly release.
3. Old API versions never get the new resource type. Customers who
   want the new type upgrade.

This is exactly how K8s handles new resource kinds: `apps/v1` only
ever knows about the kinds it knew about at v1 release; new kinds
go into `apps/v2`.

---

## Why this matters for the moat

Per `docs/competitive-defense.md`, the strongest possible moat for
Mazkir is **standards capture** — making the MCP tool surface the
expected interface for code intelligence, the way Stripe's API
became the expected interface for payments. Standards capture
requires:

1. A stable contract customers can build on
2. Backwards compatibility customers can trust
3. A predictable evolution path that doesn't punish early adopters

API versioning is the precondition for all three. Without it,
"standards capture" is empty marketing. With it, the contract is
real and customers can depend on it for years.

This is the **least visible** but **most important** piece of the
Mazkir moat strategy. The versioning contract is what lets the
1000th customer trust us as much as the first.

---

## When to break the contract (almost never)

There is exactly one situation where breaking the contract is
acceptable: **a security vulnerability that requires immediate
schema change to fix**. Even then:

1. The fix lands in a new version, and
2. Old versions get a security patch that maintains the old wire
   format but fixes the underlying issue, and
3. We notify customers explicitly via email and the dashboard
   warning banner.

Any other reason to break the contract — "the old code is ugly,"
"nobody's using the old version," "we're tired of maintaining it"
— is wrong and should be rejected at code review time.

---

## How this document gets used

- **For RFC reviews:** any proposal that touches the MCP tool
  surface must explicitly state whether it's backwards compatible
  in the current version or requires a new version
- **For engineering discipline:** the team treats versioning as a
  feature, not an afterthought. New tool definitions are added to
  the latest version module, never directly to "the codebase"
- **For customer trust:** the deprecation timeline is published in
  the docs and we never miss a deprecation date
- **For competitive defense:** the standards-capture moat depends
  on this contract being real, not aspirational. Don't break it.
