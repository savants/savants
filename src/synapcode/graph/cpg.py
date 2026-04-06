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
    FileNode,
    FunctionNode,
    create_class_query,
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


def _sha256(content: str) -> str:
    return hashlib.sha256(content.encode()).hexdigest()


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

    def parse_file(self, file_path: Path) -> dict[str, Any]:
        """Parse a single file and extract structural elements."""
        ext = file_path.suffix
        language = SUPPORTED_EXTENSIONS.get(ext)
        if not language:
            return {"functions": [], "classes": [], "imports": []}

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
        }

        parser = self._get_parser(language)
        if parser is None:
            return result

        tree = parser.parse(content.encode())
        self._extract_nodes(tree.root_node, rel_path, result, content.encode())
        return result

    def _extract_nodes(
        self, node: Any, file_path: str, result: dict, source: bytes
    ) -> None:
        """Recursively walk the AST and extract functions, classes, imports."""
        if node.type in ("function_definition", "function_declaration", "method_definition"):
            name_node = node.child_by_field_name("name")
            name = name_node.text.decode() if name_node else "<anonymous>"

            params = []
            params_node = node.child_by_field_name("parameters")
            if params_node:
                for child in params_node.children:
                    if child.type in ("identifier", "typed_parameter", "typed_default_parameter"):
                        param_name = child.child_by_field_name("name")
                        params.append(
                            param_name.text.decode() if param_name else child.text.decode()
                        )

            result["functions"].append(
                FunctionNode(
                    name=name,
                    file_path=file_path,
                    start_line=node.start_point[0] + 1,
                    end_line=node.end_point[0] + 1,
                    parameters=params,
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

        elif node.type == "call":
            fn_node = node.child_by_field_name("function")
            if fn_node:
                result["calls"].append(
                    {
                        "caller_file": file_path,
                        "callee_name": fn_node.text.decode(),
                        "line": node.start_point[0] + 1,
                    }
                )

        for child in node.children:
            self._extract_nodes(child, file_path, result, source)

    def build(self) -> dict[str, int]:
        """Build the full Code Property Graph. Returns counts of nodes created."""
        self.client.ensure_schema()
        files = self.discover_files()
        logger.info("Discovered %d source files in %s", len(files), self.repo_path)

        stats = {"files": 0, "functions": 0, "classes": 0, "edges": 0}

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

            # Create CALLS edges
            for call in parsed["calls"]:
                cypher, params = create_edge_query(
                    "File", "path", call["caller_file"],
                    "Function", "name", call["callee_name"],
                    "CALLS",
                )
                try:
                    self.client.query(cypher, params)
                    stats["edges"] += 1
                except Exception:
                    pass  # Target function may not exist in the graph yet

        logger.info("CPG build complete: %s", stats)
        return stats

    def build_incremental(self, changed_files: list[str], deleted_files: list[str]) -> dict[str, int]:
        """Incrementally update the graph based on a git diff.

        Only re-parses changed files and removes nodes for deleted files.
        """
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

                stats["updated"] += 1
            except Exception as e:
                logger.warning("Failed to update %s: %s", rel_path, e)

        logger.info("Incremental update complete: %s", stats)
        return stats
