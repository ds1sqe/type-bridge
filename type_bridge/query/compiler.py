"""Compiler for converting AST nodes to TypeQL strings.

The Rust core renders every node. This module only marshals the Python AST
into the core's serde form.
"""

from __future__ import annotations

import math
from datetime import date, datetime, timedelta
from decimal import Decimal
from typing import Any

import isodate
from type_bridge_core import QueryCompiler as _CoreQueryCompiler

from type_bridge.query.ast import (
    ArithmeticValue,
    AttributePattern,
    Clause,
    Constraint,
    DeleteClause,
    DeleteThingStatement,
    EntityPattern,
    FetchAttribute,
    FetchAttributeList,
    FetchClause,
    FetchFunction,
    FetchItem,
    FetchNestedWildcard,
    FetchVariable,
    FetchWildcard,
    FunctionCallValue,
    HasConstraint,
    HasPattern,
    HasStatement,
    IidConstraint,
    IidPattern,
    InsertClause,
    IsaConstraint,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
    NotPattern,
    OrPattern,
    Pattern,
    QueryNode,
    RawPattern,
    RawStatement,
    ReduceAssignment,
    ReduceClause,
    RelationPattern,
    RelationStatement,
    Statement,
    SubTypePattern,
    UpdateClause,
    Value,
    ValueComparisonPattern,
)

_core_compiler = _CoreQueryCompiler()
_MIN_INTEGER = -(2**63)
_MAX_INTEGER = 2**63 - 1


def _literal_to_dict(literal: LiteralValue) -> dict[str, Any]:
    """Marshal a literal into the core's typed form.

    A ``str`` keeps its declared value type, so a parsed date or datetime
    renders unquoted again. Any other Python value is typed by its class,
    because callers attach a generic declared type (``"string"``) to dates,
    decimals, and durations.
    """
    value = literal.value
    if hasattr(value, "value"):
        # Attribute instances and enum members carry the literal in `.value`.
        value = value.value
    if isinstance(value, str):
        return {"value": value, "value_type": literal.value_type}
    if isinstance(value, bool):
        return {"value": value, "value_type": "boolean"}
    if isinstance(value, int):
        if not _MIN_INTEGER <= value <= _MAX_INTEGER:
            raise ValueError(f"TypeQL integer literals are signed 64-bit; got {value}")
        return {"value": value, "value_type": "long"}
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(f"TypeQL has no literal for the double {value}")
        return {"value": value, "value_type": "double"}
    if isinstance(value, Decimal):
        return {"value": str(value), "value_type": "decimal"}
    if isinstance(value, datetime):
        value_type = "datetime" if value.tzinfo is None else "datetime-tz"
        return {"value": value.isoformat(), "value_type": value_type}
    if isinstance(value, date):
        return {"value": value.isoformat(), "value_type": "date"}
    if isinstance(value, timedelta | isodate.Duration):
        return {"value": isodate.duration_isoformat(value), "value_type": "duration"}
    return {"value": str(value), "value_type": "string"}


def _value_to_dict(value: Value | str) -> dict[str, Any]:
    """Convert a Value AST node to a serde-compatible dict."""
    if isinstance(value, str):
        return {"type": "Variable", "data": value}
    if isinstance(value, LiteralValue):
        return {"type": "Literal", "data": _literal_to_dict(value)}
    if isinstance(value, FunctionCallValue):
        return {
            "type": "FunctionCall",
            "data": {
                "function": value.function,
                "args": [_value_to_dict(a) for a in value.args],
            },
        }
    if isinstance(value, ArithmeticValue):
        return {
            "type": "Arithmetic",
            "data": {
                "left": _value_to_dict(value.left),
                "operator": value.operator,
                "right": _value_to_dict(value.right),
            },
        }
    raise ValueError(f"Unknown value type: {type(value).__name__}")


def _constraint_to_dict(constraint: Constraint) -> dict[str, Any]:
    """Convert a Constraint AST node to a serde-compatible dict."""
    if isinstance(constraint, IidConstraint):
        return {"type": "Iid", "data": constraint.iid}
    if isinstance(constraint, HasConstraint):
        val = (
            _value_to_dict(constraint.value)
            if isinstance(constraint.value, Value)
            else {"type": "Variable", "data": constraint.value}
        )
        return {
            "type": "Has",
            "data": {"attr_name": constraint.attr_name, "value": val},
        }
    if isinstance(constraint, IsaConstraint):
        return {
            "type": "Isa",
            "data": {"type_name": constraint.type_name, "strict": constraint.strict},
        }
    raise ValueError(f"Unknown constraint type: {type(constraint).__name__}")


def _pattern_to_dict(pattern: Pattern) -> dict[str, Any]:
    """Convert a Pattern AST node to a serde-compatible dict."""
    if isinstance(pattern, EntityPattern):
        return {
            "type": "Entity",
            "data": {
                "variable": pattern.variable,
                "type_name": pattern.type_name,
                "constraints": [_constraint_to_dict(c) for c in pattern.constraints],
                "is_strict": pattern.is_strict,
            },
        }
    if isinstance(pattern, RelationPattern):
        return {
            "type": "Relation",
            "data": {
                "variable": pattern.variable,
                "type_name": pattern.type_name,
                "role_players": [
                    {"role": rp.role, "player_var": rp.player_var} for rp in pattern.role_players
                ],
                "constraints": [_constraint_to_dict(c) for c in pattern.constraints],
            },
        }
    if isinstance(pattern, SubTypePattern):
        return {
            "type": "SubType",
            "data": {"variable": pattern.variable, "parent_type": pattern.parent_type},
        }
    if isinstance(pattern, AttributePattern):
        return {
            "type": "Attribute",
            "data": {
                "variable": pattern.variable,
                "type_name": pattern.type_name,
                "value": _value_to_dict(pattern.value) if pattern.value is not None else None,
            },
        }
    if isinstance(pattern, HasPattern):
        return {
            "type": "Has",
            "data": {
                "thing_var": pattern.thing_var,
                "attr_type": pattern.attr_type,
                "attr_var": pattern.attr_var,
            },
        }
    if isinstance(pattern, ValueComparisonPattern):
        val = (
            _value_to_dict(pattern.value)
            if isinstance(pattern.value, Value)
            else {"type": "Variable", "data": pattern.value}
        )
        return {
            "type": "ValueComparison",
            "data": {"var": pattern.var, "operator": pattern.operator, "value": val},
        }
    if isinstance(pattern, NotPattern):
        return {"type": "Not", "data": [_pattern_to_dict(p) for p in pattern.patterns]}
    if isinstance(pattern, OrPattern):
        return {
            "type": "Or",
            "data": [[_pattern_to_dict(p) for p in alt] for alt in pattern.alternatives],
        }
    if isinstance(pattern, IidPattern):
        return {
            "type": "Iid",
            "data": {"variable": pattern.variable, "iid": pattern.iid},
        }
    if isinstance(pattern, RawPattern):
        return {"type": "Raw", "data": pattern.content}
    raise ValueError(f"Unknown pattern type: {type(pattern).__name__}")


def _statement_to_dict(stmt: Statement) -> dict[str, Any]:
    """Convert a Statement AST node to a serde-compatible dict."""
    if isinstance(stmt, HasStatement):
        return {
            "type": "Has",
            "data": {
                "subject_var": stmt.subject_var,
                "attr_name": stmt.attr_name,
                "value": _value_to_dict(stmt.value),
            },
        }
    if isinstance(stmt, IsaStatement):
        return {
            "type": "Isa",
            "data": {"variable": stmt.variable, "type_name": stmt.type_name},
        }
    if isinstance(stmt, RelationStatement):
        return {
            "type": "Relation",
            "data": {
                "variable": stmt.variable,
                "type_name": stmt.type_name,
                "role_players": [
                    {"role": rp.role, "player_var": rp.player_var} for rp in stmt.role_players
                ],
                "include_variable": stmt.include_variable,
                "attributes": [_statement_to_dict(a) for a in stmt.attributes],
            },
        }
    if isinstance(stmt, DeleteThingStatement):
        return {"type": "DeleteThing", "data": stmt.variable}
    if isinstance(stmt, RawStatement):
        return {"type": "Raw", "data": stmt.content}
    raise ValueError(f"Unknown statement type: {type(stmt).__name__}")


def _fetch_item_to_dict(item: FetchItem | str) -> dict[str, Any]:
    """Convert a FetchItem AST node to a serde-compatible dict (externally tagged)."""
    if isinstance(item, str):
        return {"Raw": item}
    if isinstance(item, FetchAttribute):
        return {"Attribute": {"key": item.key, "var": item.var, "attr_name": item.attr_name}}
    if isinstance(item, FetchVariable):
        return {"Variable": {"key": item.key, "var": item.var}}
    if isinstance(item, FetchAttributeList):
        return {"AttributeList": {"key": item.key, "var": item.var, "attr_name": item.attr_name}}
    if isinstance(item, FetchFunction):
        return {"Function": {"key": item.key, "func_name": item.func_name, "var": item.var}}
    if isinstance(item, FetchWildcard):
        return {"Wildcard": {"key": item.key, "var": item.var}}
    if isinstance(item, FetchNestedWildcard):
        return {"NestedWildcard": {"key": item.key, "var": item.var}}
    raise ValueError(f"Unknown fetch item type: {type(item).__name__}")


def _let_assignment_to_dict(assign: LetAssignment) -> dict[str, Any]:
    """Convert a LetAssignment to a serde-compatible dict."""
    expr = (
        _value_to_dict(assign.expression)
        if isinstance(assign.expression, Value)
        else {"type": "Variable", "data": assign.expression}
    )
    return {
        "variables": assign.variables,
        "expression": expr,
        "is_stream": assign.is_stream,
    }


def _reduce_assignment_to_dict(assign: ReduceAssignment) -> dict[str, Any]:
    """Convert a ReduceAssignment to a serde-compatible dict."""
    expr = (
        _value_to_dict(assign.expression)
        if isinstance(assign.expression, Value)
        else {"type": "Variable", "data": assign.expression}
    )
    return {"variable": assign.variable, "expression": expr}


def _clause_to_dict(clause: Clause) -> dict[str, Any]:
    """Convert a Clause AST node to a serde-compatible dict for Rust deserialization."""
    if isinstance(clause, MatchClause):
        return {"Match": [_pattern_to_dict(p) for p in clause.patterns]}
    if isinstance(clause, MatchLetClause):
        return {"MatchLet": [_let_assignment_to_dict(a) for a in clause.assignments]}
    if isinstance(clause, InsertClause):
        return {"Insert": [_statement_to_dict(s) for s in clause.statements]}
    if isinstance(clause, DeleteClause):
        return {"Delete": [_statement_to_dict(s) for s in clause.statements]}
    if isinstance(clause, UpdateClause):
        return {"Update": [_statement_to_dict(s) for s in clause.statements]}
    if isinstance(clause, FetchClause):
        return {"Fetch": [_fetch_item_to_dict(i) for i in clause.items]}
    if isinstance(clause, ReduceClause):
        return {
            "Reduce": {
                "assignments": [_reduce_assignment_to_dict(a) for a in clause.assignments],
                "group_by": clause.group_by,
            },
        }
    raise ValueError(f"Unknown clause type: {type(clause).__name__}")


def _node_to_dict(node: QueryNode) -> dict[str, Any]:
    """Wrap one node in the core's ``QueryNode`` form."""
    if isinstance(node, Clause):
        return {"Clause": _clause_to_dict(node)}
    if isinstance(node, Pattern):
        return {"Pattern": _pattern_to_dict(node)}
    if isinstance(node, Statement):
        return {"Statement": _statement_to_dict(node)}
    if isinstance(node, Constraint):
        return {"Constraint": _constraint_to_dict(node)}
    if isinstance(node, Value):
        return {"Value": _value_to_dict(node)}
    raise ValueError(f"Unknown node type: {type(node).__name__}")


class QueryCompiler:
    """Compiles AST nodes into TypeQL strings with the Rust core."""

    def compile(self, node: QueryNode) -> str:
        """Compile a single clause, pattern, statement, constraint, or value.

        Raises:
            ValueError: The node, or a value inside it, cannot be marshalled
                to the core.
        """
        return _core_compiler.compile_node(_node_to_dict(node))

    def compile_batch(self, nodes: list[QueryNode], operation: str = "") -> str:
        """Compile a list of nodes into a single query string.

        Args:
            nodes: Nodes to compile, one per line.
            operation: Optional operation keyword (e.g., "match", "insert") to
                prepend when the nodes are bare patterns or statements.
        """
        query = "\n".join(self.compile(node) for node in nodes)
        if operation:
            return f"{operation}\n{query}"
        return query
