"""The major TypeBridge pipelines through the Python SDK, against a live TypeDB.

Usage::

    python -m perf.drivers.python.pipelines --out FILE --cli PATH
        [--iterations N] [--warmup N] [--pipeline NAME]... [--work-dir DIR]

The same data pipelines, seeds and plans as the Rust driver
(``type-bridge-core/crates/perf/src/bin/pipelines.rs``), written as the
``type-bridge-perf-pipelines/v1`` report. ``migration_apply`` is a CLI
operation with no Python SDK entry, so only the Rust report has it.

Each iteration is timed from the Python call that starts building its plans
to the last decoded answer. Stage times come from the private recorder
``type_bridge_core._perf_stages()``, which exists only in a binding built
with ``--features perf-stages``. TypeDB comes from ``TYPEDB_ADDRESS``,
``TYPEDB_HTTP_PORT``, ``TYPEDB_USERNAME`` and ``TYPEDB_PASSWORD``.

``--binding`` instead times the offline ops of ``perf.harness.binding``
with no TypeDB (the CLI still sets up the workspace for the declared
schema) and writes a ``type-bridge-perf-binding/v1`` report.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

import type_bridge_core
from type_bridge_core import (
    PyRustDatabase,
    QueryV2Authority,
    query_v2_authority,
    query_v2_query_only_authority,
)

from perf.drivers.workspace import PROFILE, SCOPE, Workspace, drop
from type_bridge.plan import Count, Matched, Plan, Rows

FORMAT = "type-bridge-perf-pipelines/v1"
BINDING_FORMAT = "type-bridge-perf-binding/v1"
SEEDED_ITEMS = 1_000
FILTER_STOCK = 900
BATCH_ROWS = 1_000
MIXED_BATCH_ROWS = 50
DOCTORS = 10
# The data pipelines, in run order, with logical operations per iteration.
PIPELINES: dict[str, int] = {
    "insert_one": 1,
    "fetch_by_key": 1,
    "fetch_filtered": 1,
    "update": 1,
    "delete": 1,
    "batch_given_rows_1k": BATCH_ROWS,
    "staged_mixed": 1,
}
# The offline ``--binding`` ops, in report order (perf/harness/binding.py).
BINDING_OPS = ("build_purchase", "build_rows_1k", "encode_rows_1k")


# ------------------------------------------------------------------ tokens
# The generated token shapes for the purchase schema, as the race driver
# (tests/live/races/driver.py) hand-makes them.


def _model(kind: str, label: str) -> type[Any]:
    name = label.title().replace("-", "")
    return type(name, (), {"__type_id__": json.dumps({"kind": kind, "label": label})})


Item = _model("entity", "item")
Account = _model("entity", "account")
Doctor = _model("entity", "doctor")
Purchase = _model("relation", "purchase")


@dataclass(frozen=True)
class _Token:
    owner: type[Any]
    fact: dict[str, object]


def _field(owner: type[Any], attribute: str) -> _Token:
    return _Token(owner, {"id": {"owner": json.loads(owner.__type_id__), "attribute": attribute}})


def _role(owner: type[Any], label: str) -> _Token:
    return _Token(owner, {"role": {"declaring_relation": "purchase", "label": label}})


ITEM_ID, STOCK, PRICE = (_field(Item, name) for name in ("item-id", "stock", "price"))
ACCOUNT_ID, BALANCE = _field(Account, "account-id"), _field(Account, "balance")
NAME, ON_CALL = _field(Doctor, "name"), _field(Doctor, "on-call")
ORDER_ID = _field(Purchase, "order-id")
BUYER, BOUGHT = _role(Purchase, "buyer"), _role(Purchase, "item")


# ------------------------------------------------------------------- plans


def insert_items(authority: QueryV2Authority, prefix: str, numbers: range) -> Plan:
    plan = Plan.builder()
    item_id = plan.input("item_id", "string")
    stock = plan.input("stock", "long")
    price = plan.input("price", "long")
    plan.insert(Item).field(ITEM_ID, item_id).field(STOCK, stock).field(PRICE, price)
    for number in numbers:
        plan.row([item_id.value(f"{prefix}{number}"), stock.value(number), price.value(5)])
    return plan.build(authority)


def fetch_by_key(authority: QueryV2Authority, key: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    item_id = plan.input("item_id", "string")
    stock, price = plan.field(item, STOCK), plan.field(item, PRICE)
    plan.where_(plan.field(item, ITEM_ID).eq(item_id)).returning([stock, price])
    plan.row([item_id.value(key)])
    return plan.build(authority)


def fetch_filtered(authority: QueryV2Authority) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    item_key, stock = plan.field(item, ITEM_ID), plan.field(item, STOCK)
    plan.where_(stock.ge(FILTER_STOCK)).returning([item_key, stock])
    return plan.build(authority)


def restock(authority: QueryV2Authority, key: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    item_id = plan.input("item_id", "string")
    stock = plan.field(item, STOCK)
    plan.where_(plan.field(item, ITEM_ID).eq(item_id))
    plan.update(item, STOCK, plan.let_("next_stock", stock + 1))
    plan.row([item_id.value(key)])
    return plan.build(authority)


def delete_item(authority: QueryV2Authority, key: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    item_id = plan.input("item_id", "string")
    plan.where_(plan.field(item, ITEM_ID).eq(item_id)).delete(item)
    plan.row([item_id.value(key)])
    return plan.build(authority)


def owners(authority: QueryV2Authority) -> Plan:
    plan = Plan.builder()
    plan.insert(Item).field(ITEM_ID, "I1").field(STOCK, 1_000_000).field(PRICE, 1)
    plan.insert(Account).field(ACCOUNT_ID, "A1").field(BALANCE, 1_000_000_000)
    return plan.build(authority)


def doctors(authority: QueryV2Authority) -> Plan:
    plan = Plan.builder()
    name = plan.input("name", "string")
    plan.insert(Doctor).field(NAME, name).field(ON_CALL, True)
    for number in range(DOCTORS):
        plan.row([name.value(f"D{number}")])
    return plan.build(authority)


def purchase(authority: QueryV2Authority, order: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_id = plan.input("order_id", "string")
    stock, price = plan.field(item, STOCK), plan.field(item, PRICE)
    balance = plan.field(buyer, BALANCE)
    plan.where_(plan.field(item, ITEM_ID).eq("I1"))
    plan.where_(plan.field(buyer, ACCOUNT_ID).eq("A1"))
    plan.where_(stock.gt(0)).where_(balance.ge(price))
    plan.update(item, STOCK, plan.let_("next_stock", stock - 1))
    plan.update(buyer, BALANCE, plan.let_("next_balance", balance - price))
    plan.insert(Purchase).field(ORDER_ID, order_id).player(BUYER, buyer).player(BOUGHT, item)
    plan.row([order_id.value(order)])
    return plan.build(authority)


def touch(authority: QueryV2Authority) -> Plan:
    plan = Plan.builder()
    doctor = plan.exact(Doctor)
    plan.update(doctor, ON_CALL, plan.field(doctor, ON_CALL))
    return plan.build(authority)


def orders(authority: QueryV2Authority, prefix: str, rows: int) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_id = plan.input("order_id", "string")
    plan.where_(plan.field(item, ITEM_ID).eq("I1"))
    plan.where_(plan.field(buyer, ACCOUNT_ID).eq("A1"))
    plan.insert(Purchase).field(ORDER_ID, order_id).player(BUYER, buyer).player(BOUGHT, item)
    for number in range(rows):
        plan.row([order_id.value(f"{prefix}{number}")])
    return plan.build(authority)


# --------------------------------------------------------------- iterations


def commit(
    db: PyRustDatabase, kind: Literal["read", "write"], plans: Sequence[Plan]
) -> list[object]:
    """Stage ``plans`` in one transaction, commit, and decode their answers."""
    tx = db.staged_transaction(kind)
    try:
        pending = [tx.stage(plan) for plan in plans]
        tx.commit()
    finally:
        tx.close()
    return [answer.result() for answer in pending]


def expect(answer: object, rows: int) -> None:
    if isinstance(answer, Matched):
        actual = answer.count
    elif isinstance(answer, Count):
        actual = answer.count
    elif isinstance(answer, Rows):
        actual = len(answer.rows)
    else:
        raise AssertionError(f"unexpected answer {answer!r}")
    if actual != rows:
        raise AssertionError(f"{actual} rows, expected {rows}")


def seed(name: str, db: PyRustDatabase, authority: QueryV2Authority, iterations: int) -> None:
    if name in ("fetch_by_key", "fetch_filtered", "update"):
        commit(db, "write", [insert_items(authority, "item-", range(SEEDED_ITEMS))])
    elif name == "delete":
        commit(db, "write", [insert_items(authority, "doomed-", range(iterations))])
    elif name == "staged_mixed":
        commit(db, "write", [owners(authority), doctors(authority)])


def iteration(name: str, db: PyRustDatabase, authority: QueryV2Authority, number: int) -> None:
    key = f"item-{number % SEEDED_ITEMS}"
    if name == "insert_one":
        expect(
            commit(db, "write", [insert_items(authority, "new-", range(number, number + 1))])[0], 1
        )
    elif name == "fetch_by_key":
        expect(commit(db, "read", [fetch_by_key(authority, key)])[0], 1)
    elif name == "fetch_filtered":
        expect(commit(db, "read", [fetch_filtered(authority)])[0], SEEDED_ITEMS - FILTER_STOCK)
    elif name == "update":
        expect(commit(db, "write", [restock(authority, key)])[0], 1)
    elif name == "delete":
        expect(commit(db, "write", [delete_item(authority, f"doomed-{number}")])[0], 1)
    elif name == "batch_given_rows_1k":
        plan = insert_items(authority, f"batch-{number}-", range(BATCH_ROWS))
        expect(commit(db, "write", [plan])[0], BATCH_ROWS)
    elif name == "staged_mixed":
        plans = [
            purchase(authority, f"O{number}"),
            touch(authority),
            orders(authority, f"B{number}-", MIXED_BATCH_ROWS),
        ]
        bought, touched, batch = commit(db, "write", plans)
        expect(bought, 1)
        expect(touched, DOCTORS)
        expect(batch, MIXED_BATCH_ROWS)
    else:
        raise ValueError(f"{name} is not a data pipeline")


# ------------------------------------------------------------------- driver


@dataclass
class Config:
    out: Path
    cli: Path
    work_dir: Path
    iterations: int
    warmup: int
    only: list[str]
    binding: bool
    address: str
    http_port: int
    username: str
    password: str
    samples: dict[str, list[dict[str, object]]] = field(default_factory=dict)


def _stage_reader() -> Callable[[], dict[str, int]]:
    reader = getattr(type_bridge_core, "_perf_stages", None)
    if reader is None:
        raise SystemExit(
            "error: type_bridge_core has no _perf_stages(); build it with `--features perf-stages`"
        )
    return reader


def connect(config: Config, database: str) -> PyRustDatabase:
    return PyRustDatabase.connect(
        config.address,
        database,
        username=config.username,
        password=config.password,
        http_port=config.http_port,
    )


def run_pipeline(
    name: str,
    config: Config,
    workspace: Workspace,
    declared: bytes,
    stages: Callable[[], dict[str, int]],
) -> list[dict[str, object]]:
    database = f"tb_perf_py_{os.getpid()}_{name}"
    try:
        workspace.apply(database)
        db = connect(config, database)
        try:
            authority = query_v2_query_only_authority(db, declared, SCOPE, PROFILE)
            seed(name, db, authority, config.warmup + config.iterations)
            samples: list[dict[str, object]] = []
            for number in range(config.warmup + config.iterations):
                stages()
                started = time.perf_counter_ns()
                iteration(name, db, authority, number)
                total = time.perf_counter_ns() - started
                recorded = stages()
                if number >= config.warmup:
                    samples.append({"total_ns": total, "stages": dict(sorted(recorded.items()))})
            return samples
        finally:
            db.close()
    finally:
        drop(config.address, config.http_port, config.username, config.password, database)


def parse(argv: Sequence[str] | None, env: dict[str, str]) -> Config:
    parser = argparse.ArgumentParser(prog="python -m perf.drivers.python.pipelines")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--cli", type=Path, required=True, help="type-bridge CLI binary")
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--pipeline", action="append", default=[], choices=sorted(PIPELINES))
    parser.add_argument("--binding", action="store_true", help="time the offline binding ops")
    arguments = parser.parse_args(argv)
    for name in ("TYPEDB_USERNAME", "TYPEDB_PASSWORD"):
        if not arguments.binding and not env.get(name):
            parser.error(f"set {name} for the TypeDB login")
    if arguments.iterations < 1 or arguments.warmup < 1:
        parser.error("--iterations and --warmup take positive counts")
    return Config(
        out=arguments.out,
        cli=arguments.cli,
        work_dir=arguments.work_dir or Path(tempfile.mkdtemp(prefix="type-bridge-perf-py-")),
        iterations=arguments.iterations,
        warmup=arguments.warmup,
        only=arguments.pipeline,
        binding=arguments.binding,
        address=env.get("TYPEDB_ADDRESS") or "localhost:1729",
        http_port=int(env.get("TYPEDB_HTTP_PORT") or "8000"),
        username=env.get("TYPEDB_USERNAME", ""),
        password=env.get("TYPEDB_PASSWORD", ""),
    )


def binding(config: Config, declared: bytes) -> dict[str, object]:
    """Time the offline ops: no database opens, since staging needs a connected one."""
    authority = query_v2_authority(declared, SCOPE, PROFILE)
    rows = insert_items(authority, "batch-", range(BATCH_ROWS))
    calls: dict[str, Callable[[], object]] = {
        "build_purchase": lambda: purchase(authority, "O0"),
        "build_rows_1k": lambda: insert_items(authority, "batch-", range(BATCH_ROWS)),
        "encode_rows_1k": rows.canonical_bytes,
    }
    ops = []
    for name in BINDING_OPS:
        call = calls[name]
        samples = []
        for number in range(config.warmup + config.iterations):
            started = time.perf_counter_ns()
            call()
            elapsed = time.perf_counter_ns() - started
            if number >= config.warmup:
                samples.append(elapsed)
        ops.append({"name": name, "samples_ns": samples})
    return {"format": BINDING_FORMAT, "binding": "python", "warmup": config.warmup, "ops": ops}


def main(argv: Sequence[str] | None = None) -> int:
    config = parse(argv, dict(os.environ))
    stages = _stage_reader()
    workspace = Workspace.create(config.work_dir, config.cli, config.address, config.http_port)
    declared = workspace.declared()
    if config.binding:
        config.out.parent.mkdir(parents=True, exist_ok=True)
        text = json.dumps(binding(config, declared), indent=2) + "\n"
        config.out.write_text(text, encoding="utf-8")
        return 0
    pipelines = []
    for name, operations in PIPELINES.items():
        if config.only and name not in config.only:
            continue
        print(f"pipeline {name}", file=sys.stderr)
        samples = run_pipeline(name, config, workspace, declared, stages)
        pipelines.append({"name": name, "operations": operations, "samples": samples})
    report = {
        "format": FORMAT,
        "binding": "python",
        "warmup": config.warmup,
        "pipelines": pipelines,
    }
    config.out.parent.mkdir(parents=True, exist_ok=True)
    config.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
