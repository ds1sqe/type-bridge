"""Comparison expressions for value-based filtering.

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
    For example, when var="$actor" and attr="name":
        Generated: $actor has name $actor_name; $actor_name > "value"
"""

from typing import TYPE_CHECKING, Literal

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute


class ComparisonExpr[T: "Attribute"](Expression):
    """
    Type-safe comparison expression for filtering by attribute values.

    Represents comparisons like age > 30, score <= 100, etc.
    """

    def __init__(
        self,
        attr_type: type[T],
        operator: Literal[">", "<", ">=", "<=", "==", "!="],
        value: T,
    ):
        """
        Create a comparison expression.

        Args:
            attr_type: Attribute type to filter on
            operator: Comparison operator
            value: Value to compare against
        """
        self.attr_type = attr_type
        self.operator = operator
        self.value = value

    def to_typeql(self, var: str) -> str:
        """
        Generate TypeQL pattern for this comparison.

        Example output: "$e has Age $e_age; $e_age > 30"

        Args:
            var: Entity variable name (e.g., "$e", "$actor")

        Returns:
            TypeQL pattern string (without trailing semicolon)
        """
        from type_bridge.crud.utils import format_value

        # Format the value for TypeQL
        formatted_value = format_value(self.value.value)

        # Get attribute type name for schema
        attr_type_name = self.attr_type.get_attribute_name()

        # Generate unique attribute variable name by combining entity var and attr name
        # This prevents collisions when filtering multiple entities by same attribute type
        # e.g., $actor -> $actor_name, $target -> $target_name
        var_prefix = var.lstrip("$")
        attr_var = f"${var_prefix}_{attr_type_name.lower()}"

        # Generate pattern (no trailing semicolon - QueryBuilder adds those)
        pattern = (
            f"{var} has {attr_type_name} {attr_var}; {attr_var} {self.operator} {formatted_value}"
        )

        return pattern


class AttributeExistsExpr[T: "Attribute"](Expression):
    """Attribute presence/absence check expression."""

    def __init__(self, attr_type: type[T], present: bool):
        self.attr_type = attr_type
        self.present = present

    def to_typeql(self, var: str) -> str:
        attr_type_name = self.attr_type.get_attribute_name()
        # Generate unique attribute variable name by combining entity var and attr name
        var_prefix = var.lstrip("$")
        attr_var = f"${var_prefix}_{attr_type_name.lower()}"

        # Presence: simple has clause; Absence: negate a has clause block
        if self.present:
            return f"{var} has {attr_type_name} {attr_var}"
        return f"not {{ {var} has {attr_type_name} {attr_var}; }}"
