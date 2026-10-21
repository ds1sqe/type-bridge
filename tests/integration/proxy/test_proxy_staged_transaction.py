"""Staged transactions over the proxy against a live TypeDB 3.13.6.

The server crate's `sdk_staged_fixture_serves_until_stopped` test seeds the
databases, serves three proxies (purchase, retry, refused) and writes
`fixture.json` into TB_STAGED_FIXTURE_DIR. Each fixture is one-shot: the
purchase consumes its stock.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

import pytest
from type_bridge_core import (
    Count,
    Exists,
    Matched,
    Pending,
    ProxyConnection,
    Transaction,
    query_v2_authority,
)

FIXTURE_DIR = os.environ.get("TB_STAGED_FIXTURE_DIR")

pytestmark = [
    pytest.mark.integration,
    pytest.mark.proxy,
    pytest.mark.skipif(not FIXTURE_DIR, reason="requires TB_STAGED_FIXTURE_DIR"),
]

PROGRAM = ("purchase", "count", "exists")


@pytest.fixture(scope="module")
def live() -> dict[str, Any]:
    assert FIXTURE_DIR is not None
    return json.loads((Path(FIXTURE_DIR) / "fixture.json").read_text())


def _transaction(
    live: dict[str, Any], role: str, headers: list[tuple[str, str]] | None = None
) -> Transaction:
    proxy = live["proxies"][role]
    authority = query_v2_authority(
        bytes.fromhex(live["declared_hex"]), proxy["scope"], live["profile"]
    )
    connection = ProxyConnection.connect(authority, proxy["endpoint"], headers=headers or [])
    return connection.transaction()


def _stage_program(live: dict[str, Any], tx: Transaction) -> list[Pending]:
    return [tx.stage(bytes.fromhex(live["plans_hex"][name])) for name in PROGRAM]


def test_staged_purchase_over_the_proxy_answers_like_direct(live: dict[str, Any]) -> None:
    tx = _transaction(live, "purchase")
    pending = _stage_program(live, tx)
    with pytest.raises(RuntimeError):
        pending[0].result()
    tx.commit()
    answers = [answer.result() for answer in pending]
    # The values direct execution gives for the same program (the Rust and TS
    # live suites compare direct and proxy answers on the same fixture).
    assert answers == [Matched(1), Count(1), Exists(True)]


def test_a_lost_reply_is_unknown_and_the_database_tells(live: dict[str, Any]) -> None:
    tx = _transaction(live, "lost_reply", [("x-tb-drop-reply", "1")])
    tx.stage(bytes.fromhex(live["plans_hex"]["purchase"]))
    with pytest.raises(Exception) as unknown:
        tx.commit()
    assert getattr(unknown.value, "code", None) == "commit_outcome_unknown"
    # The purchase is conditional on stock, so running it again is safe: it
    # matches nothing because the lost commit applied.
    again = _transaction(live, "lost_reply")
    (purchase,) = [again.stage(bytes.fromhex(live["plans_hex"]["purchase"]))]
    again.commit()
    assert purchase.result() == Matched(0)


def test_an_unauthenticated_write_is_refused_with_a_typed_error(live: dict[str, Any]) -> None:
    tx = _transaction(live, "refused")
    (purchase,) = [tx.stage(bytes.fromhex(live["plans_hex"]["purchase"]))]
    with pytest.raises(Exception) as refused:
        tx.commit()
    assert getattr(refused.value, "code", None) == "staged_write_not_authorized"
    with pytest.raises(Exception):
        purchase.result()
