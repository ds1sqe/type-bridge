"""The answer-policy defaults Python reports are the core's, per transport."""

from __future__ import annotations

import socket

import type_bridge_core
from type_bridge_core import ProxyConnection, QueryExecutionResourceLimits

from tests.unit.query.test_query_v2_remote_runtime import (
    PLAN_CAPABILITIES,
    _advertisement,
    _authority,
)

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


def _core_defaults(transport: str) -> dict[str, int]:
    # Read from the extension's exports of the orm constants, never literals,
    # so a changed core default changes what Python reports.
    return {
        name: getattr(type_bridge_core, f"DEFAULT_{transport}_QUERY_{name.upper()}")
        for name in _DIMENSIONS
    }


def _reported(limits: QueryExecutionResourceLimits) -> dict[str, int | None]:
    return {name: getattr(limits, name) for name in _DIMENSIONS}


def _closed_endpoint() -> str:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    return f"http://127.0.0.1:{port}"


def test_both_transport_default_sets_are_exported() -> None:
    direct = _core_defaults("DIRECT")
    proxy = _core_defaults("PROXY")
    assert all(type(value) is int for value in (*direct.values(), *proxy.values()))
    # A direct session keeps the 2.2 in-process bounds; a proxy connection is
    # bounded by what one remote envelope carries.
    assert direct == {
        "timeout_milliseconds": 60_000,
        "items": 100_000,
        "bytes": 64 * 1024 * 1024,
        "graph_nodes": 1_000_000,
        "attribute_values": 4_000_000,
        "collection_members": 1_000_000,
        "role_players": 1_000_000,
        "statements": 3,
    }
    assert proxy == {
        "timeout_milliseconds": 60_000,
        "items": 65_536,
        "bytes": 32 * 1024 * 1024,
        "graph_nodes": 65_536,
        "attribute_values": 65_536,
        "collection_members": 65_536,
        "role_players": 65_536,
        "statements": 3,
    }
    assert not hasattr(type_bridge_core, "DEFAULT_QUERY_ITEMS")


def test_session_default_answer_policy_is_the_direct_default() -> None:
    # QuerySession uses `QueryExecutionResourceLimits()` when given none.
    assert _reported(QueryExecutionResourceLimits()) == {
        **_core_defaults("DIRECT"),
        "statements": None,
    }


def test_omitted_dimension_keeps_the_direct_default_beside_a_given_one() -> None:
    limits = QueryExecutionResourceLimits(items=5, statements=1)
    assert _reported(limits) == {**_core_defaults("DIRECT"), "items": 5, "statements": 1}


def test_proxy_connection_without_answer_limits_runs_at_the_proxy_defaults() -> None:
    # A pinned advertisement connects without reaching the endpoint.
    connection = ProxyConnection.connect(
        _authority(),
        _closed_endpoint(),
        advertisement=_advertisement(PLAN_CAPABILITIES),
    )
    assert _reported(connection.answer_limits) == {**_core_defaults("PROXY"), "statements": None}


def test_proxy_connection_keeps_explicit_answer_limits() -> None:
    connection = ProxyConnection.connect(
        _authority(),
        _closed_endpoint(),
        advertisement=_advertisement(PLAN_CAPABILITIES),
        answer_limits=QueryExecutionResourceLimits(items=8),
    )
    assert _reported(connection.answer_limits) == {
        **_core_defaults("DIRECT"),
        "items": 8,
        "statements": None,
    }


def test_omitted_statement_limit_remains_unset_and_explicit_three_is_retained() -> None:
    assert QueryExecutionResourceLimits().statements is None
    assert QueryExecutionResourceLimits(statements=3).statements == 3


def test_raised_byte_and_item_limits_refuse_values_outside_the_shared_integer_domain() -> None:
    import pytest

    maximum = (1 << 64) - 1
    limits = QueryExecutionResourceLimits(bytes=maximum, items=maximum)
    assert limits.bytes == maximum
    assert limits.items == maximum
    with pytest.raises(ValueError, match="bytes"):
        QueryExecutionResourceLimits(bytes=maximum + 1)
    with pytest.raises(ValueError, match="items"):
        QueryExecutionResourceLimits(items=maximum + 1)


def test_phase_collection_is_opt_in_and_survives_connection_policy() -> None:
    assert QueryExecutionResourceLimits().collect_phase_timestamps is False
    limits = QueryExecutionResourceLimits(collect_phase_timestamps=True)
    assert limits.collect_phase_timestamps is True
    connection = ProxyConnection.connect(
        _authority(),
        _closed_endpoint(),
        advertisement=_advertisement(PLAN_CAPABILITIES),
        answer_limits=limits,
    )
    assert connection.answer_limits.collect_phase_timestamps is True


def test_proxy_query_limits_intersect_independent_advertised_dimensions() -> None:
    import json

    from type_bridge_core import QueryLimits

    from tests.unit.query.test_query_v2_remote_runtime import _canonical

    raw = json.loads(_advertisement(PLAN_CAPABILITIES))
    raw["capabilities"] = sorted(
        {*raw["capabilities"], "query.plan.v3", "query.remote.envelope-v3"}
    )
    raw["query_limits"] = {"bindings": 768, "boolean_terms": 300, "selected_slots": 256}
    connection = ProxyConnection.connect(
        _authority(),
        _closed_endpoint(),
        advertisement=_canonical(raw),
        query_limits=QueryLimits(bindings=1024, boolean_terms=512, input_rows=13),
    )
    effective = connection.query_limits
    assert effective.get("bindings") == 768
    assert effective.get("boolean_terms") == 300
    assert effective.get("input_rows") == 13
    assert effective.get("selected_slots") == 16


def test_invalid_v3_query_limits_are_refused_before_proxy_io() -> None:
    import pytest
    from type_bridge_core import QueryLimits

    with pytest.raises(ValueError, match="selected_slots"):
        ProxyConnection.connect(
            _authority(), _closed_endpoint(), query_limits=QueryLimits(bindings=8)
        )


def test_invalid_v3_query_limits_are_refused_before_direct_io() -> None:
    import pytest
    from type_bridge_core import PyDatabase, QueryLimits

    with pytest.raises(ValueError, match="selected_slots"):
        PyDatabase.connect("127.0.0.1:1", "app", query_limits=QueryLimits(bindings=8))
