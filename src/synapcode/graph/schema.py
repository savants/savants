"""Code Property Graph schema definitions for FalkorDB.

Node types:
  - File: source file in the repository
  - Function: function/method definition
  - Class: class definition
  - Module: logical module or package
  - Variable: global/module-level variable or constant

Edge types:
  - CONTAINS: File -> Function/Class/Variable
  - CALLS: Function -> Function
  - INHERITS_FROM: Class -> Class
  - IMPORTS: File -> File/Module
  - DEPENDS_ON: Module -> Module
  - DEFINES: Class -> Function (methods)
  - REFERENCES: Function -> Variable
"""

from __future__ import annotations

from dataclasses import dataclass, field

# Cypher schema creation queries
SCHEMA_INDICES = [
    "CREATE INDEX FOR (f:File) ON (f.path)",
    "CREATE INDEX FOR (fn:Function) ON (fn.name)",
    "CREATE INDEX FOR (c:Class) ON (c.name)",
    "CREATE INDEX FOR (m:Module) ON (m.name)",
]


@dataclass
class FileNode:
    path: str
    language: str
    line_count: int
    sha256: str  # provenance hash
    last_commit: str = ""


@dataclass
class FunctionNode:
    name: str
    file_path: str
    start_line: int
    end_line: int
    parameters: list[str] = field(default_factory=list)
    return_type: str = ""


@dataclass
class ClassNode:
    name: str
    file_path: str
    start_line: int
    end_line: int
    bases: list[str] = field(default_factory=list)


@dataclass
class ProvenanceStamp:
    """SHA-256 provenance attached to every graph entry."""

    source_commit: str
    author: str
    timestamp: str
    content_hash: str


def create_file_query(node: FileNode) -> tuple[str, dict]:
    return (
        "MERGE (f:File {path: $path}) "
        "SET f.language = $language, f.line_count = $line_count, "
        "f.sha256 = $sha256, f.last_commit = $last_commit",
        {
            "path": node.path,
            "language": node.language,
            "line_count": node.line_count,
            "sha256": node.sha256,
            "last_commit": node.last_commit,
        },
    )


def create_function_query(node: FunctionNode) -> tuple[str, dict]:
    return (
        "MERGE (fn:Function {name: $name, file_path: $file_path}) "
        "SET fn.start_line = $start_line, fn.end_line = $end_line, "
        "fn.parameters = $parameters, fn.return_type = $return_type",
        {
            "name": node.name,
            "file_path": node.file_path,
            "start_line": node.start_line,
            "end_line": node.end_line,
            "parameters": node.parameters,
            "return_type": node.return_type,
        },
    )


def create_class_query(node: ClassNode) -> tuple[str, dict]:
    return (
        "MERGE (c:Class {name: $name, file_path: $file_path}) "
        "SET c.start_line = $start_line, c.end_line = $end_line, "
        "c.bases = $bases",
        {
            "name": node.name,
            "file_path": node.file_path,
            "start_line": node.start_line,
            "end_line": node.end_line,
            "bases": node.bases,
        },
    )


def create_edge_query(
    from_label: str, from_key: str, from_val: str,
    to_label: str, to_key: str, to_val: str,
    edge_type: str,
) -> tuple[str, dict]:
    return (
        f"MATCH (a:{from_label} {{{from_key}: $from_val}}) "
        f"MATCH (b:{to_label} {{{to_key}: $to_val}}) "
        f"MERGE (a)-[:{edge_type}]->(b)",
        {"from_val": from_val, "to_val": to_val},
    )
