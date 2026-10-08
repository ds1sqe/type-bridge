"""The answer-policy defaults Python reports are the core's."""

from __future__ import annotations

import type_bridge_core
from type_bridge_core import QueryExecutionResourceLimits

_DIMENSIONS = (
    "timeout_milliseconds",
    "items",
    "bytes",
    "graph_nodes",
    "attribute_values",
    "collection_members",
    "role_players",
    "statements",
)


def _core_defaults() -> dict[str, int]:
    # Read from the extension's exports of the orm constants, never literals,
    # so a changed core default changes what Python reports.
    return {
        name: getattr(type_bridge_core, f"DEFAULT_QUERY_{name.upper()}") for name in _DIMENSIONS
    }


def _reported(limits: QueryExecutionResourceLimits) -> dict[str, int]:
    return {name: getattr(limits, name) for name in _DIMENSIONS}


def test_session_default_answer_policy_is_the_cores() -> None:
    defaults = _core_defaults()
    assert all(type(value) is int for value in defaults.values())
    # QuerySession uses `QueryExecutionResourceLimits()` when given none.
    assert _reported(QueryExecutionResourceLimits()) == defaults


def test_omitted_dimension_keeps_the_core_default_beside_a_given_one() -> None:
    limits = QueryExecutionResourceLimits(items=5, statements=1)
    assert _reported(limits) == {**_core_defaults(), "items": 5, "statements": 1}
