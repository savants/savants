"""Code Property Graph builder using tree-sitter for AST parsing.

Parses source files into an AST, extracts functions, classes, imports,
and call relationships, then populates FalkorDB.
"""

from __future__ import annotations

import hashlib
import logging
from pathlib import Path
from typing import Any

from synapcode.graph.client import GraphClient
from synapcode.graph.schema import (
    ClassNode,
    ConfigKeyNode,
    FileNode,
    FunctionNode,
    create_class_query,
    create_config_key_query,
    create_edge_query,
    create_file_query,
    create_function_query,
)

logger = logging.getLogger(__name__)

SUPPORTED_EXTENSIONS = {
    ".py": "python",
    ".js": "javascript",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".jsx": "javascript",
    ".go": "go",
    ".rs": "rust",
    ".java": "java",
}

# Config files — parsed via stdlib (yaml/tomllib/json), not tree-sitter.
# Key paths become ConfigKey nodes so `search_code` can find them by name.
CONFIG_EXTENSIONS = {
    ".yaml": "yaml",
    ".yml": "yaml",
    ".toml": "toml",
    ".json": "json",
}

# Don't index generated / lockfile / vendored config — noise, not signal.
_CONFIG_EXCLUDE_NAMES = {
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "poetry.lock",
    "Cargo.lock",
    "tsconfig.tsbuildinfo",
    ".prettierrc.json",  # most are trivially small
}

_VALUE_MAX_LEN = 200  # truncate long values; we just need substring-searchability
_KEY_PATH_MAX_DEPTH = 8  # don't recurse arbitrarily deep into huge JSON blobs


def _sha256(content: str) -> str:
    return hashlib.sha256(content.encode()).hexdigest()


_IDENT_RE = __import__("re").compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$")


def _looks_like_symbol(s: str) -> bool:
    """True if `s` could plausibly be a Python/JS symbol name or dotted path.

    Filters out empty strings, very long strings (probably not symbols),
    strings with whitespace, and anything that isn't a valid identifier shape.
    The goal is to catch registry keys like "HandleTsCoinTransfer" or
    "handlers.coin.Handle" without capturing every string in the codebase.
    """
    if not s or len(s) > 120 or " " in s or "\n" in s:
        return False
    # Must contain at least one uppercase letter OR a dot — filters out
    # common lowercase words like "utf-8", "get", "post", etc.
    if not any(c.isupper() for c in s) and "." not in s:
        return False
    return bool(_IDENT_RE.match(s))


def _flatten_config(
    node: Any,
    prefix: str,
    file_path: str,
    fmt: str,
    out: list,
    depth: int,
) -> None:
    """Recursively flatten a parsed config tree into dotted-path leaves.

    Dicts become `parent.child`. Lists become `parent[0]`, `parent[1]`, ...
    Leaves (str/int/bool/None) become ConfigKeyNode entries. Long values
    are truncated to _VALUE_MAX_LEN.
    """
    if depth > _KEY_PATH_MAX_DEPTH:
        return

    if isinstance(node, dict):
        for k, v in node.items():
            key = str(k)
            new_prefix = f"{prefix}.{key}" if prefix else key
            _flatten_config(v, new_prefix, file_path, fmt, out, depth + 1)
    elif isinstance(node, list):
        # For list-of-dicts (k8s/docker-compose style), index by position.
        # For list-of-scalars (e.g. [8080, 8443]), join the values as one leaf
        # so we don't explode into 50 meaningless entries.
        if all(not isinstance(x, (dict, list)) for x in node):
            value = ", ".join(str(x) for x in node)[:_VALUE_MAX_LEN]
            if prefix:
                out.append(ConfigKeyNode(name=prefix, file_path=file_path, value=value, format=fmt))
        else:
            for i, item in enumerate(node):
                new_prefix = f"{prefix}[{i}]" if prefix else f"[{i}]"
                _flatten_config(item, new_prefix, file_path, fmt, out, depth + 1)
    else:
        # Leaf: scalar
        if prefix:
            value = "" if node is None else str(node)
            out.append(
                ConfigKeyNode(
                    name=prefix,
                    file_path=file_path,
                    value=value[:_VALUE_MAX_LEN],
                    format=fmt,
                )
            )


def _decorator_name(dec_node: object) -> str:
    """Extract a decorator's callable name, e.g. @workflow.defn -> 'workflow.defn'.

    Handles both bare decorators (@cached) and calls (@app.route("/x")).
    """
    # tree-sitter-python: decorator -> (identifier | attribute | call)
    for child in dec_node.children:  # type: ignore[attr-defined]
        if child.type in ("identifier", "attribute", "dotted_name"):
            return child.text.decode(errors="replace")
        if child.type == "call":
            fn = child.child_by_field_name("function")
            if fn is not None:
                return fn.text.decode(errors="replace")
    # Fallback: strip the leading '@'
    return dec_node.text.decode(errors="replace").lstrip("@").split("(")[0]  # type: ignore[attr-defined]


class CodePropertyGraphBuilder:
    """Builds a Code Property Graph from a repository directory."""

    def __init__(
        self,
        repo_path: str | Path,
        client: GraphClient | None = None,
        commit_sha: str = "",
        author: str = "",
    ):
        self.repo_path = Path(repo_path).resolve()
        self.client = client or GraphClient()
        self.commit_sha = commit_sha
        self.author = author
        self._parsers: dict[str, Any] = {}

    def _get_parser(self, language: str) -> Any:
        """Lazy-load tree-sitter parser for a language."""
        if language not in self._parsers:
            try:
                import tree_sitter
                import importlib

                lang_mod = importlib.import_module(f"tree_sitter_{language}")
                lang = tree_sitter.Language(lang_mod.language())
                parser = tree_sitter.Parser(lang)
                self._parsers[language] = parser
            except (ImportError, AttributeError) as e:
                logger.warning("No tree-sitter parser for %s: %s", language, e)
                return None
        return self._parsers.get(language)

    def discover_files(self) -> list[Path]:
        """Find all supported source files in the repository."""
        files = []
        for ext in SUPPORTED_EXTENSIONS:
            files.extend(self.repo_path.rglob(f"*{ext}"))
        # Exclude common non-source directories
        excluded = {"node_modules", ".git", "__pycache__", "venv", ".venv", "dist", "build"}
        return [f for f in files if not any(part in excluded for part in f.parts)]

    def discover_config_files(self) -> list[Path]:
        """Find YAML/TOML/JSON config files worth indexing."""
        excluded = {"node_modules", ".git", "__pycache__", "venv", ".venv", "dist", "build", "target"}
        files: list[Path] = []
        for ext in CONFIG_EXTENSIONS:
            for f in self.repo_path.rglob(f"*{ext}"):
                if any(part in excluded for part in f.parts):
                    continue
                if f.name in _CONFIG_EXCLUDE_NAMES:
                    continue
                # Skip files > 1 MB — probably data, not config
                try:
                    if f.stat().st_size > 1_000_000:
                        continue
                except OSError:
                    continue
                files.append(f)
        return files

    def parse_config_file(self, file_path: Path) -> list[ConfigKeyNode]:
        """Parse a config file and flatten it into dotted-path ConfigKey nodes."""
        ext = file_path.suffix
        fmt = CONFIG_EXTENSIONS.get(ext)
        if fmt is None:
            return []

        try:
            text = file_path.read_text(errors="replace")
        except OSError:
            return []

        try:
            if fmt == "yaml":
                import yaml
                # safe_load_all for multi-document YAML (k8s manifests)
                docs = list(yaml.safe_load_all(text))
                data: Any = docs[0] if len(docs) == 1 else docs
            elif fmt == "toml":
                import tomllib
                data = tomllib.loads(text)
            elif fmt == "json":
                import json as _json
                data = _json.loads(text)
            else:
                return []
        except Exception as e:
            logger.debug("Could not parse config %s: %s", file_path, e)
            return []

        rel_path = str(file_path.relative_to(self.repo_path))
        keys: list[ConfigKeyNode] = []
        _flatten_config(data, "", rel_path, fmt, keys, depth=0)
        return keys

    def parse_file(self, file_path: Path) -> dict[str, Any]:
        """Parse a single file and extract structural elements."""
        ext = file_path.suffix
        language = SUPPORTED_EXTENSIONS.get(ext)
        if not language:
            return {"file": None, "functions": [], "classes": [], "imports": [], "calls": []}

        content = file_path.read_text(errors="replace")
        rel_path = str(file_path.relative_to(self.repo_path))

        result: dict[str, Any] = {
            "file": FileNode(
                path=rel_path,
                language=language,
                line_count=content.count("\n") + 1,
                sha256=_sha256(content),
                last_commit=self.commit_sha,
            ),
            "functions": [],
            "classes": [],
            "imports": [],
            "calls": [],
            "string_refs": [],  # {caller_file, caller_function, value, line}
        }

        parser = self._get_parser(language)
        if parser is None:
            return result

        tree = parser.parse(content.encode())
        self._extract_nodes(tree.root_node, rel_path, result, content.encode())
        return result

    def _extract_nodes(
        self, node: Any, file_path: str, result: dict, source: bytes,
        enclosing_function: str = "",
    ) -> None:
        """Recursively walk the AST and extract functions, classes, imports."""
        current_function = enclosing_function

        if node.type in ("function_definition", "function_declaration", "method_definition"):
            name_node = node.child_by_field_name("name")
            name = name_node.text.decode() if name_node else "<anonymous>"
            current_function = name

            params = []
            params_node = node.child_by_field_name("parameters")
            if params_node:
                for child in params_node.children:
                    if child.type in ("identifier", "typed_parameter", "typed_default_parameter"):
                        param_name = child.child_by_field_name("name")
                        params.append(
                            param_name.text.decode() if param_name else child.text.decode()
                        )

            # Extract decorators. In tree-sitter-python, a function_definition
            # is wrapped in a `decorated_definition` whose earlier children
            # are `decorator` nodes. For JS/TS, decorators are direct children.
            decorators: list[str] = []
            parent = node.parent
            if parent is not None and parent.type == "decorated_definition":
                for sib in parent.children:
                    if sib.type == "decorator":
                        decorators.append(_decorator_name(sib))
            for child in node.children:
                if child.type == "decorator":
                    decorators.append(_decorator_name(child))

            result["functions"].append(
                FunctionNode(
                    name=name,
                    file_path=file_path,
                    start_line=node.start_point[0] + 1,
                    end_line=node.end_point[0] + 1,
                    parameters=params,
                    decorators=decorators,
                )
            )

        elif node.type in ("class_definition", "class_declaration"):
            name_node = node.child_by_field_name("name")
            name = name_node.text.decode() if name_node else "<anonymous>"
            result["classes"].append(
                ClassNode(
                    name=name,
                    file_path=file_path,
                    start_line=node.start_point[0] + 1,
                    end_line=node.end_point[0] + 1,
                )
            )

        elif node.type in ("import_statement", "import_from_statement"):
            result["imports"].append(node.text.decode())

        elif node.type in ("string", "string_literal"):
            # Capture bare string literals that look like symbol references:
            # either a bare identifier ("HandleTsCoinTransfer") or a dotted
            # path ("pkg.mod.Symbol"). This is how registries, Temporal activity
            # names, Celery tasks, Django URL routes, and config keys are wired.
            raw = node.text.decode(errors="replace")
            stripped = raw.strip("'\"`")
            if _looks_like_symbol(stripped):
                result["string_refs"].append(
                    {
                        "caller_file": file_path,
                        "caller_function": current_function,
                        "value": stripped,
                        "line": node.start_point[0] + 1,
                    }
                )

        elif node.type == "call":
            fn_node = node.child_by_field_name("function")
            if fn_node:
                callee = fn_node.text.decode()
                # For method calls like self.validate(), extract just the method name
                if "." in callee:
                    callee = callee.rsplit(".", 1)[-1]
                result["calls"].append(
                    {
                        "caller_file": file_path,
                        "caller_function": current_function,
                        "callee_name": callee,
                        "line": node.start_point[0] + 1,
                    }
                )

        for child in node.children:
            self._extract_nodes(child, file_path, result, source, current_function)

    def build(self) -> dict[str, int]:
        """Build the full Code Property Graph. Returns counts of nodes created."""
        self.client.ensure_schema()
        files = self.discover_files()
        logger.info("Discovered %d source files in %s", len(files), self.repo_path)

        stats = {"files": 0, "functions": 0, "classes": 0, "edges": 0}
        all_calls: list[dict] = []
        all_string_refs: list[dict] = []

        # Pass 1: Create all nodes and CONTAINS edges
        for file_path in files:
            try:
                parsed = self.parse_file(file_path)
            except Exception as e:
                logger.warning("Failed to parse %s: %s", file_path, e)
                continue

            # Create file node
            file_node = parsed["file"]
            cypher, params = create_file_query(file_node)
            self.client.query(cypher, params)
            stats["files"] += 1

            # Create function nodes + CONTAINS edges
            for fn in parsed["functions"]:
                cypher, params = create_function_query(fn)
                self.client.query(cypher, params)
                stats["functions"] += 1

                cypher, params = create_edge_query(
                    "File", "path", fn.file_path,
                    "Function", "name", fn.name,
                    "CONTAINS",
                )
                self.client.query(cypher, params)
                stats["edges"] += 1

            # Create class nodes + CONTAINS edges
            for cls in parsed["classes"]:
                cypher, params = create_class_query(cls)
                self.client.query(cypher, params)
                stats["classes"] += 1

                cypher, params = create_edge_query(
                    "File", "path", cls.file_path,
                    "Class", "name", cls.name,
                    "CONTAINS",
                )
                self.client.query(cypher, params)
                stats["edges"] += 1

            # Collect calls for pass 2
            all_calls.extend(parsed["calls"])
            all_string_refs.extend(parsed.get("string_refs", []))

        # Pass 2: Create CALLS edges (all function nodes exist now).
        #
        # Disambiguation order — for each call, find the callee target this way:
        #   1. Same-file match: caller's file has a function with the callee name
        #   2. Globally unique: exactly one function in the repo has that name
        #   3. Otherwise: skip (cross-product would explode the graph)
        #
        # This avoids the bug where 6 functions named create_app produce
        # a 6× cross-product of edges per call site.
        callee_index: dict[str, list[str]] = {}  # callee_name -> [file_paths]
        for file_path in [str(f.relative_to(self.repo_path)) for f in files]:
            pass  # populated below
        # Build the index by re-querying the graph for known functions
        idx_result = self.client.query(
            "MATCH (fn:Function) RETURN fn.name, fn.file_path"
        )
        for row in idx_result.result_set:
            name, fpath = row[0], row[1]
            callee_index.setdefault(name, []).append(fpath)

        for call in all_calls:
            caller_fn = call.get("caller_function")
            if not caller_fn:
                continue
            callee = call["callee_name"]
            caller_file = call["caller_file"]

            candidates = callee_index.get(callee, [])
            if not candidates:
                continue  # external function, not indexed

            # 1. Prefer same-file match
            target_file: str | None = None
            if caller_file in candidates:
                target_file = caller_file
            # 2. Globally unique
            elif len(candidates) == 1:
                target_file = candidates[0]
            # 3. Ambiguous — skip this call
            else:
                continue

            cypher = (
                "MATCH (a:Function {name: $caller_name, file_path: $caller_file}) "
                "MATCH (b:Function {name: $callee_name, file_path: $target_file}) "
                "MERGE (a)-[:CALLS]->(b)"
            )
            params = {
                "caller_name": caller_fn,
                "caller_file": caller_file,
                "callee_name": callee,
                "target_file": target_file,
            }
            try:
                self.client.query(cypher, params)
                stats["edges"] += 1
            except Exception:
                pass

        # Pass 3: Create REFERENCES_SYMBOL edges from functions that contain
        # string literals matching a known Function/Class name. This closes the
        # registry-dispatch blind spot (the thing that blinded us on the
        # HandleTsCoinTransfer Temporal question). We reuse the callee_index
        # (Function names) and also query Class names.
        class_index: dict[str, list[str]] = {}
        cls_rows = self.client.query(
            "MATCH (c:Class) RETURN c.name, c.file_path"
        ).result_set
        for row in cls_rows:
            class_index.setdefault(row[0], []).append(row[1])

        resolved = 0
        seen: set[tuple[str, str, str, str]] = set()
        for ref in all_string_refs:
            caller_fn = ref.get("caller_function")
            if not caller_fn:
                continue
            value = ref["value"]
            # Resolve: take the last dotted segment as the terminal symbol
            terminal = value.rsplit(".", 1)[-1]

            target_label: str | None = None
            candidates = callee_index.get(terminal, [])
            if candidates:
                target_label = "Function"
            else:
                candidates = class_index.get(terminal, [])
                if candidates:
                    target_label = "Class"
                else:
                    continue

            # Unique target only — skip ambiguous to avoid cross-product blowup
            if len(candidates) != 1:
                continue
            target_file = candidates[0]
            caller_file = ref["caller_file"]

            key = (caller_fn, caller_file, terminal, target_file)
            if key in seen:
                continue
            seen.add(key)

            cypher = (
                f"MATCH (a:Function {{name: $caller_name, file_path: $caller_file}}) "
                f"MATCH (b:{target_label} {{name: $target_name, file_path: $target_file}}) "
                f"MERGE (a)-[r:REFERENCES_SYMBOL]->(b) "
                f"SET r.via = 'string_literal'"
            )
            try:
                self.client.query(cypher, {
                    "caller_name": caller_fn,
                    "caller_file": caller_file,
                    "target_name": terminal,
                    "target_file": target_file,
                })
                resolved += 1
                stats["edges"] += 1
            except Exception:
                pass
        stats["references_symbol"] = resolved

        # Pass 4: Config files. These aren't code, but questions like "is the
        # Mongo profiler enabled?" or "what's the slow query threshold?" live
        # in YAML/TOML/JSON and SynapCode used to have no visibility into them.
        # We flatten each config into dotted-path ConfigKey nodes and link
        # them to a File node with a CONTAINS edge.
        config_files = self.discover_config_files()
        stats["config_files"] = 0
        stats["config_keys"] = 0
        for cf in config_files:
            try:
                keys = self.parse_config_file(cf)
            except Exception as e:
                logger.debug("Failed to parse config %s: %s", cf, e)
                continue
            if not keys:
                continue

            rel_path = str(cf.relative_to(self.repo_path))
            # File node for the config file itself (so CONTAINS edges have a target)
            file_node = FileNode(
                path=rel_path,
                language=CONFIG_EXTENSIONS[cf.suffix],
                line_count=cf.read_text(errors="replace").count("\n") + 1,
                sha256=_sha256(cf.read_text(errors="replace")),
            )
            cypher, params = create_file_query(file_node)
            self.client.query(cypher, params)
            stats["config_files"] += 1

            for key in keys:
                cypher, params = create_config_key_query(key)
                self.client.query(cypher, params)
                stats["config_keys"] += 1

                # CONTAINS edge: File -> ConfigKey
                self.client.query(
                    "MATCH (f:File {path: $fp}) "
                    "MATCH (k:ConfigKey {file_path: $fp, name: $name}) "
                    "MERGE (f)-[:CONTAINS]->(k)",
                    {"fp": rel_path, "name": key.name},
                )

        logger.info("CPG build complete: %s", stats)
        return stats

    def build_incremental(self, changed_files: list[str], deleted_files: list[str]) -> dict[str, int]:
        """Incrementally update the graph based on a git diff.

        Only re-parses changed files and removes nodes for deleted files.
        Filters out unsupported files and excluded directories.
        """
        excluded_parts = {"node_modules", ".git", "__pycache__", "venv", ".venv", "dist", "build", ".synapcode"}

        def _is_indexable(rel_path: str) -> bool:
            parts = Path(rel_path).parts
            if any(p in excluded_parts for p in parts):
                return False
            return Path(rel_path).suffix in SUPPORTED_EXTENSIONS

        changed_files = [p for p in changed_files if _is_indexable(p)]
        deleted_files = [p for p in deleted_files if _is_indexable(p)]

        stats = {"updated": 0, "deleted": 0}

        # Remove deleted files and their children
        for rel_path in deleted_files:
            self.client.query(
                "MATCH (f:File {path: $path})-[r]->(n) DELETE r, n",
                {"path": rel_path},
            )
            self.client.query(
                "MATCH (f:File {path: $path}) DELETE f",
                {"path": rel_path},
            )
            stats["deleted"] += 1

        # Re-parse changed/added files
        for rel_path in changed_files:
            full_path = self.repo_path / rel_path
            if not full_path.exists():
                continue

            # Remove old nodes for this file first
            self.client.query(
                "MATCH (f:File {path: $path})-[r]->(n) DELETE r, n",
                {"path": rel_path},
            )
            self.client.query(
                "MATCH (f:File {path: $path}) DELETE f",
                {"path": rel_path},
            )

            # Re-parse and insert
            try:
                parsed = self.parse_file(full_path)
                file_node = parsed["file"]
                cypher, params = create_file_query(file_node)
                self.client.query(cypher, params)

                for fn in parsed["functions"]:
                    cypher, params = create_function_query(fn)
                    self.client.query(cypher, params)
                    cypher, params = create_edge_query(
                        "File", "path", fn.file_path,
                        "Function", "name", fn.name,
                        "CONTAINS",
                    )
                    self.client.query(cypher, params)

                for cls in parsed["classes"]:
                    cypher, params = create_class_query(cls)
                    self.client.query(cypher, params)
                    cypher, params = create_edge_query(
                        "File", "path", cls.file_path,
                        "Class", "name", cls.name,
                        "CONTAINS",
                    )
                    self.client.query(cypher, params)

                # Create CALLS edges for the newly-parsed file
                for call in parsed.get("calls", []):
                    if not call.get("caller_function"):
                        continue
                    cypher, params = create_edge_query(
                        "Function", "name", call["caller_function"],
                        "Function", "name", call["callee_name"],
                        "CALLS",
                    )
                    try:
                        self.client.query(cypher, params)
                    except Exception:
                        pass

                stats["updated"] += 1
            except Exception as e:
                logger.warning("Failed to update %s: %s", rel_path, e)

        logger.info("Incremental update complete: %s", stats)
        return stats
