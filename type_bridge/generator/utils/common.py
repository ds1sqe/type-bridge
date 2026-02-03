"""Common utility functions for code generation."""

from collections.abc import Callable
from typing import Any, TypeVar

T = TypeVar("T")


def topological_sort(
    items: dict[str, T],
    get_parent: Callable[[T], str | None],
) -> list[str]:
    """Sort items topologically so parents come before children.

    This is a generic topological sort that works for any inheritance hierarchy
    (attributes, entities, relations) by accepting a callback to get the parent.

    Args:
        items: Dictionary mapping names to items
        get_parent: Callable that returns the parent name for an item, or None

    Returns:
        List of names sorted so parents come before children

    Example:
        # Sort entities by inheritance
        sorted_names = topological_sort(
            schema.entities,
            lambda entity: entity.parent
        )
    """
    result: list[str] = []
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in visited or name not in items:
            return
        visited.add(name)

        item = items[name]
        parent = get_parent(item)
        if parent and parent in items:
            visit(parent)

        result.append(name)

    for name in items:
        visit(name)

    return result


def group_and_sort_by_value(mapping: dict[str, str]) -> dict[str, list[str]]:
    """Group keys by their values and sort each group alphabetically.

    Useful for grouping attributes by their subkey groups, etc.

    Args:
        mapping: Dictionary mapping keys to group values

    Returns:
        Dictionary mapping group values to sorted lists of keys

    Example:
        >>> group_and_sort_by_value({"name": "group1", "age": "group1", "id": "group2"})
        {"group1": ["age", "name"], "group2": ["id"]}
    """
    groups: dict[str, list[str]] = {}
    for key, group_name in mapping.items():
        groups.setdefault(group_name, []).append(key)

    # Sort each group
    for group in groups:
        groups[group] = sorted(groups[group])

    return groups


def dedupe_preserve_order(items: list[Any]) -> list[Any]:
    """Remove duplicates from a list while preserving order.

    Args:
        items: List that may contain duplicates

    Returns:
        List with duplicates removed, original order preserved
    """
    seen: set[Any] = set()
    result: list[Any] = []
    for item in items:
        if item not in seen:
            seen.add(item)
            result.append(item)
    return result
