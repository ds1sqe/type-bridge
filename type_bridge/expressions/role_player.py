"""Role-player expression for type-safe relation filtering.

This module provides a type-safe wrapper for filtering role players
by their attributes in relation queries.

TypeDB 3.x Variable Scoping:
    TypeDB uses variable bindings to create implicit equality constraints.
    If the same variable is used twice in a match clause, both bindings
    must have the same value.

    Wrong approach:
        $actor has name $name;    -- $name binds to actor's name
        $target has name $name;   -- CONSTRAINT: target's name must EQUAL actor's name!

    Correct approach (unique variables):
        $actor has name $actor_name;    -- $actor_name binds to actor's name
        $target has name $target_name;  -- $target_name binds to target's name (independent)

    This is why expressions generate ${var_prefix}_${attr_name} patterns.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.models.base import TypeDBType


class RolePlayerExpr[T: "TypeDBType"](Expression):
    """Type-safe expression for filtering role players by their attributes.

    Wraps an attribute expression and associates it with a specific role,
    ensuring type safety and proper TypeQL variable scoping.

    TypeDB 3.x Note:
        Variable names are prefixed with the entity variable to avoid collisions.
        Using the same variable twice creates an implicit equality constraint.

        Example:
            # If both actor and target have 'name' attribute:
            $actor has name $actor_name; $actor_name contains "Bot";
            $target has name $target_name; $target_name == "Resource1";

    Args:
        role_name: The role name (e.g., "employee", "employer", "actor")
        inner_expr: The attribute expression to apply (e.g., Age.gt(Age(30)))
        player_types: Tuple of allowed entity types that can play this role

    Example:
        >>> expr = RolePlayerExpr("employee", Age.gt(Age(30)), (Person,))
        >>> expr.to_typeql("$employee")
        '$employee has age $employee_age; $employee_age > 30'
    """

    def __init__(
        self,
        role_name: str,
        inner_expr: Expression,
        player_types: tuple[type[T], ...],
    ):
        """Initialize a role player expression.

        Args:
            role_name: Name of the role being filtered
            inner_expr: Attribute expression to apply to the role player
            player_types: Tuple of entity types allowed to play this role
        """
        self.role_name = role_name
        self.inner_expr = inner_expr
        self.player_types = player_types

    def to_typeql(self, var: str) -> str:
        """Generate TypeQL pattern using role-prefixed variable names.

        The inner expression generates TypeQL with unique variable names
        based on the entity variable (e.g., $employee_age instead of $age).
        This prevents collisions when filtering multiple role players
        by the same attribute type.

        Args:
            var: The role player variable name (e.g., "$employee")

        Returns:
            TypeQL pattern string with role-prefixed attribute variables

        Example:
            >>> expr = RolePlayerExpr("employee", Age.gt(Age(30)), (Person,))
            >>> expr.to_typeql("$employee")
            '$employee has age $employee_age; $employee_age > 30'
        """
        return self.inner_expr.to_typeql(var)

    def get_attribute_types(self) -> set[type[Attribute]]:
        """Get all attribute types referenced by this expression.

        Delegates to the inner expression.

        Returns:
            Set of attribute types used in this expression
        """
        return self.inner_expr.get_attribute_types()

    def __repr__(self) -> str:
        """Return string representation for debugging."""
        return (
            f"RolePlayerExpr(role_name={self.role_name!r}, "
            f"inner_expr={self.inner_expr!r}, "
            f"player_types={self.player_types!r})"
        )
