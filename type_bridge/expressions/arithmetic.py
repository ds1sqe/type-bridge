"""Arithmetic expressions for TypeQL queries.

TypeQL supports six infix operators with standard precedence:
+, -, *, /, %, ^

These can be used in let assignments and reduce clauses to perform
arithmetic on query variables and literals.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.query.ast import Pattern, Value

_VALID_OPERATORS = frozenset({"+", "-", "*", "/", "%", "^"})


def _normalize_operand(operand: str | int | float) -> str:
    """Normalize an operand to a string suitable for TypeQL.

    - Bare variable names (no $) get a $ prefix.
    - Numeric literals are converted to strings.
    - Already-prefixed variables pass through unchanged.
    """
    if isinstance(operand, (int, float)):
        return str(operand)
    if isinstance(operand, str) and not operand.startswith("$") and not operand[0:1].isdigit():
        return f"${operand}"
    return operand


@dataclass
class ArithmeticExpr(Expression):
    """Binary arithmetic expression for TypeQL queries.

    Represents an infix operation like ``$x + $y`` or ``$price * 1.1``.

    Example:
        >>> expr = ArithmeticExpr("$x", "+", "$y")
        >>> expr.to_typeql("$unused")
        '($x + $y)'

        >>> # In a let assignment:
        >>> # let $total = ($price * $quantity);
    """

    left: str
    operator: str
    right: str

    def __post_init__(self) -> None:
        if self.operator not in _VALID_OPERATORS:
            raise ValueError(
                f"Invalid arithmetic operator: {self.operator!r}. "
                f"Must be one of: {', '.join(sorted(_VALID_OPERATORS))}"
            )

    def to_ast(self, var: str) -> list[Pattern]:
        """Arithmetic expressions are values, not match patterns."""
        return []

    def to_value_ast(self, var: str | None = None) -> Value:
        """Convert to an ArithmeticValue AST node."""
        from type_bridge.query.ast import ArithmeticValue

        left: Value | str = self.left
        right: Value | str = self.right

        # If either operand is itself an ArithmeticExpr (nested), recurse
        # For now, operands are always strings (variables or literals)
        return ArithmeticValue(left=left, operator=self.operator, right=right)

    def to_typeql(self, var: str) -> str:
        """Generate parenthesized TypeQL arithmetic expression.

        Args:
            var: Ignored (arithmetic expressions use their own operands)

        Returns:
            Parenthesized TypeQL expression, e.g. "($x + $y)"
        """
        return f"({self.left} {self.operator} {self.right})"

    def get_attribute_types(self) -> set[type[Attribute]]:
        """Arithmetic expressions don't reference attribute types directly."""
        return set()

    def __repr__(self) -> str:
        return f"ArithmeticExpr({self.left!r}, {self.operator!r}, {self.right!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ArithmeticExpr):
            return NotImplemented
        return (
            self.left == other.left
            and self.operator == other.operator
            and self.right == other.right
        )

    def __hash__(self) -> int:
        return hash((self.left, self.operator, self.right))


# --- Helper functions ---


def add(left: str | int | float, right: str | int | float) -> ArithmeticExpr:
    """Create an addition expression.

    Args:
        left: Variable name or numeric literal
        right: Variable name or numeric literal

    Returns:
        ArithmeticExpr representing ``left + right``

    Example:
        >>> add("x", "y").to_typeql("")
        '($x + $y)'
        >>> add("price", 10).to_typeql("")
        '($price + 10)'
    """
    return ArithmeticExpr(_normalize_operand(left), "+", _normalize_operand(right))


def sub(left: str | int | float, right: str | int | float) -> ArithmeticExpr:
    """Create a subtraction expression.

    Args:
        left: Variable name or numeric literal
        right: Variable name or numeric literal

    Returns:
        ArithmeticExpr representing ``left - right``
    """
    return ArithmeticExpr(_normalize_operand(left), "-", _normalize_operand(right))


def mul(left: str | int | float, right: str | int | float) -> ArithmeticExpr:
    """Create a multiplication expression.

    Args:
        left: Variable name or numeric literal
        right: Variable name or numeric literal

    Returns:
        ArithmeticExpr representing ``left * right``
    """
    return ArithmeticExpr(_normalize_operand(left), "*", _normalize_operand(right))


def div(left: str | int | float, right: str | int | float) -> ArithmeticExpr:
    """Create a division expression.

    Args:
        left: Variable name or numeric literal
        right: Variable name or numeric literal

    Returns:
        ArithmeticExpr representing ``left / right``
    """
    return ArithmeticExpr(_normalize_operand(left), "/", _normalize_operand(right))


def mod(left: str | int | float, right: str | int | float) -> ArithmeticExpr:
    """Create a modulo expression.

    Args:
        left: Variable name or numeric literal
        right: Variable name or numeric literal

    Returns:
        ArithmeticExpr representing ``left % right``
    """
    return ArithmeticExpr(_normalize_operand(left), "%", _normalize_operand(right))


def pow_(left: str | int | float, right: str | int | float) -> ArithmeticExpr:
    """Create an exponentiation expression.

    Args:
        left: Variable name or numeric literal
        right: Variable name or numeric literal

    Returns:
        ArithmeticExpr representing ``left ^ right``
    """
    return ArithmeticExpr(_normalize_operand(left), "^", _normalize_operand(right))
