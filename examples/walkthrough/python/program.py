"""A typed shop walkthrough; the runner owns schema setup and database cleanup."""

from __future__ import annotations

import json
import logging
import os
import time
from collections.abc import Callable
from dataclasses import asdict
from typing import Any

from walkthrough_models import (
    Account,
    Document,
    Item,
    Memo,
    OrderId,
    OrderLine,
    Price,
    Quantity,
    Report,
    Sku,
    Stock,
)

from type_bridge import Database
from type_bridge.plan import Plan


class Statements(logging.Handler):
    """Collect the documented provider TRACE events, including asynchronous delivery."""

    def __init__(self) -> None:
        super().__init__(level=5)
        self.events: list[dict[str, Any]] = []

    def emit(self, record: logging.LogRecord) -> None:
        event = json.loads(record.getMessage())
        if event.get("name") == "type_bridge.io.provider.statement":
            self.events.append(event["fields"])

    def finish(self, expected: int) -> list[dict[str, Any]]:
        deadline = time.monotonic() + 10
        while len(self.events) < expected and time.monotonic() < deadline:
            time.sleep(0.01)
        if len(self.events) != expected:
            raise AssertionError(
                f"expected {expected} provider statements, captured {len(self.events)}"
            )
        events, self.events = self.events, []
        return events


capture = Statements()
logger = logging.getLogger("type_bridge.io")
logger.setLevel(5)
logger.addHandler(capture)


def emit(name: str, step: str, value: object) -> None:
    print(json.dumps({"case": name, "step": step, "value": value}, sort_keys=True), flush=True)


def models(values: list[Any]) -> list[dict[str, Any]]:
    records = []
    for item in values:
        record = {"iid": str(item.iid), "deleted": item.deleted}
        if isinstance(item, Account):
            record.update(
                type="account",
                account_id=item.account_id.value,
                balance=item.balance.value if item.balance is not None else None,
            )
        elif isinstance(item, Item):
            record.update(
                type="item",
                sku=item.sku.value,
                price=item.price.value if item.price is not None else None,
                stock=item.stock.value if item.stock is not None else None,
            )
        elif isinstance(item, OrderLine):
            record.update(
                type="order-line",
                order_id=item.order_id.value if item.order_id is not None else None,
                quantity=item.quantity.value if item.quantity is not None else None,
                buyer={"type": "account", "iid": str(item.buyer.iid)} if item.buyer else None,
                item={"type": "item", "iid": str(item.item.iid)} if item.item else None,
            )
        else:
            assert isinstance(item, (Report, Memo))
            record.update(
                type="report" if isinstance(item, Report) else "memo",
                doc_id=item.doc_id.value,
                status=item.status.value,
            )
        records.append(record)
    return records


def trace(name: str, events: list[dict[str, Any]]) -> None:
    emit(
        name,
        "plan",
        {"source": "TRACE", "fingerprints": [event.get("fingerprint", "") for event in events]},
    )
    emit(
        name, "explain", {"source": "TRACE", "statements": [event["statement"] for event in events]}
    )


def show(
    db: Database, name: str, call: str, plan: Plan[Any], *, statements: int = 1, split: bool = False
) -> list[Any]:
    emit(name, "call", call)
    explanation = None
    if not split:
        explanation = asdict(plan.explain())
        shape = json.loads(plan.canonical_bytes())
        del shape["given_rows"]
        emit(name, "plan", {"source": "canonical_bytes", "plan": shape})
        emit(name, "explain", {"source": "explain", **explanation})
    with db.staged_transaction() as tx:
        pending = tx.stage(plan)
    events = capture.finish(statements)
    if split:
        trace(name, events)
    else:
        assert explanation is not None
        assert explanation["statements"] == [event["statement"] for event in events]
    emit(name, "raw", pending.documents())
    result = pending.result()
    emit(name, "models", models(result))
    return result


def manager(name: str, call: str, execute: Callable[[], Any], *, statements: int = 1) -> list[Any]:
    emit(name, "call", call)
    result = execute()
    trace(name, capture.finish(statements))
    values = result if isinstance(result, list) else list(result.items)
    emit(name, "raw", "raw documents: not exposed by manager result")
    emit(name, "models", models(values))
    return values


def purchase() -> Plan[list[OrderLine]]:
    with Plan():
        item = Item.match("I1").where(Item.stock > 0)
        buyer = Account.match("A1")
        balance = buyer.balance
        buyer = buyer.where(balance >= item.price)
        item.set(stock=item.stock - 1)
        buyer.set(balance=buyer.balance - item.price)
        order = OrderLine.insert(order_id="O1", quantity=1, buyer=buyer, item=item)
    return order


PURCHASE_CALL = """with Plan():
    item = Item.match("I1").where(Item.stock > 0)
    buyer = Account.match("A1")
    balance = buyer.balance
    buyer = buyer.where(balance >= item.price)
    item.set(stock=item.stock - 1)
    buyer.set(balance=buyer.balance - item.price)
    order = OrderLine.insert(order_id="O1", quantity=1, buyer=buyer, item=item)"""


def main() -> None:
    db = Database(
        address=os.environ["TYPEDB_ADDRESS"],
        database=os.environ["TYPE_BRIDGE_WALKTHROUGH_DATABASE"],
        username=os.environ.get("TYPEDB_USERNAME", "admin"),
        password=os.environ.get("TYPEDB_PASSWORD", "password"),
        http_port=int(os.environ["TYPEDB_HTTP_PORT"]),
    )
    try:
        with Plan():
            account = Account.insert(account_id="A1", balance=10)
        with Plan():
            item = Item.insert(sku="I1", price=3, stock=5)
        with Plan():
            report = Report.insert(doc_id="R1", status="archived")
        with Plan():
            memo = Memo.insert(doc_id="M1", status="archived")
        with db.staged_transaction() as tx:
            for plan in (account, item, report, memo):
                tx.stage(plan)
        capture.finish(4)

        bought = show(db, "purchase", PURCHASE_CALL, purchase())
        assert len(bought) == 1 and bought[0].order_id.value == "O1"
        assert bought[0].buyer.balance.value == 7 and bought[0].item.stock.value == 4

        with Plan():
            gone = Document.subtypes().where(Document.status == "archived").delete_returning()
        removed = show(
            db,
            "delete_subtypes",
            'with Plan():\n    gone = Document.subtypes().where(Document.status == "archived").delete_returning()',
            gone,
            statements=2,
        )
        assert {type(value) for value in removed} == {Report, Memo}
        assert all(value.deleted for value in removed)

        with Plan():
            batch = Item.insert_many(
                Item.Values(sku=f"B{n}", price=3, stock=10) for n in range(10_000)
            )
        items = show(
            db,
            "bulk_insert",
            'with Plan():\n    batch = Item.insert_many(Item.Values(sku=f"B{n}", price=3, stock=10) for n in range(10_000))',
            batch,
            statements=3,
            split=True,
        )
        assert len(items) == 10_000
        assert [item.sku.value for item in items] == [f"B{n}" for n in range(10_000)]

        item_manager = Item.manager(db)
        made = manager(
            "insert",
            'Item.manager(db).insert(Item(sku=Sku("M1"), price=Price(3), stock=Stock(5)))',
            lambda: item_manager.insert(Item(sku=Sku("M1"), price=Price(3), stock=Stock(5))),
        )
        iid = made[0].iid
        assert iid is not None
        manager(
            "put",
            'Item.manager(db).put(Item(sku=Sku("M1"), price=Price(3), stock=Stock(6)))',
            lambda: item_manager.put(Item(sku=Sku("M1"), price=Price(3), stock=Stock(6))),
        )
        found = manager(
            "typed_iid",
            "Item.manager(db).filter(iid=iid).all()",
            lambda: item_manager.filter(iid=iid).all(),
        )
        assert len(found) == 1 and found[0].iid == iid
        manager(
            "filtered_fetch",
            'Item.manager(db).filter(sku=Sku("M1")).all()',
            lambda: item_manager.filter(sku=Sku("M1")).all(),
        )
        updated = manager(
            "update",
            'Item.manager(db).filter(iid=iid).update(Item(sku=Sku("M1"), price=Price(3), stock=Stock(8)))',
            lambda: item_manager.filter(iid=iid).update(
                Item(sku=Sku("M1"), price=Price(3), stock=Stock(8))
            ),
        )
        assert updated[0].stock.value == 8
        manager(
            "insert_many",
            'OrderLine.manager(db).insert_many([OrderLine(order_id=OrderId(key), quantity=Quantity(1), buyer=bought[0].buyer, item=bought[0].item) for key in ("L1", "L2")])',
            lambda: OrderLine.manager(db).insert_many(
                [
                    OrderLine(
                        order_id=OrderId(key),
                        quantity=Quantity(1),
                        buyer=bought[0].buyer,
                        item=bought[0].item,
                    )
                    for key in ("L1", "L2")
                ]
            ),
        )
        deleted = manager(
            "delete_returning",
            "Item.manager(db).delete_returning(updated[0])",
            lambda: item_manager.delete_returning(updated[0]),
            statements=2,
        )
        assert deleted[0].deleted

        with Plan():
            new_account = Account.insert(account_id="A2", balance=20)
        with Plan():
            new_item = Item.insert(sku="I2", price=4, stock=3)
        with Plan():
            buyer = Account.match("A2")
            item = Item.match("I2")
            new_order = OrderLine.insert(order_id="O2", quantity=1, buyer=buyer, item=item)
        plans = (new_account, new_item, new_order)
        emit(
            "staged_transaction",
            "call",
            """with Plan():
    new_account = Account.insert(account_id="A2", balance=20)
with Plan():
    new_item = Item.insert(sku="I2", price=4, stock=3)
with Plan():
    buyer = Account.match("A2")
    item = Item.match("I2")
    new_order = OrderLine.insert(order_id="O2", quantity=1, buyer=buyer, item=item)
with db.staged_transaction() as tx:
    pending = [tx.stage(plan) for plan in (new_account, new_item, new_order)]""",
        )
        shapes = [json.loads(plan.canonical_bytes()) for plan in plans]
        for shape in shapes:
            del shape["given_rows"]
        explanations = [asdict(plan.explain()) for plan in plans]
        emit("staged_transaction", "plan", {"source": "canonical_bytes", "plans": shapes})
        emit("staged_transaction", "explain", {"source": "explain", "plans": explanations})
        with db.staged_transaction() as tx:
            pending = [tx.stage(plan) for plan in plans]
        events = capture.finish(3)
        assert [
            statement for explanation in explanations for statement in explanation["statements"]
        ] == [event["statement"] for event in events]
        emit(
            "staged_transaction",
            "raw",
            [document for value in pending for document in value.documents()],
        )
        values = [model for value in pending for model in value.result()]
        emit("staged_transaction", "models", models(values))
        assert values[-1].buyer.account_id.value == "A2" and values[-1].item.sku.value == "I2"

        emit(
            "raw_unchecked",
            "call",
            'db.execute_query("match $x isa order-line; reduce $n = count;")',
        )
        emit("raw_unchecked", "unchecked", "no plan, no hydration")
        emit(
            "raw_unchecked", "raw", db.execute_query("match $x isa order-line; reduce $n = count;")
        )
    finally:
        db.close()


if __name__ == "__main__":
    main()
