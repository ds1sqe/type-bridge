"""Role-player lookup filter parsing utilities.

This module provides parsing for Django-style lookup filters on role-player attributes
in relation queries.

Example:
    Employment.manager(db).filter(employee__age__gt=30, employer__name__contains="Tech")
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

from type_bridge.crud.lookup import build_lookup_expression
from type_bridge.expressions import (
    BooleanExpr,
    Expression,
    IidExpr,
    RolePlayerExpr,
)

if TYPE_CHECKING:
    from type_bridge.attribute.base import _QueryAttribute as Attribute
    from type_bridge.models.relation import _QueryRelation as Relation


def parse_role_lookup_filters(
    model_class: type[Relation],
    filters: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any], dict[str, list[RolePlayerExpr]], list[Expression]]:
    """Parse Django-style lookup filters including role-player attributes.

    Handles patterns like:
    - employee__age__gt=30 -> role lookup expression
    - employee__name="Alice" -> role lookup exact match
    - employee=alice -> exact role player match (entity instance)
    - position="Engineer" -> relation attribute filter
    - salary__gt=85000 -> relation attribute lookup expression

    Args:
        model_class: The Relation class being queried
        filters: Raw filter keyword arguments

    Returns:
        Tuple of:
        - attr_filters: dict of relation attribute filters (exact match)
        - role_player_filters: dict of role -> entity instance (exact match)
        - role_expressions: dict of role_name -> list of Expression objects
        - attr_expressions: list of Expression objects for relation attribute lookups

    Raises:
        ValueError: If unknown role/attribute or invalid lookup operator
    """
    owned_attrs = model_class.get_all_attributes()
    roles = model_class._roles

    attr_filters: dict[str, Any] = {}
    role_player_filters: dict[str, Any] = {}
    role_expressions: dict[str, list[RolePlayerExpr]] = {}
    attr_expressions: list[Expression] = []

    for raw_key, raw_value in filters.items():
        # Handle special iid__in lookup for relation itself (IID is not an attribute)
        if raw_key == "iid__in":
            if not isinstance(raw_value, (list, tuple, set)):
                raise ValueError("iid__in lookup requires an iterable of IID strings")
            iids = list(raw_value)
            if not iids:
                raise ValueError("iid__in lookup requires a non-empty iterable")
            iid_exprs: list[Expression] = [IidExpr(iid) for iid in iids]
            if len(iid_exprs) == 1:
                attr_expressions.append(iid_exprs[0])
            else:
                attr_expressions.append(BooleanExpr("or", iid_exprs))
            continue

        # Case 1: No "__" - either exact attribute match or entity instance for role
        if "__" not in raw_key:
            if raw_key in roles:
                # Exact role player match (entity instance)
                role_player_filters[raw_key] = raw_value
            elif raw_key in owned_attrs:
                # Exact relation attribute match
                attr_filters[raw_key] = raw_value
            else:
                raise ValueError(
                    f"Unknown filter field '{raw_key}' for {model_class.__name__}. "
                    f"Available roles: {list(roles.keys())}, "
                    f"Available attributes: {list(owned_attrs.keys())}"
                )
            continue

        # Case 2: Has "__" - parse parts
        parts = raw_key.split("__")
        first_part = parts[0]

        # Check if first part is a role name
        if first_part in roles:
            # Role lookup: role__attr or role__attr__lookup
            role_name = first_part
            remaining_parts = parts[1:]

            if not remaining_parts:
                raise ValueError(
                    f"Role lookup '{raw_key}' must specify an attribute (e.g., {role_name}__name)"
                )

            # Parse remaining as attr or attr__lookup
            expr = _parse_role_attribute_lookup(model_class, role_name, remaining_parts, raw_value)

            if role_name not in role_expressions:
                role_expressions[role_name] = []
            role_expressions[role_name].append(expr)
        elif first_part in owned_attrs:
            # Relation attribute lookup: attr__lookup
            # Parse into expression using build_lookup_expression
            lookup = parts[1] if len(parts) > 1 else "exact"
            attr_info = owned_attrs[first_part]
            attr_type = attr_info.typ

            expr = build_lookup_expression(attr_type, lookup, raw_value)
            attr_expressions.append(expr)
        else:
            raise ValueError(
                f"Unknown filter field '{first_part}' for {model_class.__name__}. "
                f"Available roles: {list(roles.keys())}, "
                f"Available attributes: {list(owned_attrs.keys())}"
            )

    return attr_filters, role_player_filters, role_expressions, attr_expressions


def _parse_role_attribute_lookup(
    model_class: type[Relation],
    role_name: str,
    parts: list[str],
    value: Any,
) -> RolePlayerExpr:
    """Parse role attribute lookup into a type-safe RolePlayerExpr.

    Args:
        model_class: The Relation class
        role_name: Name of the role (e.g., "employee")
        parts: Remaining parts after role name (e.g., ["age", "gt"] or ["name"] or ["iid", "in"])
        value: The filter value

    Returns:
        RolePlayerExpr wrapping the attribute expression for type safety

    Raises:
        ValueError: If attribute not found or invalid lookup
    """
    role = model_class._roles[role_name]
    player_types = role.player_entity_types

    # Handle special case: role__iid__in (IID is not an attribute)
    if parts == ["iid", "in"]:
        if not isinstance(value, (list, tuple, set)):
            raise ValueError(f"{role_name}__iid__in lookup requires an iterable of IID strings")
        iids = list(value)
        if not iids:
            raise ValueError(f"{role_name}__iid__in lookup requires a non-empty iterable")
        iid_exprs: list[Expression] = [IidExpr(iid) for iid in iids]
        if len(iid_exprs) == 1:
            inner_expr = iid_exprs[0]
        else:
            inner_expr = BooleanExpr("or", iid_exprs)
        return RolePlayerExpr(
            role_name=role_name,
            inner_expr=inner_expr,
            player_types=player_types,
        )

    # Collect all attributes from all player types (for Role.multi)
    all_player_attrs: dict[str, tuple[type[Attribute], Any]] = {}
    for player_type in player_types:
        player_attrs = player_type.get_all_attributes()
        for field_name, attr_info in player_attrs.items():
            if field_name not in all_player_attrs:
                all_player_attrs[field_name] = (attr_info.typ, attr_info)

    # Parse: either [attr] or [attr, lookup]
    if len(parts) == 1:
        field_name = parts[0]
        lookup = "exact"
    elif len(parts) == 2:
        field_name, lookup = parts
    else:
        raise ValueError(
            f"Invalid role lookup format. Expected 'role__attr' or 'role__attr__lookup', "
            f"got too many parts: {parts}"
        )

    # Validate field exists on at least one player type
    if field_name not in all_player_attrs:
        available = list(all_player_attrs.keys())
        raise ValueError(
            f"Role '{role_name}' players do not have attribute '{field_name}'. "
            f"Available attributes: {available}"
        )

    attr_type, attr_info = all_player_attrs[field_name]

    # Build inner expression based on lookup type
    inner_expr = build_lookup_expression(attr_type, lookup, value)

    # Wrap in type-safe RolePlayerExpr
    return RolePlayerExpr(
        role_name=role_name,
        inner_expr=inner_expr,
        player_types=player_types,
    )
