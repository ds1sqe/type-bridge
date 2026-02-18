"""Comparison expressions for value-based filtering.

See :mod:`type_bridge.expressions.utils` for documentation on TypeDB 3.x
variable scoping and why we generate unique attribute variables.
"""

from typing import TYPE_CHECKING, Literal

from type_bridge.expressions.base import Expression
from type_bridge.expressions.utils import generate_attr_var

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.query.ast import Pattern


class ComparisonExpr[T: "Attribute"](Expression):
    """Type-safe comparison expression for filtering by attribute values.

    Represents comparisons like age > 30, score <= 100, etc.
    """

    def __init__(
        self,
        attr_type: type[T],
        operator: Literal[">", "<", ">=", "<=", "==", "!="],
        value: T,
    ):
        """Create a comparison expression.

        Args:
            attr_type: Attribute type to filter on
            operator: Comparison operator
            value: Value to compare against
        """
        self.attr_type = attr_type
        self.operator = operator
        self.value = value

    def to_ast(self, var: str) -> list["Pattern"]:
        """Generate AST patterns for this comparison.

        Example: "$e has age $e_age; $e_age > 30"
        """
        from type_bridge.crud.patterns import _get_literal_type
        from type_bridge.query.ast import (
            HasPattern,
            LiteralValue,
            ValueComparisonPattern,
        )

        attr_type_name = self.attr_type.get_attribute_name()
        attr_var = generate_attr_var(var, self.attr_type)

        # Unwrap value from Attribute wrapper if needed
        raw_value = self.value.value if hasattr(self.value, "value") else self.value
        literal_type = _get_literal_type(raw_value)

        return [
            HasPattern(thing_var=var, attr_type=attr_type_name, attr_var=attr_var),
            ValueComparisonPattern(
                var=attr_var,
                operator=self.operator,
                value=LiteralValue(value=raw_value, value_type=literal_type),
            ),
        ]


class AttributeExistsExpr[T: "Attribute"](Expression):
    """Attribute presence/absence check expression."""

    def __init__(self, attr_type: type[T], present: bool):
        self.attr_type = attr_type
        self.present = present

    def to_ast(self, var: str) -> list["Pattern"]:
        from type_bridge.query.ast import HasPattern, NotPattern

        attr_type_name = self.attr_type.get_attribute_name()
        attr_var = generate_attr_var(var, self.attr_type)

        start_pattern = HasPattern(
            thing_var=var,
            attr_type=attr_type_name,
            attr_var=attr_var,
        )

        if self.present:
            return [start_pattern]
        else:
            return [NotPattern(patterns=[start_pattern])]
