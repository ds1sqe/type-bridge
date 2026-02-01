"""Unified lookup parsing utilities.

This module provides parsing for Django-style lookup filters (e.g. `age__gt=30`).
It unifies logic previously duplicated in EntityManager and RelationManager.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from type_bridge.expressions import Expression

if TYPE_CHECKING:
    from type_bridge.attribute import Attribute


def build_lookup_expression(
    attr_type: type[Attribute],
    lookup: str,
    value: Any,
) -> Expression:
    """Build an Expression for the given lookup operator.

    Delegates to Attribute type methods for type-specific logic.

    Args:
        attr_type: The attribute type class
        lookup: Lookup operator (exact, gt, gte, lt, lte, in, isnull, contains, etc.)
        value: The filter value

    Returns:
        Expression object

    Raises:
        ValueError: If unsupported lookup or type mismatch
    """
    # Fully delegated to the Attribute class to avoid coupling here
    return attr_type.build_lookup(lookup, value)
