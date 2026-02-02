"""Base expression class for TypeQL query building."""

from abc import ABC, abstractmethod
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.expressions.boolean import BooleanExpr
    from type_bridge.query.ast import Pattern, Value


class Expression(ABC):
    """
    Base class for all query expressions.

    Expressions represent query constraints that can be composed using
    boolean operators and converted to TypeQL patterns.
    """

    @abstractmethod
    def to_ast(self, var: str) -> list["Pattern"]:
        """
        Convert this expression to AST patterns.

        Args:
            var: The variable name to use in the pattern (e.g., "$e")

        Returns:
            List of AST patterns
        """
        ...

    def to_value_ast(self) -> "Value":
        """
        Convert this expression to an AST Value (if applicable).

        Most expressions are patterns (filters) and cannot be converted to values.
        FunctionCall expressions and Literal wrappers are values.

        Raises:
            NotImplementedError: If expression cannot be converted to a value.
        """
        raise NotImplementedError(f"{type(self).__name__} cannot be converted to an AST Value.")

    def to_typeql(self, var: str) -> str:
        """Deprecated: Use to_ast() instead."""
        from type_bridge.query.compiler import QueryCompiler

        compiler = QueryCompiler()
        patterns = self.to_ast(var)
        # We need to compile each pattern and join them?
        # Compiler has _compile_pattern.
        # But _compile_pattern is private.
        # compiler.compile() takes a Clause.
        # We can create a temporary MatchClause?
        from type_bridge.query.ast import MatchClause

        return compiler.compile(MatchClause(patterns=patterns)).replace("match\n", "").strip(";")

    def get_attribute_types(self) -> set[type["Attribute"]]:
        """
        Get all attribute types referenced by this expression.

        Returns:
            Set of attribute types used in this expression

        Note:
            Default implementation returns attr_type if present.
            BooleanExpr overrides to recursively collect from operands.
        """
        # Check if this expression has attr_type attribute
        attr_type = getattr(self, "attr_type", None)
        if attr_type is not None:
            return {attr_type}
        return set()

    def and_(self, other: "Expression") -> "BooleanExpr":
        """
        Combine this expression with another using AND logic.

        Args:
            other: Another expression to AND with this one

        Returns:
            BooleanExpr representing the conjunction
        """
        from type_bridge.expressions.boolean import BooleanExpr

        return BooleanExpr(operation="and", operands=[self, other])

    def or_(self, other: "Expression") -> "BooleanExpr":
        """
        Combine this expression with another using OR logic.

        Args:
            other: Another expression to OR with this one

        Returns:
            BooleanExpr representing the disjunction
        """
        from type_bridge.expressions.boolean import BooleanExpr

        return BooleanExpr(operation="or", operands=[self, other])

    def not_(self) -> "BooleanExpr":
        """
        Negate this expression using NOT logic.

        Returns:
            BooleanExpr representing the negation
        """
        from type_bridge.expressions.boolean import BooleanExpr

        return BooleanExpr(operation="not", operands=[self])
