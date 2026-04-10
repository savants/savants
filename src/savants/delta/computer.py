"""Local Delta Computer.

Given a "before" state (the last committed file content) and an "after" state
(the user's working copy), compute a Delta describing exactly what changed at
the graph level.

This runs entirely on the user's machine. The output Delta is what the client
sends to the cloud as part of a query — typically just a few KB even for
substantial refactors.

Usage:
    from savants.delta.computer import compute_file_delta

    delta = compute_file_delta(
        file_path="src/auth/jwt.py",
        before_content=open("/git/HEAD/src/auth/jwt.py").read(),
        after_content=open("/working_copy/src/auth/jwt.py").read(),
        org="acme",
        repo="backend",
        branch="alice/refactor-auth",
    )
    print(delta.stats())
    # {'add_node': 2, 'remove_node': 1, 'add_edge': 3, 'remove_edge': 2, 'total': 8}
"""

from __future__ import annotations

import hashlib
import logging
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from savants.delta.schema import Delta, DeltaScope, Provenance

logger = logging.getLogger(__name__)


# Reuse the parsers from the existing CPG builder
SUPPORTED_LANGUAGES = {
    ".py": "python",
    ".js": "javascript",
    ".ts": "typescript",
    ".tsx": "typescript",
    ".jsx": "javascript",
}


@dataclass
class ParsedFile:
    """Structural representation of a parsed file, used for diffing."""

    file_path: str
    language: str
    sha256: str
    line_count: int
    functions: dict[str, dict[str, Any]] = field(default_factory=dict)  # name -> properties
    classes: dict[str, dict[str, Any]] = field(default_factory=dict)
    imports: list[str] = field(default_factory=list)
    calls: list[dict[str, Any]] = field(default_factory=list)  # caller_function, callee_name, line


def _sha256(content: str) -> str:
    return hashlib.sha256(content.encode()).hexdigest()


def _get_parser(language: str):
    """Lazy-load tree-sitter parser."""
    try:
        import importlib

        import tree_sitter

        lang_mod = importlib.import_module(f"tree_sitter_{language}")
        lang = tree_sitter.Language(lang_mod.language())
        return tree_sitter.Parser(lang)
    except (ImportError, AttributeError) as e:
        logger.warning("No parser for %s: %s", language, e)
        return None


def parse_content(file_path: str, content: str) -> ParsedFile | None:
    """Parse file content into structural form. Returns None if unparseable."""
    ext = Path(file_path).suffix
    language = SUPPORTED_LANGUAGES.get(ext)
    if not language:
        return None

    parser = _get_parser(language)
    if parser is None:
        return None

    parsed = ParsedFile(
        file_path=file_path,
        language=language,
        sha256=_sha256(content),
        line_count=content.count("\n") + 1,
    )

    tree = parser.parse(content.encode())
    _walk(tree.root_node, parsed, current_function="")

    return parsed


def _walk(node: Any, parsed: ParsedFile, current_function: str) -> None:
    """Recursively walk the AST collecting structural elements."""
    enclosing = current_function

    if node.type in ("function_definition", "function_declaration", "method_definition"):
        name_node = node.child_by_field_name("name")
        if name_node:
            name = name_node.text.decode()
            enclosing = name
            params: list[str] = []
            params_node = node.child_by_field_name("parameters")
            if params_node:
                for child in params_node.children:
                    if child.type in ("identifier", "typed_parameter", "typed_default_parameter"):
                        param_name = child.child_by_field_name("name")
                        params.append(
                            param_name.text.decode() if param_name else child.text.decode()
                        )
            parsed.functions[name] = {
                "name": name,
                "file_path": parsed.file_path,
                "start_line": node.start_point[0] + 1,
                "end_line": node.end_point[0] + 1,
                "parameters": params,
            }

    elif node.type in ("class_definition", "class_declaration"):
        name_node = node.child_by_field_name("name")
        if name_node:
            name = name_node.text.decode()
            parsed.classes[name] = {
                "name": name,
                "file_path": parsed.file_path,
                "start_line": node.start_point[0] + 1,
                "end_line": node.end_point[0] + 1,
            }

    elif node.type in ("import_statement", "import_from_statement"):
        parsed.imports.append(node.text.decode())

    elif node.type == "call":
        fn_node = node.child_by_field_name("function")
        if fn_node:
            callee = fn_node.text.decode()
            if "." in callee:
                callee = callee.rsplit(".", 1)[-1]
            parsed.calls.append({
                "caller_function": current_function,
                "callee_name": callee,
                "line": node.start_point[0] + 1,
            })

    for child in node.children:
        _walk(child, parsed, enclosing)


def _properties_changed(old: dict[str, Any], new: dict[str, Any]) -> bool:
    """True if the structural properties of a node changed."""
    keys = set(old) | set(new)
    keys.discard("file_path")
    return any(old.get(k) != new.get(k) for k in keys)


def diff_parsed(
    before: ParsedFile | None,
    after: ParsedFile | None,
    file_path: str,
    delta: Delta,
) -> None:
    """Append operations to `delta` representing the change from before to after.

    Both arguments may be None to represent file creation or deletion:
        - before=None, after=X: file was added
        - before=X, after=None: file was deleted
        - before=X, after=Y: file was modified
    """

    # File node itself
    if before is None and after is not None:
        delta.add_node(
            "File",
            file_path=after.file_path,
            language=after.language,
            line_count=after.line_count,
            sha256=after.sha256,
        )
    elif after is None and before is not None:
        delta.remove_node("File", file_path=file_path)
        # Also remove all functions/classes from the old file
        for fn_name in before.functions:
            delta.remove_node("Function", file_path=file_path, name=fn_name)
        for cls_name in before.classes:
            delta.remove_node("Class", file_path=file_path, name=cls_name)
        return

    if before is None or after is None:
        # Edge cases handled above
        if after is not None:
            # All nodes are new
            for fn_name, fn_props in after.functions.items():
                delta.add_node("Function", **fn_props)
                delta.add_edge(
                    "CONTAINS",
                    "File", after.file_path, None,
                    "Function", after.file_path, fn_name,
                )
            for cls_name, cls_props in after.classes.items():
                delta.add_node("Class", **cls_props)
                delta.add_edge(
                    "CONTAINS",
                    "File", after.file_path, None,
                    "Class", after.file_path, cls_name,
                )
            _emit_calls(after, delta)
        return

    # File modified — diff the contents
    before_fns = set(before.functions)
    after_fns = set(after.functions)

    # Removed functions
    for fn_name in before_fns - after_fns:
        delta.remove_edge(
            "CONTAINS",
            "File", file_path, None,
            "Function", file_path, fn_name,
        )
        delta.remove_node("Function", file_path=file_path, name=fn_name)

    # Added functions
    for fn_name in after_fns - before_fns:
        delta.add_node("Function", **after.functions[fn_name])
        delta.add_edge(
            "CONTAINS",
            "File", file_path, None,
            "Function", file_path, fn_name,
        )

    # Modified functions (same name, different signature/body)
    for fn_name in after_fns & before_fns:
        if _properties_changed(before.functions[fn_name], after.functions[fn_name]):
            # Easier to remove + re-add than emit update_node — composition handles it
            delta.remove_node("Function", file_path=file_path, name=fn_name)
            delta.add_node("Function", **after.functions[fn_name])
            delta.add_edge(
                "CONTAINS",
                "File", file_path, None,
                "Function", file_path, fn_name,
            )

    # Same logic for classes
    before_cs = set(before.classes)
    after_cs = set(after.classes)
    for cls_name in before_cs - after_cs:
        delta.remove_edge(
            "CONTAINS",
            "File", file_path, None,
            "Class", file_path, cls_name,
        )
        delta.remove_node("Class", file_path=file_path, name=cls_name)
    for cls_name in after_cs - before_cs:
        delta.add_node("Class", **after.classes[cls_name])
        delta.add_edge(
            "CONTAINS",
            "File", file_path, None,
            "Class", file_path, cls_name,
        )
    for cls_name in after_cs & before_cs:
        if _properties_changed(before.classes[cls_name], after.classes[cls_name]):
            delta.remove_node("Class", file_path=file_path, name=cls_name)
            delta.add_node("Class", **after.classes[cls_name])
            delta.add_edge(
                "CONTAINS",
                "File", file_path, None,
                "Class", file_path, cls_name,
            )

    # Update the File node sha256 if content changed
    if before.sha256 != after.sha256:
        delta.remove_node("File", file_path=file_path)
        delta.add_node(
            "File",
            file_path=after.file_path,
            language=after.language,
            line_count=after.line_count,
            sha256=after.sha256,
        )

    # Calls — for now, recompute all calls in the file as part of any change
    # (precise call diffing requires more sophisticated AST diffing)
    if before.calls != after.calls:
        # Remove all old calls from this file
        for call in before.calls:
            if call["caller_function"]:
                delta.remove_edge(
                    "CALLS",
                    "Function", file_path, call["caller_function"],
                    "Function", None, call["callee_name"],
                )
        # Add all new calls
        _emit_calls(after, delta)


def _emit_calls(parsed: ParsedFile, delta: Delta) -> None:
    """Emit add_edge ops for all CALLS in a parsed file."""
    for call in parsed.calls:
        if not call["caller_function"]:
            continue
        delta.add_edge(
            "CALLS",
            "Function", parsed.file_path, call["caller_function"],
            "Function", None, call["callee_name"],
            line=call["line"],
        )


def compute_file_delta(
    file_path: str,
    before_content: str | None,
    after_content: str | None,
    org: str,
    repo: str,
    branch: str = "main",
    base_sha: str | None = None,
    head_sha: str | None = None,
    author: str | None = None,
    session_id: str | None = None,
) -> Delta:
    """Compute a Delta from a single file's before/after content.

    Args:
        file_path: Repo-relative path to the file (e.g. "src/auth/jwt.py")
        before_content: File contents at the base SHA, or None if file was added
        after_content: File contents in the working copy, or None if file was deleted
        org, repo, branch: Scope identifiers
        base_sha, head_sha: Optional git SHAs for tracking
        author, session_id: Optional provenance

    Returns:
        A Delta describing the changes from before to after.
    """
    delta = Delta(
        scope=DeltaScope(
            org=org,
            repo=repo,
            branch=branch,
            base_sha=base_sha,
            head_sha=head_sha,
        ),
        provenance=Provenance(author=author, session_id=session_id) if author or session_id else None,
    )

    before = parse_content(file_path, before_content) if before_content else None
    after = parse_content(file_path, after_content) if after_content else None

    diff_parsed(before, after, file_path, delta)
    return delta


def compute_multi_file_delta(
    changes: dict[str, tuple[str | None, str | None]],
    org: str,
    repo: str,
    branch: str = "main",
    base_sha: str | None = None,
    head_sha: str | None = None,
    author: str | None = None,
    session_id: str | None = None,
) -> Delta:
    """Compute a single Delta covering multiple file changes.

    Args:
        changes: Dict mapping file_path -> (before_content, after_content)
                 Use None for added/deleted files.
        ... (other args same as compute_file_delta)

    Returns:
        A single Delta with operations from all files.
    """
    delta = Delta(
        scope=DeltaScope(
            org=org,
            repo=repo,
            branch=branch,
            base_sha=base_sha,
            head_sha=head_sha,
        ),
        provenance=Provenance(author=author, session_id=session_id) if author or session_id else None,
    )

    for file_path, (before_content, after_content) in changes.items():
        before = parse_content(file_path, before_content) if before_content else None
        after = parse_content(file_path, after_content) if after_content else None
        diff_parsed(before, after, file_path, delta)

    return delta


__all__ = [
    "ParsedFile",
    "parse_content",
    "diff_parsed",
    "compute_file_delta",
    "compute_multi_file_delta",
]
