"""The purchase program of the `writes` SDK conformance suite, in Python.

It builds every plan from the generated package's tokens and authority,
then, when a live target is configured, runs the same program directly and
over the proxy: seed, stage the purchase, a touch and a batch of 50 orders in
one transaction, commit, and read the state back. It prints one JSON report.

The generated package (see `tests/contracts/sdk_conformance/writes/purchase`)
is imported by name, `purchase_models` unless TYPE_BRIDGE_SDK_WRITES_PYTHON_PACKAGE
names another.
"""

from __future__ import annotations

import importlib
import json
import os
import sys
from typing import Any

from type_bridge_core import ProxyConnection

from type_bridge import Database
from type_bridge.plan import Count, Exists, Matched, Plan, PlanAnswer, Rows

models: Any = importlib.import_module(
    os.environ.get("TYPE_BRIDGE_SDK_WRITES_PYTHON_PACKAGE", "purchase_models")
)
Item, Account, Doctor, Purchase = models.Item, models.Account, models.Doctor, models.Purchase

BATCH_ORDERS = [f"B{number:02d}" for number in range(1, 51)]


def seed(authority: Any) -> Plan:
    """One item in stock, one funded account and one on-call doctor."""
    plan = Plan.builder()
    item_id = plan.input("item_id", "string")
    stock = plan.input("stock", "long")
    price = plan.input("price", "long")
    account_id = plan.input("account_id", "string")
    balance = plan.input("balance", "long")
    doctor = plan.input("doctor", "string")
    on_call = plan.input("on_call", "boolean")
    (
        plan.insert(Item)
        .field(Item.item_id, item_id)
        .field(Item.stock, stock)
        .field(Item.price, price)
    )
    plan.insert(Account).field(Account.account_id, account_id).field(Account.balance, balance)
    plan.insert(Doctor).field(Doctor.name, doctor).field(Doctor.on_call, on_call)
    plan.row(
        [
            item_id.value("I1"),
            stock.value(1),
            price.value(40),
            account_id.value("A1"),
            balance.value(100),
            doctor.value("D1"),
            on_call.value(True),
        ]
    )
    return plan.build(authority)


def purchase(authority: Any) -> Plan:
    """The one-query purchase: take one item and its price, record the order."""
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_input = plan.input("order_id", "string")
    item_input = plan.input("item_id", "string")
    account_input = plan.input("account_id", "string")
    item_key = plan.field(item, Item.item_id)
    buyer_key = plan.field(buyer, Account.account_id)
    stock = plan.field(item, Item.stock)
    price = plan.field(item, Item.price)
    balance = plan.field(buyer, Account.balance)
    plan.where_(item_key.eq(item_input)).where_(buyer_key.eq(account_input))
    plan.where_(stock.gt(0)).where_(balance.ge(price))
    plan.update(item, Item.stock, plan.let_("next_stock", stock - 1))
    plan.update(buyer, Account.balance, plan.let_("next_balance", balance - price))
    (
        plan.insert(Purchase)
        .field(Purchase.order_id, order_input)
        .player(Purchase.buyer, buyer)
        .player(Purchase.item, item)
    )
    plan.row([order_input.value("O1"), item_input.value("I1"), account_input.value("A1")])
    return plan.build(authority)


def touch(authority: Any) -> Plan:
    """Rewrite every doctor's on-call value with itself, taking its lock."""
    plan = Plan.builder()
    doctor = plan.exact(Doctor)
    plan.update(doctor, Doctor.on_call, plan.field(doctor, Doctor.on_call))
    return plan.build(authority)


def batch(authority: Any) -> Plan:
    """Fifty orders of the item by the account, one given row each."""
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_input = plan.input("order_id", "string")
    plan.where_(plan.field(item, Item.item_id).eq("I1"))
    plan.where_(plan.field(buyer, Account.account_id).eq("A1"))
    (
        plan.insert(Purchase)
        .field(Purchase.order_id, order_input)
        .player(Purchase.buyer, buyer)
        .player(Purchase.item, item)
    )
    for order in BATCH_ORDERS:
        plan.row([order_input.value(order)])
    return plan.build(authority)


def sold_out(authority: Any) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    plan.where_(plan.field(item, Item.item_id).eq("I1"))
    plan.where_(plan.field(item, Item.stock).eq(0))
    return plan.count().build(authority)


def charged(authority: Any) -> Plan:
    plan = Plan.builder()
    account = plan.exact(Account)
    plan.where_(plan.field(account, Account.account_id).eq("A1"))
    plan.where_(plan.field(account, Account.balance).eq(60))
    return plan.count().build(authority)


def orders(authority: Any) -> Plan:
    plan = Plan.builder()
    plan.exact(Purchase)
    return plan.count().build(authority)


def on_call(authority: Any) -> Plan:
    plan = Plan.builder()
    doctor = plan.exact(Doctor)
    plan.where_(plan.field(doctor, Doctor.on_call).eq(True))
    return plan.exists().build(authority)


STEPS = {
    "seed": ("seed",),
    "program": ("purchase", "touch", "batch"),
    "read": ("sold_out", "charged", "orders", "on_call"),
}
BUILDERS = {
    "seed": seed,
    "purchase": purchase,
    "touch": touch,
    "batch": batch,
    "sold_out": sold_out,
    "charged": charged,
    "orders": orders,
    "on_call": on_call,
}


def answer(value: PlanAnswer) -> list[Any]:
    """One answer as [kind, value]; rows report their count, exists 0 or 1."""
    match value:
        case Matched(count):
            return ["matched", count]
        case Count(count):
            return ["count", count]
        case Exists(exists):
            return ["exists", int(exists)]
        case Rows(rows):
            return ["rows", len(rows)]
    raise TypeError(f"unknown answer {value!r}")


def run(transaction: Any, plans: dict[str, Plan], names: tuple[str, ...]) -> dict[str, Any]:
    """Stage `names` in one transaction, commit, and read their answers."""
    with transaction as tx:
        pending = {name: tx.stage(plans[name]) for name in names}
    return {name: answer(item.result()) for name, item in pending.items()}


def main() -> int:
    authority = models.schema_authority()
    plans = {name: build(authority) for name, build in BUILDERS.items()}
    report: dict[str, Any] = {
        "binding": "python",
        "plans": {name: plan.canonical_bytes().hex() for name, plan in plans.items()},
        "results": {},
    }
    database = os.environ.get("TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE")
    endpoint = os.environ.get("TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT")
    if database:
        db = Database(
            address=os.environ.get("TYPEDB_ADDRESS", "localhost:1729"),
            database=database,
            username=os.environ.get("TYPEDB_USERNAME", "admin"),
            password=os.environ.get("TYPEDB_PASSWORD", "password"),
            http_port=int(os.environ.get("TYPEDB_HTTP_PORT", "8000")),
        )
        db.connect()
        try:
            report["results"]["direct"] = {
                step: run(
                    db.staged_transaction("read" if step == "read" else "write"), plans, names
                )
                for step, names in STEPS.items()
            }
        finally:
            db.close()
    if endpoint:
        proxy = ProxyConnection.connect(authority, endpoint)
        report["results"]["proxy"] = {
            step: run(proxy.transaction("read" if step == "read" else "write"), plans, names)
            for step, names in STEPS.items()
        }
    json.dump(report, sys.stdout, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
