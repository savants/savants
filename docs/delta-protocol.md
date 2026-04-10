# Graph Delta Protocol

**Status:** Specification v0.1
**Implementation:** `src/savants/delta/schema.py`

The Delta Protocol is the wire format for representing graph mutations. It is used for:

1. **Local working delta** — what the user has changed in their working copy
2. **Branch overlay** — what a feature branch has changed from main
3. **Incremental indexing** — the diff applied when re-indexing a changed file

A delta is a JSON object describing graph mutations. It is intentionally **declarative** (state to apply, not commands to execute) so that composition is associative and idempotent.

## Top-level structure

```json
{
  "version": "0.1",
  "schema_id": "savants/delta/v0.1",
  "scope": {
    "org": "acme",
    "repo": "backend",
    "branch": "alice/refactor-auth",
    "base_sha": "abc123def456",
    "head_sha": "def789abc012"
  },
  "provenance": {
    "author": "alice@acme.com",
    "timestamp": "2026-04-07T15:30:00Z",
    "session_id": "a1b2c3"
  },
  "operations": [
    { "op": "add_node", ... },
    { "op": "remove_node", ... },
    { "op": "update_node", ... },
    { "op": "add_edge", ... },
    { "op": "remove_edge", ... }
  ]
}
```

### Required fields

| Field | Type | Description |
|---|---|---|
| `version` | string | Protocol version. Currently `"0.1"`. |
| `schema_id` | string | URI identifying the delta schema. |
| `scope` | object | Identifies which org/repo/branch/SHAs the delta applies to. |
| `operations` | array | List of mutations to apply, in order. |

### Optional fields

| Field | Type | Description |
|---|---|---|
| `provenance` | object | Author, timestamp, session ID for audit. |
| `compressed` | bool | If `true`, the `operations` field is base64-encoded zstd-compressed. |
| `signature` | string | Optional cryptographic signature for integrity verification. |

## Operations

### `add_node`

Add a new node to the graph.

```json
{
  "op": "add_node",
  "id": "fn:src/auth/jwt.py:authenticate",
  "label": "Function",
  "properties": {
    "name": "authenticate",
    "file_path": "src/auth/jwt.py",
    "start_line": 10,
    "end_line": 25,
    "parameters": ["token"],
    "return_type": "bool"
  }
}
```

**Required:** `op`, `id`, `label`
**Optional:** `properties` (key-value pairs to set)

The `id` is a stable identifier the client computes deterministically from `(label, file_path, name)`. Multiple deltas referring to the same `id` refer to the same node.

### `remove_node`

Remove a node and all its incident edges.

```json
{
  "op": "remove_node",
  "id": "fn:src/auth/jwt.py:authenticate"
}
```

**Required:** `op`, `id`

When applied during composition, this masks any node with matching `id` from lower layers and prevents edges referencing it from being included.

### `update_node`

Update properties of an existing node.

```json
{
  "op": "update_node",
  "id": "fn:src/auth/jwt.py:verify_session",
  "set": {
    "parameters": ["token", "strict"],
    "end_line": 30
  },
  "unset": ["return_type"]
}
```

**Required:** `op`, `id`
**Optional:** `set` (properties to set), `unset` (properties to remove)

`update_node` is a convenience equivalent to `remove_node` + `add_node` with merged properties. The composition engine merges properties from lower layers with the `set` overrides.

### `add_edge`

Add a directed edge between two nodes.

```json
{
  "op": "add_edge",
  "id": "edge:src/api/users.py:get_user→src/auth/jwt.py:authenticate:CALLS",
  "type": "CALLS",
  "from_id": "fn:src/api/users.py:get_user",
  "to_id": "fn:src/auth/jwt.py:authenticate",
  "properties": {
    "line": 15
  }
}
```

**Required:** `op`, `id`, `type`, `from_id`, `to_id`
**Optional:** `properties`

### `remove_edge`

Remove an edge between two nodes.

```json
{
  "op": "remove_edge",
  "id": "edge:src/api/users.py:get_user→src/auth/jwt.py:authenticate:CALLS"
}
```

**Required:** `op`, `id`

## Node label canonical types

| Label | What it represents | Required properties |
|---|---|---|
| `File` | A source file | `path`, `language`, `line_count`, `sha256` |
| `Function` | A function or method | `name`, `file_path`, `start_line`, `end_line` |
| `Class` | A class definition | `name`, `file_path`, `start_line`, `end_line` |
| `Module` | A logical module/package | `name` |
| `Variable` | A module-level variable | `name`, `file_path` |
| `Episode` | An episodic memory entry | `content`, `source_type`, `timestamp` |
| `Entity` | An abstract entity for episodic facts | `name` |

## Edge type canonical types

| Type | From → To | Meaning |
|---|---|---|
| `CONTAINS` | File → Function/Class/Variable | File defines this symbol |
| `CALLS` | Function → Function | Caller invokes callee |
| `INHERITS_FROM` | Class → Class | Subclass relationship |
| `IMPORTS` | File → File/Module | Import dependency |
| `DEPENDS_ON` | Module → Module | Module-level dependency |
| `DEFINES` | Class → Function | Class defines method |
| `REFERENCES` | Function → Variable | Function reads/writes variable |
| `MENTIONS` | Episode → Entity | Episode references entity |
| `FACT` | Entity → Entity | Temporal fact (with valid_from/valid_to) |

## ID format

Node IDs are deterministic and human-readable. The format is:

```
{label_short}:{file_path}:{name}
```

Where `label_short` is one of: `f` (File), `fn` (Function), `c` (Class), `m` (Module), `v` (Variable), `ep` (Episode), `e` (Entity).

Examples:
- `f:src/auth/jwt.py` — the file
- `fn:src/auth/jwt.py:authenticate` — the function
- `c:src/models.py:User` — the class
- `e:JWT` — an entity

Edge IDs are computed from their endpoints + type:

```
edge:{from_id}→{to_id}:{type}
```

Examples:
- `edge:fn:src/api/users.py:get_user→fn:src/auth/jwt.py:authenticate:CALLS`

These IDs are stable across runs because they're derived from path + name. If a function is renamed, its ID changes — which is the correct behavior (the old ID is removed, the new ID is added).

## Composition algorithm

Pseudocode for composing layers `[base, overlay, delta]` into a single in-memory graph:

```python
def compose(layers: list[Delta]) -> Graph:
    nodes = {}  # id → properties
    edges = {}  # id → (from, to, type, properties)
    removed_nodes = set()
    removed_edges = set()

    for layer in layers:
        for op in layer.operations:
            if op.op == "add_node":
                if op.id in removed_nodes:
                    continue  # earlier removal wins... no wait, later wins
                nodes[op.id] = (op.label, dict(op.properties))
            elif op.op == "remove_node":
                removed_nodes.add(op.id)
                nodes.pop(op.id, None)
                # Cascade-remove edges
                for eid in list(edges):
                    if op.id in edges[eid][:2]:
                        edges.pop(eid)
                        removed_edges.add(eid)
            elif op.op == "update_node":
                if op.id in nodes:
                    label, props = nodes[op.id]
                    props.update(op.set or {})
                    for k in (op.unset or []):
                        props.pop(k, None)
                    nodes[op.id] = (label, props)
            elif op.op == "add_edge":
                if op.id in removed_edges:
                    continue
                if op.from_id in nodes and op.to_id in nodes:
                    edges[op.id] = (op.from_id, op.to_id, op.type, dict(op.properties or {}))
            elif op.op == "remove_edge":
                removed_edges.add(op.id)
                edges.pop(op.id, None)

    return Graph(nodes=nodes, edges=edges)
```

**Order matters within a layer** (later operations override earlier ones), but **layers are applied in order** (base → overlay → delta), with later layers overriding earlier ones.

This is associative for non-conflicting operations and allows safe parallel composition of independent overlays.

## Wire format

### JSON (default)

Plain UTF-8 JSON, no compression. Used for small deltas (<100 KB).

### Compressed JSON

For larger deltas, the `operations` array is serialized to JSON, compressed with zstd level 3, and base64-encoded:

```json
{
  "version": "0.1",
  "compressed": true,
  "scope": { ... },
  "operations_compressed": "KLUv/QBYqQQAYWJjZA=="
}
```

Compression typically achieves 5-15x reduction on graph deltas due to repeated structural patterns.

### Binary protocol (future)

For maximum efficiency, a binary protocol using FlatBuffers or Cap'n Proto is planned for v1.0. Not implemented yet.

## Authenticated deltas

For team mode where deltas may pass through untrusted networks, deltas can be signed:

```json
{
  "version": "0.1",
  "scope": { ... },
  "operations": [ ... ],
  "signature": {
    "alg": "ed25519",
    "key_id": "alice@acme.com:key1",
    "signature": "base64-encoded-signature"
  }
}
```

The signature covers the SHA-256 hash of `(scope || operations)` serialized canonically.

## Examples

### Example 1: Adding a new function

Alice adds a new function `verify_session` to `src/auth/jwt.py`:

```json
{
  "version": "0.1",
  "schema_id": "savants/delta/v0.1",
  "scope": {
    "org": "acme",
    "repo": "backend",
    "branch": "alice/refactor-auth"
  },
  "operations": [
    {
      "op": "add_node",
      "id": "fn:src/auth/jwt.py:verify_session",
      "label": "Function",
      "properties": {
        "name": "verify_session",
        "file_path": "src/auth/jwt.py",
        "start_line": 30,
        "end_line": 45,
        "parameters": ["token"]
      }
    },
    {
      "op": "add_edge",
      "id": "edge:f:src/auth/jwt.py→fn:src/auth/jwt.py:verify_session:CONTAINS",
      "type": "CONTAINS",
      "from_id": "f:src/auth/jwt.py",
      "to_id": "fn:src/auth/jwt.py:verify_session"
    }
  ]
}
```

### Example 2: Renaming a function

Alice renames `authenticate` to `verify_session`:

```json
{
  "version": "0.1",
  "schema_id": "savants/delta/v0.1",
  "scope": { "org": "acme", "repo": "backend", "branch": "alice/refactor-auth" },
  "operations": [
    {
      "op": "remove_node",
      "id": "fn:src/auth/jwt.py:authenticate"
    },
    {
      "op": "add_node",
      "id": "fn:src/auth/jwt.py:verify_session",
      "label": "Function",
      "properties": {
        "name": "verify_session",
        "file_path": "src/auth/jwt.py",
        "start_line": 10,
        "end_line": 25,
        "parameters": ["token"]
      }
    },
    {
      "op": "add_edge",
      "id": "edge:f:src/auth/jwt.py→fn:src/auth/jwt.py:verify_session:CONTAINS",
      "type": "CONTAINS",
      "from_id": "f:src/auth/jwt.py",
      "to_id": "fn:src/auth/jwt.py:verify_session"
    }
  ]
}
```

The composition engine handles the cascade: removing `authenticate` cascades to all edges referencing it. Adding `verify_session` and the new CONTAINS edge brings the new structure in. Existing CALLS edges from `get_user` etc. would have referenced `fn:src/auth/jwt.py:authenticate` — they were cascaded away. The user's working copy will need to add new CALLS edges for any callers they updated.

### Example 3: Massive refactor (1000 files changed)

A delta this large would normally be ~1-5 MB compressed. The structure is the same, just many more operations. Compression handles it. The cloud applies it in memory in <1 second.

## Validation rules

A delta is **valid** if:

1. `version`, `schema_id`, `scope`, and `operations` are present
2. Every node ID is unique within the delta (a single delta cannot both add and remove the same node)
3. Every edge `from_id` and `to_id` references a node that exists in either the delta or in the lower layers (otherwise the edge is silently dropped during composition)
4. All required fields for each operation type are present
5. Property values are JSON primitives or arrays of primitives (no nested objects in properties — keep it simple for FalkorDB)
6. Total uncompressed size < 50 MB (sanity limit)

## Versioning

- **0.x**: experimental, can change without notice
- **1.0**: stable, backward-compatible additions only
- **Major versions**: rare, indicate incompatible changes
- **Minor versions**: backward-compatible field additions

Clients send their `version` in the request; the server returns `415 Unsupported Media Type` if it cannot parse the version.

## Implementation status

| Component | Status |
|---|---|
| Python data classes (`src/savants/delta/schema.py`) | ✅ Implemented |
| Local delta computer (`src/savants/delta/computer.py`) | ✅ PoC implemented |
| Composition engine | ❌ Not implemented |
| Compression support | ❌ Not implemented |
| Signature support | ❌ Not implemented |
| Binary protocol (v1.0) | ❌ Not implemented |
| Server-side validation | ❌ Not implemented (no server yet) |
