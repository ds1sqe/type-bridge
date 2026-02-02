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
    """Statement defining a relation and its role players.

    For TypeDB 3.x inserts, relations don't use a variable prefix.
    Set include_variable=False for insert statements.

    Attributes can be included inline for insert statements where the variable
    is not used (TypeDB 3.x: (role: $player) isa relation, has attr value;).
    """

    variable: str
    type_name: str
    role_players: list[RolePlayer]
    include_variable: bool = True  # False for insert statements in TypeDB 3.x
    attributes: list[HasStatement] = field(default_factory=list)  # Inline attributes for insert


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
class FetchItem(QueryNode, ABC):
    """Abstract base class for fetch items."""

    key: str  # The output key name (e.g., "_iid", "name")


@dataclass
class FetchAttribute(FetchItem):
    """Fetch a single attribute value.

    Generates: "key": $var.attr_name
    """

    var: str
    attr_name: str


@dataclass
class FetchAttributeList(FetchItem):
    """Fetch a multi-value attribute as a list.

    Generates: "key": [$var.attr_name]
    """

    var: str
    attr_name: str


@dataclass
class FetchFunction(FetchItem):
    """Fetch a function result.

    Generates: "key": func($var)
    Examples: iid($var), label($t)
    """

    func_name: str
    var: str


@dataclass
class FetchWildcard(FetchItem):
    """Fetch all attributes of a variable.

    Generates: "key": $var.*
    """

    var: str


@dataclass
class FetchClause(Clause):
    """A fetch clause defining output structure.

    Can contain either typed FetchItems or raw strings for backwards compatibility.
    """

    items: list[FetchItem | str] = field(default_factory=list)


@dataclass
class AggregateExpr(QueryNode):
    """An aggregate expression like count($var) or sum($attr).

    Generates: function($var) or function($var.attr)
    """

    func_name: str  # count, sum, min, max, mean, std, median
    var: str
    attr_name: str | None = None  # If None, aggregates the variable itself


@dataclass
class ReduceAssignment(QueryNode):
    """A reduce assignment like $count = count($var).

    Generates: $result_var = function($var)
    """

    result_var: str
    aggregate: AggregateExpr


@dataclass
class ReduceClause(Clause):
    """A reduce clause for aggregations.

    Generates: reduce $count = count($var);
    Or with groupby: reduce $count = count($var) groupby $group;
    """

    assignments: list[ReduceAssignment]
    group_by: str | None = None  # Variable to group by
