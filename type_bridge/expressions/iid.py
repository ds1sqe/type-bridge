"""IID (Internal ID) expressions for TypeDB queries."""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import _QueryAttribute as Attribute
    from type_bridge.query.ast import Pattern

# IID format: 0x followed by hex digits
IID_PATTERN = re.compile(r"^0x[0-9a-fA-F]+$")


def validate_iid(iid: str) -> None:
    """Validate IID format.

    Args:
        iid: IID string to validate

    Raises:
        ValueError: If IID format is invalid
    """
    if not IID_PATTERN.match(iid):
        raise ValueError(f"Invalid IID format: '{iid}'. Expected format: 0x followed by hex digits")


class IidExpr(Expression):
    """Expression for matching entity by IID (Internal ID).

    Generates TypeQL pattern: `$var iid 0x...`

    Example:
        expr = IidExpr("0x1a2b3c4d")
        expr.to_typeql("$e")  # -> "$e iid 0x1a2b3c4d"
    """

    def __init__(self, iid: str):
        """Create an IID expression.

        Args:
            iid: TypeDB internal ID (format: 0x followed by hex digits)

        Raises:
            ValueError: If IID format is invalid
        """
        validate_iid(iid)
        self.iid = iid

    def to_ast(self, var: str) -> list[Pattern]:
        """Generate AST patterns for IID match.

        Returns:
            List containing one IidPattern.
        """
        from type_bridge.query.ast import IidPattern

        return [IidPattern(variable=var, iid=self.iid)]

    def get_attribute_types(self) -> set[type[Attribute]]:
        """IID is not an attribute, so return empty set."""
        return set()

    def __repr__(self) -> str:
        return f"IidExpr({self.iid!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, IidExpr):
            return NotImplemented
        return self.iid == other.iid

    def __hash__(self) -> int:
        return hash(self.iid)
