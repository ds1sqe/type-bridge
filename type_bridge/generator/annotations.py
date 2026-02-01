"""Annotation parsing for TQL schema comments.

Parses TypeDB-style annotations from comments like:
    # @prefix(PROJ)
    # @searchable
    # @tags(api, public)
    entity project;

Annotations use TypeDB's syntax:
    @name           -> boolean True
    @name(value)    -> parsed value (int, float, bool, or string)
    @name(a, b, c)  -> list of values
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .models import AnnotationScalar, AnnotationValue


# Pattern to match annotation comments: # @name or # @name(...)
ANNOTATION_PATTERN = re.compile(r"#\s*@([\w-]+)(?:\(([^)]*)\))?\s*$")

# Pattern to match docstring comments: ## text (double hash for docstrings)
DOCSTRING_PATTERN = re.compile(r"^##\s*(.*)$")

# Pattern to match definition starts: entity X, attribute X, relation X
DEFINITION_PATTERN = re.compile(r"^\s*(entity|attribute|relation)\s+([\w-]+)")

# Pattern to match role definitions within relations: relates X
ROLE_PATTERN = re.compile(r"^\s*#\s*@([\w-]+)(?:\(([^)]*)\))?\s*$|^\s*relates\s+([\w-]+)")


def parse_annotation_value(raw: str) -> AnnotationScalar:
    """Parse a single annotation value.

    Args:
        raw: Raw value string (may be quoted, numeric, or boolean)

    Returns:
        Parsed value as int, float, bool, or str
    """
    value = raw.strip()

    # Empty string
    if not value:
        return ""

    # Quoted string - remove quotes
    if (value.startswith('"') and value.endswith('"')) or (
        value.startswith("'") and value.endswith("'")
    ):
        return value[1:-1]

    # Boolean
    if value.lower() == "true":
        return True
    if value.lower() == "false":
        return False

    # Integer
    try:
        return int(value)
    except ValueError:
        pass

    # Float
    try:
        return float(value)
    except ValueError:
        pass

    # Plain string (unquoted identifier)
    return value


def parse_annotation(line: str) -> tuple[str, AnnotationValue] | None:
    """Parse a single annotation from a comment line.

    Args:
        line: A line that may contain an annotation comment

    Returns:
        Tuple of (name, value) if annotation found, None otherwise

    Examples:
        "# @searchable" -> ("searchable", True)
        "# @prefix(PROJ)" -> ("prefix", "PROJ")
        "# @priority(3)" -> ("priority", 3)
        "# @enabled(true)" -> ("enabled", True)
        "# @tags(api, public)" -> ("tags", ["api", "public"])
    """
    match = ANNOTATION_PATTERN.match(line.strip())
    if not match:
        return None

    name = match.group(1)
    args = match.group(2)

    # No arguments - boolean flag
    if args is None:
        return (name, True)

    # Split by comma for potential list
    parts = [p.strip() for p in args.split(",") if p.strip()]

    if len(parts) == 0:
        # Empty parens: @name() -> treat as True
        return (name, True)
    elif len(parts) == 1:
        # Single value
        return (name, parse_annotation_value(parts[0]))
    else:
        # Multiple values -> list
        return (name, [parse_annotation_value(p) for p in parts])


def extract_annotations(
    schema_text: str,
) -> tuple[
    dict[str, dict[str, AnnotationValue]],  # entity annotations
    dict[str, dict[str, AnnotationValue]],  # attribute annotations
    dict[str, dict[str, AnnotationValue]],  # relation annotations
    dict[
        str, dict[str, dict[str, AnnotationValue]]
    ],  # role annotations (relation -> role -> annotations)
]:
    """Extract all annotations from a TQL schema.

    Pre-processes the schema text to find annotation comments and associate
    them with the definitions that follow.

    Args:
        schema_text: The full TQL schema text

    Returns:
        Four dicts mapping definition names to their annotation dicts:
        - entity_annotations
        - attribute_annotations
        - relation_annotations
        - role_annotations (nested: relation_name -> role_name -> annotations)
    """
    entity_annotations: dict[str, dict[str, AnnotationValue]] = {}
    attribute_annotations: dict[str, dict[str, AnnotationValue]] = {}
    relation_annotations: dict[str, dict[str, AnnotationValue]] = {}
    role_annotations: dict[str, dict[str, dict[str, AnnotationValue]]] = {}

    lines = schema_text.splitlines()
    pending: dict[str, AnnotationValue] = {}
    pending_docstring: list[str] = []
    current_relation: str | None = None
    pending_role: dict[str, AnnotationValue] = {}

    for line in lines:
        stripped = line.strip()

        # Check for docstring comment (## text)
        docstring_match = DOCSTRING_PATTERN.match(stripped)
        if docstring_match:
            doc_text = docstring_match.group(1).strip()
            # Check if it's also an annotation (## @name)
            if doc_text.startswith("@"):
                annotation = parse_annotation("# " + doc_text)
                if annotation:
                    if current_relation is not None:
                        pending_role[annotation[0]] = annotation[1]
                    else:
                        pending[annotation[0]] = annotation[1]
                    continue
            # Regular docstring text
            pending_docstring.append(doc_text)
            continue

        # Check for annotation comment
        annotation = parse_annotation(stripped)
        if annotation:
            # Check if we're inside a relation definition (for role annotations)
            if current_relation is not None:
                pending_role[annotation[0]] = annotation[1]
            else:
                pending[annotation[0]] = annotation[1]
            continue

        # Check for definition start
        def_match = DEFINITION_PATTERN.match(stripped)
        if def_match:
            def_type = def_match.group(1)
            def_name = def_match.group(2)

            # Apply pending docstring
            if pending_docstring:
                docstring = " ".join(pending_docstring)
                pending["_docstring"] = docstring
                pending_docstring.clear()

            if pending:
                if def_type == "entity":
                    entity_annotations[def_name] = pending.copy()
                elif def_type == "attribute":
                    attribute_annotations[def_name] = pending.copy()
                elif def_type == "relation":
                    relation_annotations[def_name] = pending.copy()
                    current_relation = def_name
                    role_annotations[def_name] = {}
                pending.clear()

            # Track that we're in a relation for role annotations
            if def_type == "relation":
                current_relation = def_name
                if def_name not in role_annotations:
                    role_annotations[def_name] = {}
            continue

        # Check for role definition within relation
        if current_relation is not None:
            role_match = re.search(r"relates\s+([\w-]+)", stripped)
            if role_match:
                role_name = role_match.group(1)
                if pending_role:
                    role_annotations[current_relation][role_name] = pending_role.copy()
                    pending_role.clear()

        # Check for end of definition (semicolon)
        if ";" in stripped:
            current_relation = None
            pending_role.clear()

        # Non-comment, non-definition line - clear pending docstring
        if stripped and not stripped.startswith("#"):
            pending_docstring.clear()

    return entity_annotations, attribute_annotations, relation_annotations, role_annotations
