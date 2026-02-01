"""
Role-player field references for type-safe query building.

This module provides field reference classes that enable type-safe filtering
of role-player attributes in relations:

    Employment.employee.age.gt(Age(30))
    Employment.employer.name.contains(Name("Tech"))

These expressions return RolePlayerExpr instances that wrap the underlying
comparison or string expressions with role context for proper TypeQL generation.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.attribute.string import String
    from type_bridge.expressions.base import Expression
    from type_bridge.expressions.role_player import RolePlayerExpr
    from type_bridge.models.base import TypeDBType


class RolePlayerFieldRef[T: "Attribute"]:
    """Field reference for role-player attributes.

    Provides comparison methods that return RolePlayerExpr instead of
    regular ComparisonExpr, adding role context for proper TypeQL generation.

    Example:
        Employment.employee.age  # Returns RolePlayerNumericFieldRef[Age]
        Employment.employee.age.gt(Age(30))  # Returns RolePlayerExpr
    """

    def __init__(
        self,
        role_name: str,
        field_name: str,
        attr_type: type[T],
        player_types: tuple[type[TypeDBType], ...],
    ):
        """Create a role-player field reference.

        Args:
            role_name: Name of the role (e.g., "employee")
            field_name: Python field name on the player entity
            attr_type: Attribute type class
            player_types: Tuple of entity types that can play this role
        """
        self.role_name = role_name
        self.field_name = field_name
        self.attr_type = attr_type
        self.player_types = player_types

    def _wrap_expr(self, inner_expr: Expression) -> RolePlayerExpr:
        """Wrap inner expression in RolePlayerExpr with role context.

        Args:
            inner_expr: The underlying comparison or string expression

        Returns:
            RolePlayerExpr that wraps the inner expression
        """
        from type_bridge.expressions.role_player import RolePlayerExpr

        return RolePlayerExpr(
            role_name=self.role_name,
            inner_expr=inner_expr,
            player_types=self.player_types,
        )

    def lt(self, value: T) -> RolePlayerExpr:
        """Create a less-than comparison expression.

        Args:
            value: Value to compare against

        Returns:
            RolePlayerExpr wrapping a less-than comparison
        """
        return self._wrap_expr(self.attr_type.lt(value))

    def gt(self, value: T) -> RolePlayerExpr:
        """Create a greater-than comparison expression.

        Args:
            value: Value to compare against

        Returns:
            RolePlayerExpr wrapping a greater-than comparison
        """
        return self._wrap_expr(self.attr_type.gt(value))

    def lte(self, value: T) -> RolePlayerExpr:
        """Create a less-than-or-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            RolePlayerExpr wrapping a less-than-or-equal comparison
        """
        return self._wrap_expr(self.attr_type.lte(value))

    def gte(self, value: T) -> RolePlayerExpr:
        """Create a greater-than-or-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            RolePlayerExpr wrapping a greater-than-or-equal comparison
        """
        return self._wrap_expr(self.attr_type.gte(value))

    def eq(self, value: T) -> RolePlayerExpr:
        """Create an equality comparison expression.

        Args:
            value: Value to compare against

        Returns:
            RolePlayerExpr wrapping an equality comparison
        """
        return self._wrap_expr(self.attr_type.eq(value))

    def neq(self, value: T) -> RolePlayerExpr:
        """Create a not-equal comparison expression.

        Args:
            value: Value to compare against

        Returns:
            RolePlayerExpr wrapping a not-equal comparison
        """
        return self._wrap_expr(self.attr_type.neq(value))


class RolePlayerStringFieldRef[T: "String"](RolePlayerFieldRef[T]):
    """Role-player field reference for String attributes.

    Provides additional string-specific operations like contains, like, regex,
    all wrapped in RolePlayerExpr for proper role context.

    Example:
        Employment.employer.name  # Returns RolePlayerStringFieldRef[Name]
        Employment.employer.name.contains(Name("Tech"))  # Returns RolePlayerExpr
    """

    def contains(self, value: T) -> RolePlayerExpr:
        """Create a string contains expression.

        Args:
            value: Substring to search for

        Returns:
            RolePlayerExpr wrapping a contains expression
        """
        return self._wrap_expr(self.attr_type.contains(value))

    def like(self, pattern: T) -> RolePlayerExpr:
        """Create a string pattern matching expression.

        Args:
            pattern: Pattern to match (SQL LIKE style)

        Returns:
            RolePlayerExpr wrapping a like expression
        """
        return self._wrap_expr(self.attr_type.like(pattern))

    def regex(self, pattern: T) -> RolePlayerExpr:
        """Create a string regex expression.

        Args:
            pattern: Regex pattern to match

        Returns:
            RolePlayerExpr wrapping a regex expression
        """
        return self._wrap_expr(self.attr_type.regex(pattern))


class RolePlayerNumericFieldRef[T: "Attribute"](RolePlayerFieldRef[T]):
    """Role-player field reference for numeric attributes.

    Inherits comparison methods from RolePlayerFieldRef.
    Aggregation methods (sum, avg, etc.) are not supported for role-player
    fields as they require grouping context.

    Example:
        Employment.employee.age  # Returns RolePlayerNumericFieldRef[Age]
        Employment.employee.age.gt(Age(30))  # Returns RolePlayerExpr
    """

    pass  # Inherits comparison methods; aggregations not applicable for role-player fields


class RoleRef[T: "TypeDBType"]:
    """Reference to a role for type-safe attribute access.

    Returned when accessing a Role descriptor from the Relation class level.
    Enables chained attribute access for building type-safe filter expressions.

    Example:
        Employment.employee         # Returns RoleRef[Person]
        Employment.employee.age     # Returns RolePlayerNumericFieldRef[Age]
        Employment.employee.age.gt(Age(30))  # Returns RolePlayerExpr

    For Role.multi() (polymorphic roles), attributes from all player types
    are available. If an attribute exists on at least one player type,
    it can be accessed.

    Relations can also be role players:
        Permission.permitted_access  # Returns RoleRef[Access] where Access is a Relation
    """

    def __init__(
        self,
        role_name: str,
        player_types: tuple[type[T], ...],
    ):
        """Create a role reference.

        Args:
            role_name: Name of the role (e.g., "employee")
            player_types: Tuple of types (Entity or Relation) that can play this role
        """
        self.role_name = role_name
        self.player_types = player_types
        # Cache of collected attributes from all player types
        self._player_attrs: dict[str, tuple[type, Any]] | None = None

    def _get_player_attrs(self) -> dict[str, tuple[type, Any]]:
        """Collect all attributes from all player types (cached).

        For Role.multi() with multiple player types, this collects the union
        of all attributes. If the same attribute name exists on multiple
        player types, the first player type's attribute type is used.

        Returns:
            Dict mapping field name to (attr_type, attr_info) tuples
        """
        if self._player_attrs is None:
            all_attrs: dict[str, tuple[type, Any]] = {}
            for player_type in self.player_types:
                player_attrs = player_type.get_all_attributes()
                for field_name, attr_info in player_attrs.items():
                    if field_name not in all_attrs:
                        all_attrs[field_name] = (attr_info.typ, attr_info)
            self._player_attrs = all_attrs
        return self._player_attrs

    def __getattr__(self, name: str) -> RolePlayerFieldRef[Any]:
        """Access role-player attribute for query building.

        Args:
            name: Attribute name to access

        Returns:
            Appropriate RolePlayerFieldRef subclass based on attribute type

        Raises:
            AttributeError: If attribute doesn't exist on any player type
        """
        # Avoid infinite recursion for special attributes
        if name.startswith("_"):
            raise AttributeError(f"'{type(self).__name__}' object has no attribute '{name}'")

        player_attrs = self._get_player_attrs()

        if name not in player_attrs:
            available = sorted(player_attrs.keys())
            raise AttributeError(
                f"Role '{self.role_name}' players do not have attribute '{name}'. "
                f"Available attributes: {available}"
            )

        attr_type, _attr_info = player_attrs[name]
        return self._make_field_ref(name, attr_type)

    def _make_field_ref(self, field_name: str, attr_type: type) -> RolePlayerFieldRef[Any]:
        """Create appropriate RolePlayerFieldRef subclass based on attribute type.

        Args:
            field_name: Name of the field
            attr_type: Type of the attribute

        Returns:
            RolePlayerFieldRef, RolePlayerStringFieldRef, or RolePlayerNumericFieldRef
        """
        from type_bridge.attribute.decimal import Decimal
        from type_bridge.attribute.double import Double
        from type_bridge.attribute.integer import Integer
        from type_bridge.attribute.string import String

        if issubclass(attr_type, String):
            return RolePlayerStringFieldRef(
                role_name=self.role_name,
                field_name=field_name,
                attr_type=attr_type,
                player_types=self.player_types,
            )

        if issubclass(attr_type, (Integer, Double, Decimal)):
            return RolePlayerNumericFieldRef(
                role_name=self.role_name,
                field_name=field_name,
                attr_type=attr_type,
                player_types=self.player_types,
            )

        return RolePlayerFieldRef(
            role_name=self.role_name,
            field_name=field_name,
            attr_type=attr_type,
            player_types=self.player_types,
        )

    def __dir__(self) -> list[str]:
        """Enable IDE autocompletion for available player attributes.

        Returns:
            List of available attribute names from all player types
        """
        return sorted(self._get_player_attrs().keys())
