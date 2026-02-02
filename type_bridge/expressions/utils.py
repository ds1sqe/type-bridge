"""Shared utilities for query expressions.

TypeDB 3.x Variable Scoping
===========================

TypeDB uses variable bindings to create implicit equality constraints.
If the same variable is used twice in a match clause, both bindings
must have the same value.

Wrong approach::

    $actor has name $name;    -- $name binds to actor's name
    $target has name $name;   -- CONSTRAINT: target's name must EQUAL actor's name!

Correct approach (unique variables)::

    $actor has name $actor_name;    -- $actor_name binds to actor's name
    $target has name $target_name;  -- $target_name binds to target's name (independent)

This is why expressions generate ``${var_prefix}_${attr_name}`` patterns.
For example, when ``var="$actor"`` and ``attr="name"``::

    Generated: $actor has name $actor_name; $actor_name > "value"
"""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute


def generate_attr_var(var: str, attr_type: type["Attribute"]) -> str:
    """Generate unique attribute variable name to avoid TypeDB implicit equality.

    Combines the entity variable prefix with the attribute type name to create
    a unique variable that won't collide with other entities using the same
    attribute type in the same query.

    Uses double underscore ``__`` as separator to prevent collisions between
    variables like ``$actor_name`` + attr ``status`` vs ``$actor`` + attr ``name_status``.
    Also sanitizes hyphens to underscores since TypeDB variables cannot contain hyphens.

    Args:
        var: Entity variable name (e.g., "$e", "$actor")
        attr_type: Attribute type class

    Returns:
        Unique attribute variable (e.g., "$actor__name")

    Example:
        >>> generate_attr_var("$actor", Name)
        "$actor__name"
        >>> generate_attr_var("$e", Age)
        "$e__age"
        >>> generate_attr_var("$e", BirthDate)  # with name="birth-date"
        "$e__birth_date"
    """
    attr_type_name = attr_type.get_attribute_name()
    # Sanitize: replace hyphens with underscores (TypeDB vars can't have hyphens)
    sanitized_attr = attr_type_name.lower().replace("-", "_")
    var_prefix = var.lstrip("$")
    # Use double underscore separator to prevent collisions
    return f"${var_prefix}__{sanitized_attr}"


def generate_has_pattern(var: str, attr_type: type["Attribute"]) -> tuple[str, str]:
    """Generate TypeQL 'has' pattern with unique variable.

    Creates a TypeQL pattern for matching an entity's attribute and returns
    both the generated attribute variable and the full pattern.

    Args:
        var: Entity variable name (e.g., "$e", "$actor")
        attr_type: Attribute type class

    Returns:
        Tuple of (attr_var, pattern) where:
        - attr_var: The generated attribute variable (e.g., "$actor__name")
        - pattern: The 'has' pattern (e.g., "$actor has name $actor__name")

    Example:
        >>> generate_has_pattern("$actor", Name)
        ("$actor__name", "$actor has name $actor__name")
    """
    attr_type_name = attr_type.get_attribute_name()
    attr_var = generate_attr_var(var, attr_type)
    pattern = f"{var} has {attr_type_name} {attr_var}"
    return attr_var, pattern
