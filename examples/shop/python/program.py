"""The shop checkout of the `writes` SDK conformance suite, in Python.

A checkout takes a whole cart at once: every line must be in stock and the
account must cover the total, and then every stock, the balance and the
order lines change together. Read first and write later, and two buyers can
both see the last book; check in one query and write in the same query, and
the database decides. So a cart is one plan, and one plan is one TypeQL
query. If any check fails the match is empty, nothing is written, and the
plan answers `Matched(0)`: check the answer.

Several plans staged in one transaction commit together, and each sees the
writes staged before it. A plan whose check fails does not stop the others,
so a check that must gate the whole cart belongs in the cart's own plan.

When a live target is configured, the program runs the steps of
`tests/contracts/sdk_conformance/writes/shop/program.json` directly and over
the proxy, and prints one JSON report. The generated package is imported by
name, `shop_models` unless TYPE_BRIDGE_SDK_WRITES_PYTHON_PACKAGE names
another.
"""

from __future__ import annotations

import importlib
import json
import os
import sys
import threading
from collections.abc import Callable
from typing import Any

from type_bridge_core import ProxyConnection

from type_bridge import CommitConflict, Database
from type_bridge.plan import Count, Exists, Matched, Plan, PlanAnswer, Rows

models: Any = importlib.import_module(
    os.environ.get("TYPE_BRIDGE_SDK_WRITES_PYTHON_PACKAGE", "shop_models")
)
Account, Item, OrderLine = models.Account, models.Item, models.OrderLine


def seed_accounts(authority: Any) -> Plan:
    """Three accounts, one given row each."""
    plan = Plan.builder()
    account_id = plan.input("account_id", "string")
    balance = plan.input("balance", "long")
    plan.insert(Account).field(Account.account_id, account_id).field(Account.balance, balance)
    for key, value in (("A1", 100), ("A2", 30), ("R1", 100)):
        plan.row([account_id.value(key), balance.value(value)])
    return plan.build(authority)


def seed_items(authority: Any) -> Plan:
    """Four items with a price and a stock, one given row each."""
    plan = Plan.builder()
    sku = plan.input("sku", "string")
    price = plan.input("price", "long")
    stock = plan.input("stock", "long")
    plan.insert(Item).field(Item.sku, sku).field(Item.price, price).field(Item.stock, stock)
    for key, cost, count in (("pen", 3, 10), ("book", 40, 1), ("mug", 12, 5), ("lamp", 10, 1)):
        plan.row([sku.value(key), price.value(cost), stock.value(count)])
    return plan.build(authority)


# --8<-- [start:checkout]
def checkout(authority: Any, account_id: str, order_id: str, cart: list[tuple[str, int]]) -> Plan:
    """The whole cart in one query: check every line and the total, then write."""
    plan = Plan.builder()
    buyer = plan.exact(Account)
    balance = plan.field(buyer, Account.balance)
    plan.where_(plan.field(buyer, Account.account_id).eq(account_id))

    lines = []
    total = None
    for sku, quantity in cart:
        item = plan.exact(Item)
        stock = plan.field(item, Item.stock)
        plan.where_(plan.field(item, Item.sku).eq(sku)).where_(stock.ge(quantity))
        cost = plan.field(item, Item.price) * quantity
        total = cost if total is None else total + cost
        lines.append((item, stock, quantity))
    total = plan.let_("total", total)
    plan.where_(balance.ge(total))

    for number, (item, stock, quantity) in enumerate(lines, start=1):
        plan.update(item, Item.stock, plan.let_(f"stock_{number}", stock - quantity))
    plan.update(buyer, Account.balance, plan.let_("balance_after", balance - total))
    for item, _, quantity in lines:
        (
            plan.insert(OrderLine)
            .field(OrderLine.order_id, order_id)
            .field(OrderLine.quantity, quantity)
            .player(OrderLine.buyer, buyer)
            .player(OrderLine.item, item)
        )
    return plan.build(authority)


# --8<-- [end:checkout]


def count_where(
    authority: Any, model: Any, key: Any, key_value: str, field: Any, value: int
) -> Plan:
    """Count the things of `model` whose `key` and `field` hold the given values."""
    plan = Plan.builder()
    thing = plan.exact(model)
    plan.where_(plan.field(thing, key).eq(key_value))
    plan.where_(plan.field(thing, field).eq(value))
    return plan.count().build(authority)


def lines(authority: Any) -> Plan:
    plan = Plan.builder()
    plan.exact(OrderLine)
    return plan.count().build(authority)


def balance_is(account_id: str, value: int) -> Any:
    return lambda authority: count_where(
        authority, Account, Account.account_id, account_id, Account.balance, value
    )


def stock_is(sku: str, value: int) -> Any:
    return lambda authority: count_where(authority, Item, Item.sku, sku, Item.stock, value)


def cart(account_id: str, order_id: str, *lines: tuple[str, int]) -> Any:
    return lambda authority: checkout(authority, account_id, order_id, list(lines))


BUILDERS = {
    "seed_accounts": seed_accounts,
    "seed_items": seed_items,
    "checkout_a1": cart("A1", "O1", ("pen", 2), ("book", 1)),
    "a1_paid": balance_is("A1", 54),
    "book_sold_out": stock_is("book", 0),
    "pens_left": stock_is("pen", 8),
    "lines": lines,
    "checkout_a2": cart("A2", "O2", ("mug", 1), ("book", 1)),
    "checkout_short": cart("A2", "O3", ("mug", 3)),
    "mugs_kept": stock_is("mug", 5),
    "a2_kept": balance_is("A2", 30),
    "checkout_a2_mug": cart("A2", "O4", ("mug", 1)),
    "checkout_a2_pens": cart("A2", "O5", ("pen", 6)),
    "a2_spent": balance_is("A2", 0),
    "pens_final": stock_is("pen", 2),
    "mugs_final": stock_is("mug", 4),
    "race_lamp": cart("R1", "L1", ("lamp", 1)),
    "lamp_sold_out": stock_is("lamp", 0),
    "r1_paid_once": balance_is("R1", 90),
}
STEPS = {
    "seed": ("seed_accounts", "seed_items"),
    "checkout": ("checkout_a1",),
    "checked_out": ("a1_paid", "book_sold_out", "pens_left", "lines"),
    "sold_out": ("checkout_a2",),
    "short": ("checkout_short",),
    "unchanged": ("mugs_kept", "a2_kept", "lines"),
    "two_carts": ("checkout_a2_mug", "checkout_a2_pens"),
    "final": ("a2_spent", "pens_final", "mugs_final", "lines"),
    "race": ("race_lamp",),
    "raced": ("lamp_sold_out", "r1_paid_once", "lines"),
}
READS = {"checked_out", "unchanged", "final", "raced"}
# The race step: this many buyers commit its one plan at once.
BUYERS = 8
ATTEMPTS = 20


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


# --8<-- [start:stage]
def run(transaction: Any, plans: dict[str, Plan], names: tuple[str, ...]) -> dict[str, Any]:
    """Stage `names` in one transaction, commit once, and read their answers."""
    with transaction as tx:
        pending = {name: tx.stage(plans[name]) for name in names}
    # The block committed; each answer is known only now.
    return {name: answer(item.result()) for name, item in pending.items()}


# --8<-- [end:stage]


# --8<-- [start:race]
def race(open_transaction: Callable[[], Any], plan: Plan) -> list[Any]:
    """BUYERS buyers commit `plan` at once; answer the sum of their matched counts.

    Each buyer stages the plan in its own transaction and waits until every
    buyer has staged, so the commits overlap. A buyer that loses the race gets
    a retry-safe conflict and nothing it staged is stored; it runs the whole
    transaction again on a fresh snapshot, where the lamp is gone.
    """
    ready = threading.Barrier(BUYERS, timeout=120)
    matched = [0] * BUYERS
    errors: list[BaseException] = []
    conflicts: list[int] = []

    def buyer(index: int) -> None:
        try:
            for attempt in range(ATTEMPTS):
                try:
                    with open_transaction() as tx:
                        bought = tx.stage(plan)
                        if attempt == 0:
                            ready.wait()
                    _, matched[index] = answer(bought.result())
                    return
                except CommitConflict as error:
                    if not error.failure.retry_safe or attempt + 1 == ATTEMPTS:
                        raise
                    conflicts.append(index)
        except BaseException as error:  # reported by the main thread
            errors.append(error)
            ready.abort()

    threads = [threading.Thread(target=buyer, args=(index,)) for index in range(BUYERS)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    if errors:
        raise errors[0]
    # Diagnostic only: how many commits lost and were retried varies by run.
    print(f"race: {len(conflicts)} conflicts retried", file=sys.stderr)
    return ["matched", sum(matched)]


# --8<-- [end:race]


def steps(open_transaction: Callable[[str], Any], plans: dict[str, Plan]) -> dict[str, Any]:
    """Run every step; the race step runs its plan once per buyer at once."""
    results: dict[str, Any] = {}
    for step, names in STEPS.items():
        if step == "race":
            (name,) = names
            results[step] = {name: race(lambda: open_transaction("write"), plans[name])}
        else:
            kind = "read" if step in READS else "write"
            results[step] = run(open_transaction(kind), plans, names)
    return results


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
            report["results"]["direct"] = steps(db.staged_transaction, plans)
        finally:
            db.close()
    if endpoint:
        proxy = ProxyConnection.connect(authority, endpoint)
        report["results"]["proxy"] = steps(proxy.transaction, plans)
    json.dump(report, sys.stdout, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
