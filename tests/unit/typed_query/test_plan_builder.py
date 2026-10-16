"""The Python plan builder forwards to the shared core builder."""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest
from type_bridge_core import QueryV2Authority, QueryV2Error, query_v2_authority

from type_bridge import Plan as TopLevelPlan
from type_bridge.plan import (
    Count,
    Exists,
    Matched,
    Plan,
    PlanAnswer,
    PlanBuilder,
    PlanExpr,
    Rows,
    literal,
)

# Written and checked by the Rust test
# `python_purchase_fixture_is_the_rust_hand_built_purchase`: the Rust SDK's
# hand-built purchase and the declared schema it was built under.
FIXTURE = json.loads(
    (Path(__file__).parents[2] / "fixtures" / "plan_builder" / "purchase.json").read_text()
)


def _authority() -> QueryV2Authority:
    return query_v2_authority(
        bytes.fromhex(FIXTURE["declared_hex"]), FIXTURE["scope"], FIXTURE["profile"]
    )


# The shapes a generated package's tokens carry (`Item`, `Item.stock`,
# `Purchase.buyer`).
class Item:
    __type_id__ = '{"kind":"entity","label":"item"}'


class Account:
    __type_id__ = '{"kind":"entity","label":"account"}'


class Purchase:
    __type_id__ = '{"kind":"relation","label":"purchase"}'


@dataclass(frozen=True)
class Token:
    owner: type[Any]
    fact: Mapping[str, object]


def field(owner: type[Any], attribute: str) -> Token:
    return Token(owner, {"id": {"owner": json.loads(owner.__type_id__), "attribute": attribute}})


def role(owner: type[Any], label: str) -> Token:
    relation = json.loads(owner.__type_id__)["label"]
    return Token(owner, {"role": {"declaring_relation": relation, "label": label}})


ITEM_ID, STOCK, PRICE = field(Item, "item-id"), field(Item, "stock"), field(Item, "price")
ACCOUNT_ID, BALANCE = field(Account, "account-id"), field(Account, "balance")
ORDER_ID = field(Purchase, "order-id")
BUYER, ITEM = role(Purchase, "buyer"), role(Purchase, "item")


def purchase(order: str, item_id: str, account_id: str) -> Plan:
    """The purchase as the guide writes it, in the Rust SDK's call order."""
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_input = plan.input("order_id", "string")
    item_input = plan.input("item_id", "string")
    account_input = plan.input("account_id", "string")
    item_key = plan.field(item, ITEM_ID)
    buyer_key = plan.field(buyer, ACCOUNT_ID)
    stock = plan.field(item, STOCK)
    price = plan.field(item, PRICE)
    balance = plan.field(buyer, BALANCE)
    (
        plan.where_(item_key.eq(item_input))
        .where_(buyer_key.eq(account_input))
        .where_(stock.gt(0))
        .where_(balance.ge(price))
    )
    next_stock = plan.let_("next_stock", stock - 1)
    next_balance = plan.let_("next_balance", balance - price)
    plan.update(item, STOCK, next_stock).update(buyer, BALANCE, next_balance)
    (plan.insert(Purchase).field(ORDER_ID, order_input).player(BUYER, buyer).player(ITEM, item))
    plan.row([order_input.value(order), item_input.value(item_id), account_input.value(account_id)])
    return plan.build(_authority())


def test_the_python_purchase_equals_the_rust_hand_built_purchase_byte_for_byte() -> None:
    args = FIXTURE["purchase"]
    built = purchase(args["order"], args["item_id"], args["account_id"])
    assert built.canonical_bytes().hex() == FIXTURE["purchase_hex"]
    assert built.writes()
    assert len(built.fingerprint()) == 64
    assert isinstance(built, Plan) and TopLevelPlan is Plan
    other = purchase("O2", args["item_id"], args["account_id"])
    assert other.canonical_bytes().hex() != FIXTURE["purchase_hex"]


def _variables(plan: Plan) -> list[str]:
    program = json.loads(plan.canonical_bytes())
    return [binding["variable"] for binding in program["bindings"]]


def test_derived_names_follow_labels_and_take_a_suffix_on_collision() -> None:
    plan = PlanBuilder()
    left = plan.exact(Item)
    right = plan.exact(Item)
    plan.returning([plan.field(left, STOCK), plan.field(right, STOCK)])
    assert _variables(plan.build(_authority())) == ["item", "item_2", "item_stock", "item_2_stock"]


@pytest.mark.parametrize("terminal", ["rows", "count", "exists"])
def test_reads_answer_rows_count_or_exists_and_never_write(terminal: str) -> None:
    plan = PlanBuilder()
    item = plan.exact(Item)
    stock = plan.field(item, STOCK)
    plan.where_(stock.ge(0))
    if terminal == "rows":
        plan.returning([item, stock])
    elif terminal == "count":
        plan.count()
    else:
        plan.exists()
    built = plan.build(_authority())
    assert not built.writes()


def test_expressions_and_typed_literals_build() -> None:
    plan = PlanBuilder()
    item = plan.exact(Item)
    stock = plan.field(item, STOCK)
    price = plan.field(item, PRICE)
    total = plan.let_("total", PlanExpr.max(stock * price, literal("long", 0)) % 7)
    plan.returning([total])
    assert "total" in _variables(plan.build(_authority()))


def test_content_errors_surface_at_build_with_a_stable_code() -> None:
    plan = PlanBuilder()
    item = plan.exact(Item)
    plan.input("dup", "string")
    plan.input("dup", "string")
    plan.returning([item])
    with pytest.raises(QueryV2Error) as raised:
        plan.build(_authority())
    assert getattr(raised.value, "code", None) == "query_plan_builder_duplicate_name"


def test_count_on_a_write_is_refused_at_build() -> None:
    plan = PlanBuilder()
    item = plan.exact(Item)
    plan.delete(item).count()
    with pytest.raises(QueryV2Error) as raised:
        plan.build(_authority())
    assert getattr(raised.value, "code", None) == "query_plan_builder_write_operation"


def test_a_literal_of_the_wrong_host_type_is_refused_at_build() -> None:
    plan = PlanBuilder()
    item = plan.exact(Item)
    plan.where_(plan.field(item, STOCK).gt(literal("long", "zero")))
    with pytest.raises(QueryV2Error):
        plan.build(_authority())


def test_handles_of_another_builder_and_foreign_tokens_raise_at_once() -> None:
    first, second = PlanBuilder(), PlanBuilder()
    item = first.exact(Item)
    with pytest.raises(TypeError):
        second.delete(item)
    with pytest.raises(TypeError):
        first.field(item, BALANCE)
    with pytest.raises(TypeError):
        first.exact(object)


def test_plan_answers_are_typed_values() -> None:
    assert Matched(1) == Matched(1)
    assert Matched(1) != Matched(0)
    assert Matched(1) != Count(1)
    assert isinstance(Exists(True), PlanAnswer)
    assert repr(Matched(2)) == "Matched(2)"
    assert repr(Exists(False)) == "Exists(False)"
    assert Rows([{"a": 1}]) == Rows([{"a": 1}])
    match Count(3):
        case Count(n):
            assert n == 3
        case _:
            pytest.fail("Count did not match")
