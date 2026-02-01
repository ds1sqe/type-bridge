"""String-specific expressions for text filtering.

See :mod:`type_bridge.expressions.utils` for documentation on TypeDB 3.x
variable scoping and why we generate unique attribute variables.
"""

from typing import TYPE_CHECKING, Literal

from type_bridge.expressions.base import Expression
from type_bridge.expressions.utils import generate_has_pattern

if TYPE_CHECKING:
    from type_bridge.attribute.string import String


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

    def to_typeql(self, var: str) -> str:
        """
        Generate TypeQL pattern for this string operation.

        Example output: "$e has Name $e_name; $e_name contains 'Alice'"

        Args:
            var: Entity variable name (e.g., "$e", "$actor")

        Returns:
            TypeQL pattern string (without trailing semicolon)
        """
        # Generate unique attribute variable and 'has' pattern
        attr_var, has_pattern = generate_has_pattern(var, self.attr_type)

        # Escape and quote the pattern
        escaped_pattern = self.pattern.value.replace("\\", "\\\\").replace('"', '\\"')
        quoted_pattern = f'"{escaped_pattern}"'

        # Map operation to TypeQL keyword
        # AUTOMATIC CONVERSION: regex() → 'like' in TypeQL
        if self.operation == "regex":
            # TypeQL uses 'like' for regex patterns (both do the same thing)
            typeql_op = "like"
        else:
            typeql_op = self.operation

        # Generate full pattern (no trailing semicolon - QueryBuilder adds those)
        return f"{has_pattern}; {attr_var} {typeql_op} {quoted_pattern}"
