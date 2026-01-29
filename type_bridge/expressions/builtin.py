"""Built-in TypeQL function expressions.

TypeQL 3.8.0 introduced built-in functions that can be used in expressions:
- iid($var) - Get the internal ID of a thing
- label($var) - Get the type label of a thing
- abs($num) - Absolute value
- ceil($num) - Round up to nearest integer
- floor($num) - Round down to nearest integer
- round($num) - Round to nearest integer
- len($list) - Length of a list
- max($a, $b, ...) - Maximum value
- min($a, $b, ...) - Minimum value
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute


@dataclass
class BuiltinFunctionExpr(Expression):
    """Expression for TypeQL built-in functions.

    Built-in functions can be used in fetch clauses, let assignments,
    and other expression contexts.

    Example:
        >>> expr = BuiltinFunctionExpr("iid", "$e")
        >>> expr.to_typeql("$e")
        'iid($e)'

        >>> # In a fetch clause:
        >>> # fetch { "_iid": iid($e), "_type": label($e), $e.* }
    """

    name: str
    args: tuple[str, ...]

    def __init__(self, name: str, *args: str):
        """Create a built-in function expression.

        Args:
            name: Function name (iid, label, abs, ceil, floor, round, len, max, min)
            *args: Variable names or literal values as arguments
        """
        self.name = name
        self.args = args

    def to_typeql(self, var: str) -> str:
        """Generate TypeQL function call syntax.

        Args:
            var: Ignored for built-in functions (args are explicit)

        Returns:
            TypeQL function call like "iid($e)" or "max($a, $b)"
        """
        return f"{self.name}({', '.join(self.args)})"

    def get_attribute_types(self) -> set[type[Attribute]]:
        """Built-in functions don't reference attribute types."""
        return set()

    def __repr__(self) -> str:
        args_str = ", ".join(repr(a) for a in self.args)
        return f"BuiltinFunctionExpr({self.name!r}, {args_str})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, BuiltinFunctionExpr):
            return NotImplemented
        return self.name == other.name and self.args == other.args

    def __hash__(self) -> int:
        return hash((self.name, self.args))


def iid(var: str) -> BuiltinFunctionExpr:
    """Get the internal ID of a thing.

    Args:
        var: Variable name (e.g., "$e" or "e")

    Returns:
        Expression that generates "iid($var)"

    Example:
        >>> iid("$e").to_typeql("")
        'iid($e)'

        >>> # Use in fetch: fetch { "_iid": iid($e) }
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("iid", var)


def label(var: str) -> BuiltinFunctionExpr:
    """Get the type label of a thing.

    Args:
        var: Variable name (e.g., "$e" or "e")

    Returns:
        Expression that generates "label($var)"

    Example:
        >>> label("$e").to_typeql("")
        'label($e)'

        >>> # Use in fetch: fetch { "_type": label($e) }
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("label", var)


def abs_(var: str) -> BuiltinFunctionExpr:
    """Get the absolute value of a number.

    Args:
        var: Variable name or numeric expression

    Returns:
        Expression that generates "abs($var)"
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("abs", var)


def ceil(var: str) -> BuiltinFunctionExpr:
    """Round up to nearest integer.

    Args:
        var: Variable name or numeric expression

    Returns:
        Expression that generates "ceil($var)"
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("ceil", var)


def floor(var: str) -> BuiltinFunctionExpr:
    """Round down to nearest integer.

    Args:
        var: Variable name or numeric expression

    Returns:
        Expression that generates "floor($var)"
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("floor", var)


def round_(var: str) -> BuiltinFunctionExpr:
    """Round to nearest integer.

    Args:
        var: Variable name or numeric expression

    Returns:
        Expression that generates "round($var)"
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("round", var)


def len_(var: str) -> BuiltinFunctionExpr:
    """Get length of a list.

    Args:
        var: Variable name of a list

    Returns:
        Expression that generates "len($var)"
    """
    if not var.startswith("$"):
        var = f"${var}"
    return BuiltinFunctionExpr("len", var)


def max_(*args: str) -> BuiltinFunctionExpr:
    """Get maximum value.

    Args:
        *args: Variable names or values to compare

    Returns:
        Expression that generates "max($a, $b, ...)"
    """
    normalized = tuple(f"${a}" if not a.startswith("$") else a for a in args)
    return BuiltinFunctionExpr("max", *normalized)


def min_(*args: str) -> BuiltinFunctionExpr:
    """Get minimum value.

    Args:
        *args: Variable names or values to compare

    Returns:
        Expression that generates "min($a, $b, ...)"
    """
    normalized = tuple(f"${a}" if not a.startswith("$") else a for a in args)
    return BuiltinFunctionExpr("min", *normalized)
