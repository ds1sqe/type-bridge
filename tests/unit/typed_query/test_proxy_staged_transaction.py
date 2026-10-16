"""The proxy connection opens the core staged transaction (offline checks)."""

from __future__ import annotations

import pytest
import type_bridge_core
from type_bridge_core import ProxyConnection

from tests.unit.query.test_query_v2_remote_runtime import (
    PLAN_CAPABILITIES,
    _advertisement,
    _authority,
)
from tests.unit.typed_query.test_core_limits import _closed_endpoint


def _connection() -> ProxyConnection:
    # A pinned v2 advertisement connects without reaching the endpoint.
    return ProxyConnection.connect(
        _authority(),
        _closed_endpoint(),
        advertisement=_advertisement(PLAN_CAPABILITIES),
    )


def test_staged_transaction_types_are_exported_and_not_constructible() -> None:
    for name in ("Transaction", "Pending"):
        cls = getattr(type_bridge_core, name)
        with pytest.raises(TypeError):
            cls()
    for method in ("stage", "commit", "commit_retry", "rollback", "close"):
        assert callable(getattr(type_bridge_core.Transaction, method))


def test_unknown_transaction_kind_is_refused_before_any_exchange() -> None:
    with pytest.raises(ValueError, match="transaction kind"):
        _connection().transaction("data")  # type: ignore[arg-type]


def test_transaction_fetches_the_staged_advertisement_and_fails_typed() -> None:
    # The staged advertisement is fetched at open; an unreachable server is
    # a typed provider failure, not a transaction that fails later at commit.
    with pytest.raises(Exception) as raised:
        _connection().transaction("write")
    assert not isinstance(raised.value, (ValueError, TypeError))
    assert type(raised.value).__module__.startswith("type_bridge")
