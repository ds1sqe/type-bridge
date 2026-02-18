"""Naming convention utilities for code generation.

Handles conversion between TypeDB naming conventions (kebab-case) and
Python conventions (PascalCase for classes, snake_case for identifiers).
"""

from __future__ import annotations

from collections.abc import Mapping


def to_class_name(label: str) -> str:
    """Convert a TypeDB name to PascalCase for Python classes.

    Handles both kebab-case and snake_case inputs.

    Examples:
        "person" -> "Person"
        "isbn-13" -> "Isbn13"
        "user_story" -> "UserStory"
        "order-line" -> "OrderLine"
    """
    # Split by both - and _
    parts = label.replace("_", "-").split("-")
    return "".join(part.capitalize() for part in parts)


def to_python_name(label: str) -> str:
    """Convert a TypeDB name to snake_case for Python identifiers.

    Examples:
        "person" -> "person"
        "isbn-13" -> "isbn_13"
        "order-line" -> "order_line"
        "birth-date" -> "birth_date"
    """
    return label.replace("-", "_")


def build_class_name_map(names: Mapping[str, object]) -> dict[str, str]:
    """Build a mapping from TypeDB names to Python class names.

    Args:
        names: Mapping with TypeDB names as keys (values ignored)

    Returns:
        Dict mapping TypeDB name -> Python class name
    """
    return {name: to_class_name(name) for name in names}


def render_all_export(names: list[str], extras: list[str] | None = None) -> list[str]:
    """Generate __all__ export list lines for a module.

    Args:
        names: List of names to export (will be sorted)
        extras: Optional additional names to export

    Returns:
        Lines for the __all__ block including trailing newline
    """
    lines = ["__all__ = ["]
    for name in sorted(names):
        lines.append(f'    "{name}",')
    for extra in extras or []:
        lines.append(f'    "{extra}",')
    lines.append("]")
    lines.append("")
    return lines
