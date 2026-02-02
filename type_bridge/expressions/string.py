"""String-specific expressions for text filtering.

See :mod:`type_bridge.expressions.utils` for documentation on TypeDB 3.x
variable scoping and why we generate unique attribute variables.
"""

from typing import TYPE_CHECKING, Literal

from type_bridge.expressions.base import Expression
from type_bridge.expressions.utils import generate_attr_var

if TYPE_CHECKING:
    from type_bridge.attribute.string import String
    from type_bridge.query.ast import Pattern


class StringExpr[T: "String"](Expression):
    """
    Type-safe string expression for text-based filtering.

    Represents string operations like contains, like (regex), etc.
    """

    def __init__(
        self,
        attr_type: type[T],
        operation: Literal["contains", "like", "regex"],
        pattern: T,
    ):
        """
        Create a string expression.

        Args:
            attr_type: String attribute type to filter on
            operation: String operation type
            pattern: Pattern to match
        """
        self.attr_type = attr_type
        self.operation = operation
        self.pattern = pattern

    def to_ast(self, var: str) -> list["Pattern"]:
        """
        Generate AST patterns for this string operation.

        Example: "$e has Name $e_name; $e_name contains 'Alice'"

        Args:
            var: Entity variable name (e.g., "$e", "$actor")

        Returns:
            List of AST patterns
        """
        from type_bridge.query.ast import (
            HasPattern,
            LiteralValue,
            ValueComparisonPattern,
        )

        attr_type_name = self.attr_type.get_attribute_name()
        attr_var = generate_attr_var(var, self.attr_type)

        # Map operation to TypeQL keyword
        # AUTOMATIC CONVERSION: regex() → 'like' in TypeQL
        typeql_op = "like" if self.operation == "regex" else self.operation

        return [
            HasPattern(thing_var=var, attr_type=attr_type_name, attr_var=attr_var),
            ValueComparisonPattern(
                var=attr_var,
                operator=typeql_op,
                value=LiteralValue(value=self.pattern.value, value_type="string"),
            ),
        ]
