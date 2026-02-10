"""Parser for converting TypeQL strings back to AST nodes.

Uses the Rust ``type_bridge_core.parse_typeql_query`` function, then converts
the serde-tagged dicts into the Python AST dataclasses defined in
``type_bridge.query.ast``.
"""

from __future__ import annotations

import logging
from typing import Any

from type_bridge.query.ast import (
    ArithmeticValue,
    AttributePattern,
    Clause,
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
    RawPattern,
    RawStatement,
    ReduceAssignment,
    ReduceClause,
    RelationPattern,
    RelationStatement,
    RolePlayer,
    Statement,
    SubTypePattern,
    UpdateClause,
    Value,
    ValueComparisonPattern,
)

logger = logging.getLogger(__name__)

try:
    from type_bridge_core import (
        parse_typeql_query as _rust_parse_typeql_query,  # type: ignore[import-not-found]
    )

    RUST_AVAILABLE = True
except ImportError:
    RUST_AVAILABLE = False
    _rust_parse_typeql_query = None


# ---------------------------------------------------------------------------
# Dict -> AST converters (reverse of _*_to_dict in compiler.py)
# ---------------------------------------------------------------------------


def _dict_to_value(d: dict[str, Any]) -> Value | str:
    """Convert a serde-tagged Value dict to a Python AST Value or variable string."""
    tag = d["type"]
    data = d["data"]

    if tag == "Variable":
        return data  # str
    if tag == "Literal":
        return LiteralValue(value=data["value"], value_type=data["value_type"])
    if tag == "FunctionCall":
        return FunctionCallValue(
            function=data["function"],
            args=[_dict_to_value(a) for a in data["args"]],
        )
    if tag == "Arithmetic":
        return ArithmeticValue(
            left=_dict_to_value(data["left"]),
            operator=data["operator"],
            right=_dict_to_value(data["right"]),
        )
    raise ValueError(f"Unknown Value tag: {tag}")


def _dict_to_value_strict(d: dict[str, Any]) -> Value:
    """Convert a serde-tagged Value dict to a Python AST Value.

    Same as ``_dict_to_value`` but raises if the result is a bare variable
    string (used where the Python AST field type is ``Value``, not
    ``Value | str``).
    """
    result = _dict_to_value(d)
    if isinstance(result, str):
        # Wrap variable string in a FunctionCallValue-like marker.
        # This shouldn't happen in practice for the fields that call this,
        # but ensures type correctness.
        raise ValueError(f"Expected a Value node but got variable string: {result}")
    return result


def _dict_to_constraint(d: dict[str, Any]) -> HasConstraint | IidConstraint | IsaConstraint:
    """Convert a serde-tagged Constraint dict to a Python AST Constraint."""
    tag = d["type"]
    data = d["data"]

    if tag == "Iid":
        return IidConstraint(iid=data)
    if tag == "Has":
        return HasConstraint(
            attr_name=data["attr_name"],
            value=_dict_to_value(data["value"]),
        )
    if tag == "Isa":
        return IsaConstraint(type_name=data["type_name"], strict=data["strict"])
    raise ValueError(f"Unknown Constraint tag: {tag}")


def _dict_to_role_player(d: dict[str, Any]) -> RolePlayer:
    """Convert a RolePlayer dict to a Python AST RolePlayer."""
    return RolePlayer(role=d["role"], player_var=d["player_var"])


def _dict_to_pattern(d: dict[str, Any]) -> Pattern:
    """Convert a serde-tagged Pattern dict to a Python AST Pattern."""
    tag = d["type"]
    data = d["data"]

    if tag == "Entity":
        return EntityPattern(
            variable=data["variable"],
            type_name=data["type_name"],
            constraints=[_dict_to_constraint(c) for c in data["constraints"]],
            is_strict=data["is_strict"],
        )
    if tag == "Relation":
        return RelationPattern(
            variable=data["variable"],
            type_name=data["type_name"],
            role_players=[_dict_to_role_player(rp) for rp in data["role_players"]],
            constraints=[_dict_to_constraint(c) for c in data["constraints"]],
        )
    if tag == "SubType":
        return SubTypePattern(variable=data["variable"], parent_type=data["parent_type"])
    if tag == "Attribute":
        raw_val = data["value"]
        attr_value: Value | None = _dict_to_value_strict(raw_val) if raw_val is not None else None
        return AttributePattern(
            variable=data["variable"],
            type_name=data["type_name"],
            value=attr_value,
        )
    if tag == "Has":
        return HasPattern(
            thing_var=data["thing_var"],
            attr_type=data["attr_type"],
            attr_var=data["attr_var"],
        )
    if tag == "ValueComparison":
        return ValueComparisonPattern(
            var=data["var"],
            operator=data["operator"],
            value=_dict_to_value(data["value"]),
        )
    if tag == "Not":
        return NotPattern(patterns=[_dict_to_pattern(p) for p in data])
    if tag == "Or":
        return OrPattern(
            alternatives=[[_dict_to_pattern(p) for p in alt] for alt in data],
        )
    if tag == "Iid":
        return IidPattern(variable=data["variable"], iid=data["iid"])
    if tag == "Raw":
        return RawPattern(content=data)
    raise ValueError(f"Unknown Pattern tag: {tag}")


def _dict_to_statement(d: dict[str, Any]) -> Statement:
    """Convert a serde-tagged Statement dict to a Python AST Statement."""
    tag = d["type"]
    data = d["data"]

    if tag == "Has":
        return HasStatement(
            subject_var=data["subject_var"],
            attr_name=data["attr_name"],
            value=_dict_to_value_strict(data["value"]),
        )
    if tag == "Isa":
        return IsaStatement(variable=data["variable"], type_name=data["type_name"])
    if tag == "Relation":
        attrs: list[HasStatement] = []
        for a in data["attributes"]:
            attr_stmt = _dict_to_statement(a)
            if not isinstance(attr_stmt, HasStatement):
                raise ValueError(
                    f"Expected HasStatement in Relation attributes, got {type(attr_stmt)}"
                )
            attrs.append(attr_stmt)
        return RelationStatement(
            variable=data["variable"],
            type_name=data["type_name"],
            role_players=[_dict_to_role_player(rp) for rp in data["role_players"]],
            include_variable=data["include_variable"],
            attributes=attrs,
        )
    if tag == "DeleteThing":
        return DeleteThingStatement(variable=data)
    if tag == "Raw":
        return RawStatement(content=data)
    raise ValueError(f"Unknown Statement tag: {tag}")


def _dict_to_fetch_item(d: dict[str, Any]) -> FetchItem:
    """Convert an externally-tagged FetchItem dict to a Python AST FetchItem."""
    # FetchItem uses externally-tagged serde format: { "Variant": { ... } }
    if "Attribute" in d:
        data = d["Attribute"]
        return FetchAttribute(key=data["key"], var=data["var"], attr_name=data["attr_name"])
    if "Variable" in d:
        data = d["Variable"]
        return FetchVariable(key=data["key"], var=data["var"])
    if "AttributeList" in d:
        data = d["AttributeList"]
        return FetchAttributeList(key=data["key"], var=data["var"], attr_name=data["attr_name"])
    if "Function" in d:
        data = d["Function"]
        return FetchFunction(key=data["key"], func_name=data["func_name"], var=data["var"])
    if "Wildcard" in d:
        data = d["Wildcard"]
        return FetchWildcard(key=data["key"], var=data["var"])
    if "NestedWildcard" in d:
        data = d["NestedWildcard"]
        return FetchNestedWildcard(key=data["key"], var=data["var"])
    raise ValueError(f"Unknown FetchItem dict: {d}")


def _dict_to_let_assignment(d: dict[str, Any]) -> LetAssignment:
    """Convert a LetAssignment dict to a Python AST LetAssignment."""
    expr = _dict_to_value(d["expression"])
    return LetAssignment(
        variables=d["variables"],
        expression=expr,
        is_stream=d["is_stream"],
    )


def _dict_to_reduce_assignment(d: dict[str, Any]) -> ReduceAssignment:
    """Convert a ReduceAssignment dict to a Python AST ReduceAssignment."""
    expr = _dict_to_value(d["expression"])
    return ReduceAssignment(variable=d["variable"], expression=expr)


def _dict_to_clause(d: dict[str, Any]) -> Clause:
    """Convert an externally-tagged Clause dict to a Python AST Clause."""
    # Clause uses externally-tagged serde format: { "Match": [...] }
    if "Match" in d:
        return MatchClause(patterns=[_dict_to_pattern(p) for p in d["Match"]])
    if "MatchLet" in d:
        return MatchLetClause(
            assignments=[_dict_to_let_assignment(a) for a in d["MatchLet"]],
        )
    if "Insert" in d:
        return InsertClause(statements=[_dict_to_statement(s) for s in d["Insert"]])
    if "Delete" in d:
        return DeleteClause(statements=[_dict_to_statement(s) for s in d["Delete"]])
    if "Update" in d:
        return UpdateClause(statements=[_dict_to_statement(s) for s in d["Update"]])
    if "Fetch" in d:
        return FetchClause(items=[_dict_to_fetch_item(i) for i in d["Fetch"]])
    if "Reduce" in d:
        data = d["Reduce"]
        return ReduceClause(
            assignments=[_dict_to_reduce_assignment(a) for a in data["assignments"]],
            group_by=data.get("group_by"),
        )
    raise ValueError(f"Unknown Clause dict: {d}")


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


def parse_typeql_query(input: str) -> list[Clause]:
    """Parse a TypeQL query string into a list of Clause AST nodes.

    Uses the Rust ``type_bridge_core`` parser for performance and correctness.

    Args:
        input: A TypeQL data-manipulation query string.

    Returns:
        List of Clause AST nodes (MatchClause, InsertClause, etc.)

    Raises:
        ValueError: If the query cannot be parsed.
        NotImplementedError: If the Rust extension is not available.
    """
    if not RUST_AVAILABLE or _rust_parse_typeql_query is None:
        raise NotImplementedError(
            "parse_typeql_query requires the type_bridge_core Rust extension. "
            "Install it with: maturin develop --manifest-path type-bridge-core/Cargo.toml"
        )

    # Rust returns list[dict] in serde-tagged-enum format
    clause_dicts: list[dict[str, Any]] = _rust_parse_typeql_query(input)
    return [_dict_to_clause(d) for d in clause_dicts]
