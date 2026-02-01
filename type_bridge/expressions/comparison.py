"""Comparison expressions for value-based filtering.

See :mod:`type_bridge.expressions.utils` for documentation on TypeDB 3.x
variable scoping and why we generate unique attribute variables.
"""

from typing import TYPE_CHECKING, Literal

from type_bridge.expressions.base import Expression
from type_bridge.expressions.utils import generate_has_pattern

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

        # Generate unique attribute variable and 'has' pattern
        attr_var, has_pattern = generate_has_pattern(var, self.attr_type)

        # Generate full pattern (no trailing semicolon - QueryBuilder adds those)
        return f"{has_pattern}; {attr_var} {self.operator} {formatted_value}"


class AttributeExistsExpr[T: "Attribute"](Expression):
    """Attribute presence/absence check expression."""

    def __init__(self, attr_type: type[T], present: bool):
        self.attr_type = attr_type
        self.present = present

    def to_typeql(self, var: str) -> str:
        # Generate unique attribute variable and 'has' pattern
        attr_var, has_pattern = generate_has_pattern(var, self.attr_type)

        # Presence: simple has clause; Absence: negate a has clause block
        if self.present:
            return has_pattern
        return f"not {{ {has_pattern}; }}"
