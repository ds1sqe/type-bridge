"""Abstract Syntax Tree (AST) for TypeQL queries.

This module defines the structure of TypeQL queries as a tree of typed objects,
decoupling query construction from string formatting.
"""

from __future__ import annotations

from abc import ABC
from dataclasses import dataclass, field
from typing import Any, Literal


@dataclass
class QueryNode(ABC):
    """Abstract base class for all AST nodes."""

    pass


@dataclass
class Value(QueryNode, ABC):
    """Abstract base class for values."""

    pass


@dataclass
class LiteralValue(Value):
    """A literal value (string, number, boolean, date, etc.)."""

    value: Any
    value_type: Literal["string", "long", "double", "boolean", "datetime", "date"]


@dataclass
class RolePlayer(QueryNode):
    """A role player in a relation."""

    role: str
    player_var: str  # The variable name of the player (e.g., "$p")


@dataclass
class Constraint(QueryNode, ABC):
    """Abstract base class for constraints in a pattern."""

    pass


@dataclass
class IidConstraint(Constraint):
    """Constraint matching by IID."""

    iid: str


@dataclass
class HasConstraint(Constraint):
    """Constraint checking for an attribute value."""

    attr_name: str
    value: Value | str  # Literal value or variable name


@dataclass
class IsaConstraint(Constraint):
    """Constraint checking for type inheritance."""

    type_name: str
    strict: bool = False  # If True, uses 'isa!'


@dataclass
class Pattern(QueryNode, ABC):
    """Abstract base class for patterns in a match clause."""

    variable: str


@dataclass
class EntityPattern(Pattern):
    """Pattern matching an entity."""

    type_name: str
    constraints: list[Constraint] = field(default_factory=list)


@dataclass
class RelationPattern(Pattern):
    """Pattern matching a relation."""

    type_name: str
    role_players: list[RolePlayer] = field(default_factory=list)
    constraints: list[Constraint] = field(default_factory=list)


@dataclass
class AttributePattern(Pattern):
    """Pattern matching an attribute explicitly."""

    type_name: str
    value: Value | None = None  # None if just matching the attribute type


@dataclass
class Statement(QueryNode, ABC):
    """Abstract base class for statements in insert/delete/update clauses."""

    pass


@dataclass
class HasStatement(Statement):
    """Statement assigning an attribute value."""

    subject_var: str
    attr_name: str
    value: Value


@dataclass
class IsaStatement(Statement):
    """Statement defining the type of a variable."""

    variable: str
    type_name: str


@dataclass
class RelationStatement(Statement):
    """Statement defining a relation and its role players."""

    variable: str
    type_name: str
    role_players: list[RolePlayer]


@dataclass
class DeleteThingStatement(Statement):
    """Statement deleting a thing (entity/relation) instance."""

    variable: str


@dataclass
class Clause(QueryNode, ABC):
    """Abstract base class for top-level clauses."""

    pass


@dataclass
class MatchClause(Clause):
    """A match clause containing patterns."""

    patterns: list[Pattern]


@dataclass
class InsertClause(Clause):
    """An insert clause containing statements."""

    statements: list[Statement]


@dataclass
class DeleteClause(Clause):
    """A delete clause containing statements."""

    statements: list[Statement]


@dataclass
class UpdateClause(Clause):
    """An update clause containing statements."""

    statements: list[Statement]


@dataclass
class FetchClause(Clause):
    """A fetch clause defining output structure."""

    items: list[str]  # e.g., ["$x", "$y: name"]
