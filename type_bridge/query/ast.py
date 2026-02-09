"""Abstract Syntax Tree (AST) for TypeQL queries.

This module defines the structure of TypeQL queries as a tree of typed objects,
decoupling query construction from string formatting.
"""

from __future__ import annotations

import logging
from abc import ABC
from dataclasses import dataclass, field
from typing import Any, Literal

logger = logging.getLogger(__name__)

# Base classes for typing and inheritance
@dataclass
class QueryNode(ABC):
    """Abstract base class for all AST nodes."""
    pass

@dataclass
class Value(QueryNode, ABC):
    """Abstract base class for values."""
    pass

@dataclass
class Constraint(QueryNode, ABC):
    """Abstract base class for constraints in a pattern."""
    pass

@dataclass
class Pattern(QueryNode, ABC):
    """Abstract base class for patterns in a match clause."""
    pass

@dataclass
class Statement(QueryNode, ABC):
    """Abstract base class for statements in insert/delete/update clauses."""
    pass

@dataclass
class Clause(QueryNode, ABC):
    """Abstract base class for top-level clauses."""
    pass

@dataclass
class FetchItem(QueryNode, ABC):
    """Abstract base class for fetch items."""
    key: str  # The output key name (e.g., "_iid", "name")


try:
    from type_bridge_core import (
        LiteralValue as _LiteralValue,
        FunctionCallValue as _FunctionCallValue,
        ArithmeticValue as _ArithmeticValue,
        RolePlayer as _RolePlayer,
        IidConstraint as _IidConstraint,
        HasConstraint as _HasConstraint,
        IsaConstraint as _IsaConstraint,
        EntityPattern as _EntityPattern,
        RelationPattern as _RelationPattern,
        SubTypePattern as _SubTypePattern,
        AttributePattern as _AttributePattern,
        HasPattern as _HasPattern,
        ValueComparisonPattern as _ValueComparisonPattern,
        NotPattern as _NotPattern,
        OrPattern as _OrPattern,
        IidPattern as _IidPattern,
        RawPattern as _RawPattern,
        HasStatement as _HasStatement,
        IsaStatement as _IsaStatement,
        RelationStatement as _RelationStatement,
        DeleteThingStatement as _DeleteThingStatement,
        RawStatement as _RawStatement,
        MatchClause as _MatchClause,
        MatchLetClause as _MatchLetClause,
        LetAssignment as _LetAssignment,
        InsertClause as _InsertClause,
        DeleteClause as _DeleteClause,
        UpdateClause as _UpdateClause,
        FetchAttribute as _FetchAttribute,
        FetchVariable as _FetchVariable,
        FetchAttributeList as _FetchAttributeList,
        FetchFunction as _FetchFunction,
        FetchWildcard as _FetchWildcard,
        FetchNestedWildcard as _FetchNestedWildcard,
        FetchClause as _FetchClause,
        AggregateExpr as _AggregateExpr,
        ReduceAssignment as _ReduceAssignment,
        ReduceClause as _ReduceClause,
    )
    _CORE_AVAILABLE = True
except ImportError:
    _CORE_AVAILABLE = False
    logger.debug("type_bridge_core not found, using Python AST implementation")

if _CORE_AVAILABLE:
    LiteralValue = _LiteralValue
    FunctionCallValue = _FunctionCallValue
    ArithmeticValue = _ArithmeticValue
    RolePlayer = _RolePlayer
    IidConstraint = _IidConstraint
    HasConstraint = _HasConstraint
    IsaConstraint = _IsaConstraint
    EntityPattern = _EntityPattern
    RelationPattern = _RelationPattern
    SubTypePattern = _SubTypePattern
    AttributePattern = _AttributePattern
    HasPattern = _HasPattern
    ValueComparisonPattern = _ValueComparisonPattern
    NotPattern = _NotPattern
    OrPattern = _OrPattern
    IidPattern = _IidPattern
    RawPattern = _RawPattern
    HasStatement = _HasStatement
    IsaStatement = _IsaStatement
    RelationStatement = _RelationStatement
    DeleteThingStatement = _DeleteThingStatement
    RawStatement = _RawStatement
    MatchClause = _MatchClause
    MatchLetClause = _MatchLetClause
    LetAssignment = _LetAssignment
    InsertClause = _InsertClause
    DeleteClause = _DeleteClause
    UpdateClause = _UpdateClause
    FetchAttribute = _FetchAttribute
    FetchVariable = _FetchVariable
    FetchAttributeList = _FetchAttributeList
    FetchFunction = _FetchFunction
    FetchWildcard = _FetchWildcard
    FetchNestedWildcard = _FetchNestedWildcard
    FetchClause = _FetchClause
    AggregateExpr = _AggregateExpr
    ReduceAssignment = _ReduceAssignment
    ReduceClause = _ReduceClause

    # Register Rust classes with ABCs for isinstance checks
    Value.register(LiteralValue)
    Value.register(FunctionCallValue)
    Value.register(ArithmeticValue)
    
    Constraint.register(IidConstraint)
    Constraint.register(HasConstraint)
    Constraint.register(IsaConstraint)
    
    Pattern.register(EntityPattern)
    Pattern.register(RelationPattern)
    Pattern.register(SubTypePattern)
    Pattern.register(AttributePattern)
    Pattern.register(HasPattern)
    Pattern.register(ValueComparisonPattern)
    Pattern.register(NotPattern)
    Pattern.register(OrPattern)
    Pattern.register(IidPattern)
    Pattern.register(RawPattern)
    
    Statement.register(HasStatement)
    Statement.register(IsaStatement)
    Statement.register(RelationStatement)
    Statement.register(DeleteThingStatement)
    Statement.register(RawStatement)
    
    Clause.register(MatchClause)
    Clause.register(MatchLetClause)
    Clause.register(InsertClause)
    Clause.register(DeleteClause)
    Clause.register(UpdateClause)
    Clause.register(FetchClause)
    Clause.register(ReduceClause)
    
    FetchItem.register(FetchAttribute)
    FetchItem.register(FetchVariable)
    FetchItem.register(FetchAttributeList)
    FetchItem.register(FetchFunction)
    FetchItem.register(FetchWildcard)
    FetchItem.register(FetchNestedWildcard)

else:
    @dataclass
    class FunctionCallValue(Value):
        """Value representing a function call (e.g. iid($x))."""
        function: str
        args: list[Value | str]

    @dataclass
    class LiteralValue(Value):
        """A literal value (string, number, boolean, date, etc.)."""
        value: Any
        value_type: Literal["string", "long", "double", "boolean", "datetime", "datetime-tz", "date"]

    @dataclass
    class ArithmeticValue(Value):
        """A binary arithmetic operation that is itself a Value."""
        left: Value | str
        operator: str
        right: Value | str

    @dataclass
    class RolePlayer(QueryNode):
        """A role player in a relation."""
        role: str
        player_var: str

    @dataclass
    class IidConstraint(Constraint):
        """Constraint matching by IID."""
        iid: str

    @dataclass
    class HasConstraint(Constraint):
        """Constraint checking for an attribute value."""
        attr_name: str
        value: Value | str

    @dataclass
    class IsaConstraint(Constraint):
        """Constraint checking for type inheritance."""
        type_name: str
        strict: bool = False

    @dataclass
    class EntityPattern(Pattern):
        """Pattern matching an entity."""
        variable: str
        type_name: str
        constraints: list[Constraint] = field(default_factory=list)
        is_strict: bool = False

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
        value: Value | None = None

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
        value: Value | str

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
        """A raw string pattern."""
        content: str

    @dataclass
    class RawStatement(Statement):
        """A raw string statement."""
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
        """Statement defining a relation and its role players."""
        variable: str
        type_name: str
        role_players: list[RolePlayer]
        include_variable: bool = True
        attributes: list[HasStatement] = field(default_factory=list)

    @dataclass
    class DeleteThingStatement(Statement):
        """Statement deleting a thing instance."""
        variable: str

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
        """Assignment in a match let clause."""
        variables: list[str]
        expression: Value | str
        is_stream: bool = False

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
    class FetchAttribute(FetchItem):
        """Fetch a single attribute value."""
        var: str
        attr_name: str

    @dataclass
    class FetchVariable(FetchItem):
        """Fetch a variable directly."""
        var: str

    @dataclass
    class FetchAttributeList(FetchItem):
        """Fetch a multi-value attribute as a list."""
        var: str
        attr_name: str

    @dataclass
    class FetchFunction(FetchItem):
        """Fetch a function result."""
        func_name: str
        var: str

    @dataclass
    class FetchWildcard(FetchItem):
        """Fetch all attributes of a variable."""
        var: str

    @dataclass
    class FetchNestedWildcard(FetchItem):
        """Fetch all attributes of a variable in a nested object."""
        var: str

    @dataclass
    class FetchClause(Clause):
        """A fetch clause defining output structure."""
        items: list[FetchItem | str] = field(default_factory=list)

    @dataclass
    class AggregateExpr(QueryNode):
        """An aggregate expression like count($var) or sum($attr)."""
        func_name: str
        var: str
        attr_name: str | None = None

    @dataclass
    class ReduceAssignment(QueryNode):
        """Assignment in reduce clause."""
        variable: str
        expression: Value | str

    @dataclass
    class ReduceClause(Clause):
        """A reduce clause for aggregations."""
        assignments: list[ReduceAssignment]
        group_by: str | None = None
