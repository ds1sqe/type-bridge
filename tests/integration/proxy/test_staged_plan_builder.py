"""Python-built plans staged directly and over the proxy on a live TypeDB.

Needs its own fresh instance of the server crate's
`sdk_staged_fixture_serves_until_stopped` fixture in
TB_STAGED_BUILDER_FIXTURE_DIR, separate from the one
test_proxy_staged_transaction.py consumes: every purchase here needs
unbought stock.
"""

from __future__ import annotations

import json
import os
import threading
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from type_bridge_core import ProxyConnection, PyRustDatabase, QueryV2Authority, query_v2_authority

from type_bridge.plan import Matched, Plan, PlanAnswer

FIXTURE_DIR = os.environ.get("TB_STAGED_BUILDER_FIXTURE_DIR")

pytestmark = [
    pytest.mark.integration,
    pytest.mark.proxy,
    pytest.mark.skipif(not FIXTURE_DIR, reason="requires TB_STAGED_BUILDER_FIXTURE_DIR"),
]


@pytest.fixture(scope="module")
def live() -> dict[str, Any]:
    assert FIXTURE_DIR is not None
    return json.loads((Path(FIXTURE_DIR) / "fixture.json").read_text())


def _connect(database: str) -> PyRustDatabase:
    return PyRustDatabase.connect(
        os.environ.get("TYPEDB_ADDRESS", "localhost:1730"),
        database,
        http_port=int(os.environ.get("TYPEDB_HTTP_PORT", "8000")),
    )


def _authority(live: dict[str, Any]) -> QueryV2Authority:
    return query_v2_authority(
        bytes.fromhex(live["declared_hex"]), live["proxies"]["purchase"]["scope"], live["profile"]
    )


# The generated token shapes for the race schema.
def _model(kind: str, label: str) -> type[Any]:
    return type(label.title(), (), {"__type_id__": json.dumps({"kind": kind, "label": label})})


Item = _model("entity", "item")
Account = _model("entity", "account")
Purchase = _model("relation", "purchase")


@dataclass(frozen=True)
class _Token:
    owner: type[Any]
    fact: Mapping[str, object]


def _field(owner: type[Any], attribute: str) -> _Token:
    return _Token(owner, {"id": {"owner": json.loads(owner.__type_id__), "attribute": attribute}})


def _role(owner: type[Any], label: str) -> _Token:
    return _Token(owner, {"role": {"declaring_relation": "purchase", "label": label}})


def _purchase(authority: QueryV2Authority, order: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_input = plan.input("order_id", "string")
    item_input = plan.input("item_id", "string")
    account_input = plan.input("account_id", "string")
    item_key = plan.field(item, _field(Item, "item-id"))
    buyer_key = plan.field(buyer, _field(Account, "account-id"))
    stock = plan.field(item, _field(Item, "stock"))
    price = plan.field(item, _field(Item, "price"))
    balance = plan.field(buyer, _field(Account, "balance"))
    plan.where_(item_key.eq(item_input)).where_(buyer_key.eq(account_input))
    plan.where_(stock.gt(0)).where_(balance.ge(price))
    plan.update(item, _field(Item, "stock"), plan.let_("next_stock", stock - 1))
    plan.update(buyer, _field(Account, "balance"), plan.let_("next_balance", balance - price))
    (
        plan.insert(Purchase)
        .field(_field(Purchase, "order-id"), order_input)
        .player(_role(Purchase, "buyer"), buyer)
        .player(_role(Purchase, "item"), item)
    )
    plan.row([order_input.value(order), item_input.value("I1"), account_input.value("A1")])
    return plan.build(authority)


def test_the_python_purchase_commits_directly_and_over_the_proxy(live: dict[str, Any]) -> None:
    authority = _authority(live)
    plans = [_purchase(authority, "O1"), _purchase(authority, "O2")]
    db = _connect(live["direct_database"])
    try:
        direct = db.staged_transaction()
        direct_answers = [direct.stage(plan) for plan in plans]
        assert not direct.has_unknown_outcome()
        direct.commit()
        connection = ProxyConnection.connect(authority, live["proxies"]["purchase"]["endpoint"])
        remote = connection.transaction()
        remote_answers = [remote.stage(plan) for plan in plans]
        with pytest.raises(RuntimeError, match="not committed"):
            remote_answers[0].result()
        remote.commit()
        expected = [Matched(1), Matched(0)]
        assert [answer.result() for answer in direct_answers] == expected
        assert [answer.result() for answer in remote_answers] == expected
        direct.close()
        remote.close()
        with pytest.raises(RuntimeError):
            direct.stage(plans[0])
    finally:
        db.close()


def test_with_discards_on_an_exception_and_commits_on_a_clean_exit(live: dict[str, Any]) -> None:
    authority = _authority(live)
    db = _connect(live["proxies"]["retry"]["database"])
    try:
        tx = db.staged_transaction()
        discarded = tx.stage(_purchase(authority, "O-discarded"))
        with pytest.raises(LookupError), tx:
            raise LookupError("abandon")
        # The exit closed it without committing.
        with pytest.raises(RuntimeError, match="closed"):
            discarded.result()
        with db.staged_transaction(commit_on_exit=False) as tx:
            kept = tx.stage(_purchase(authority, "O-uncommitted"))
        with pytest.raises(RuntimeError, match="closed"):
            kept.result()
        # Nothing was bought: the next purchase still finds the stock.
        with db.staged_transaction("write") as tx:
            bought = tx.stage(_purchase(authority, "O-after"))
        assert bought.result() == Matched(1)
    finally:
        db.close()


def test_a_read_transaction_refuses_a_write_plan(live: dict[str, Any]) -> None:
    db = _connect(live["direct_database"])
    try:
        tx = db.staged_transaction("read")
        with pytest.raises(Exception, match="Read transaction cannot stage a write"):
            tx.stage(_purchase(_authority(live), "O-read"))
        tx.close()
    finally:
        db.close()


def test_two_python_transactions_race_for_the_last_item_and_buy_once(
    live: dict[str, Any],
) -> None:
    authority = _authority(live)
    # The refused proxy's database is seeded, and no test here writes to it.
    db = _connect(live["proxies"]["refused"]["database"])
    outcomes: dict[str, PlanAnswer | BaseException] = {}
    barrier = threading.Barrier(2)

    def racer(order: str) -> None:
        tx = db.staged_transaction()
        answer = tx.stage(_purchase(authority, order))
        barrier.wait()
        try:
            tx.commit()
            outcomes[order] = answer.result()
        except Exception as error:  # noqa: BLE001 - the loser's typed failure is the outcome
            outcomes[order] = error
        finally:
            tx.close()

    try:
        threads = [threading.Thread(target=racer, args=(order,)) for order in ("R1", "R2")]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()
    finally:
        db.close()
    bought = [outcome for outcome in outcomes.values() if outcome == Matched(1)]
    assert len(bought) == 1, f"exactly one racer buys: {outcomes!r}"
    for outcome in outcomes.values():
        if isinstance(outcome, BaseException):
            assert getattr(outcome, "code", None), f"the loser carries a typed code: {outcome!r}"
        else:
            assert isinstance(outcome, Matched)
