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
class FunctionCallValue(Value):
    """Value representing a function call (e.g. iid($x))."""

    function: str
    args: list[Value | str]


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

    pass


@dataclass
class EntityPattern(Pattern):
    """Pattern matching an entity."""

    variable: str
    type_name: str
    constraints: list[Constraint] = field(default_factory=list)
    is_strict: bool = False  # If True, uses 'isa!'


@dataclass
class RelationPattern(Pattern):
    """Pattern matching a relation."""

    variable: str
    type_name: str
    role_players: list[RolePlayer] = field(default_factory=list)
    constraints: list[Constraint] = field(default_factory=list)


@dataclass
class SubTypePattern(Pattern):
    """Pattern checking for type inheritance ($t sub type)."""

    variable: str
    parent_type: str


@dataclass
class AttributePattern(Pattern):
    """Pattern matching an attribute explicitly."""

    variable: str
    type_name: str
    value: Value | None = None  # None if just matching the attribute type


@dataclass
class HasPattern(Pattern):
    """Pattern for variable has attribute assignment ($x has Type $v)."""

    thing_var: str
    attr_type: str
    attr_var: str


@dataclass
class ValueComparisonPattern(Pattern):
    """Pattern for value comparison ($v > 10)."""

    var: str
    operator: str
    value: Value | str  # Literal or variable


@dataclass
class NotPattern(Pattern):
    """Pattern for negation (not { ... })."""

    patterns: list[Pattern]


@dataclass
class OrPattern(Pattern):
    """Pattern for disjunction ({ ... } or { ... })."""

    alternatives: list[list[Pattern]]


@dataclass
class IidPattern(Pattern):
    """Pattern for IID match ($x iid 0x...)."""

    variable: str
    iid: str


@dataclass
class RawPattern(Pattern):
    """A raw string pattern (legacy support)."""

    content: str


@dataclass
class Statement(QueryNode, ABC):
    """Abstract base class for statements in insert/delete/update clauses."""

    pass


@dataclass
class RawStatement(Statement):
    """A raw string statement (legacy support)."""

    content: str


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
class MatchLetClause(Clause):
    """A match let clause."""

    assignments: list[LetAssignment]


@dataclass
class LetAssignment(QueryNode):
    """Assignment in a match let clause.

    Can be:
    - $x = func() (single value)
    - $x in func() (stream)
    """

    variables: list[str]  # e.g. ["$x"] or ["$x", "$y"]
    expression: Value | str  # Function call or expression string
    is_stream: bool = False  # If True, use 'in', else '='


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
class FetchVariable(FetchItem):
    """Fetch a variable directly.

    Generates: "key": $var
    """

    var: str


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
    """Assignment in reduce clause ($x = sum($v))."""

    variable: str
    expression: Value | str


@dataclass
class ReduceClause(Clause):
    """A reduce clause for aggregations.

    Generates: reduce $count = count($var);
    Or with groupby: reduce $count = count($var) groupby $group;
    """

    assignments: list[ReduceAssignment]
    group_by: str | None = None  # Variable to group by
