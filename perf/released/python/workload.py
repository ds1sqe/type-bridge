"""Six CRUD ops through one released ``type-bridge``, against a live TypeDB.

Usage (with the interpreter of the venv that holds the version)::

    python perf/released/python/workload.py --version VERSION --database NAME
        --out FILE [--models DIR] [--schema FILE] [--iterations N] [--warmup N]

The workload is the model manager API every 2.x release documents for an
application, ``Item.manager(db)`` on a ``type_bridge.Database``, written in
2.0.2's spelling:

- ``insert_one``: ``insert`` one item;
- ``fetch_by_key``: ``filter(item_id=...).first()`` of one of 1,000 items;
- ``fetch_filtered``: ``filter(stock__gte=900).all()``, the 100 of 1,000;
- ``update``: ``update`` one fetched item with its stock plus one;
- ``delete``: ``delete`` one fetched item;
- ``batch_insert_1k``: ``insert_many`` of 1,000 items.

Each op is timed around the manager call alone; fetching the instance an
``update`` or ``delete`` takes is not timed. The database is seeded with
1,000 items first and deleted, with its migration journal, at the end.

Only the ``Item`` model differs between versions, so ``SHIMS`` is the one
place that names a version (see ``Shim``). The module imports nothing but
the standard library at import time, so the harness and its tests read
``SHIMS`` and ``OPS`` without a ``type-bridge`` install; it never imports
``perf``. TypeDB comes from ``TYPEDB_ADDRESS``, ``TYPEDB_HTTP_PORT``,
``TYPEDB_USERNAME`` and ``TYPEDB_PASSWORD``.
"""

import argparse
import importlib
import json
import os
import sys
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

FORMAT = "type-bridge-perf-release/v1"
SEEDED_ITEMS = 1_000
FILTER_STOCK = 900
BATCH_ROWS = 1_000
# The ops, in run order, with logical operations per call.
OPS: dict[str, int] = {
    "insert_one": 1,
    "fetch_by_key": 1,
    "fetch_filtered": 1,
    "update": 1,
    "delete": 1,
    "batch_insert_1k": BATCH_ROWS,
}
# The generated package's import name under ``--models``.
MODELS_PACKAGE = "release_models"


@dataclass(frozen=True)
class Shim:
    """How one version's ``Item`` model and its schema reach the workload.

    ``models``:

    - ``handwritten``: the workload declares ``Item`` as an ``Entity``
      subclass, as 2.0.2's CRUD guide does. 2.1.0 removed the hand-written
      model classes from the public package (``from type_bridge import
      Entity`` raises ``ImportError``), so later versions cannot take it.
    - ``generated``: the harness generates ``release_models`` from
      ``perf/released/workspace`` with that version's own CLI; the workload
      imports it. 2.0.2's generated manager has only ``insert``, ``all`` and
      ``get_by_iid``, so 2.0.2 cannot take this one.

    ``schema``:

    - ``schema-manager``: ``SchemaManager(db).register(Item)`` and
      ``sync_schema()``, 2.0.2's documented path.
    - ``migration-apply``: the harness runs the version's ``migration make``
      and ``migration apply``.
    - ``migration-typeql``: the harness runs ``migration make``; the workload
      runs the TypeQL it wrote in one schema transaction. 2.1.0's ``migration
      apply`` requires a TypeDB server of exactly 3.12.1, and every version
      here runs on one 3.12.3 server.

    Both model sources expose the same ``Item``, ``ItemId``, ``Stock`` and
    ``Price`` names and the same manager calls, so the ops are one code path.
    """

    models: Literal["handwritten", "generated"]
    schema: Literal["schema-manager", "migration-apply", "migration-typeql"]


SHIMS: dict[str, Shim] = {
    "2.0.2": Shim(models="handwritten", schema="schema-manager"),
    "2.1.0": Shim(models="generated", schema="migration-typeql"),
    "2.2.5": Shim(models="generated", schema="migration-apply"),
    "head": Shim(models="generated", schema="migration-apply"),
}


def shim(version: str) -> Shim:
    """The shim of a compared version (``head`` for the source tree)."""
    try:
        return SHIMS[version]
    except KeyError:
        raise ValueError(f"no shim for version {version!r}; known: {', '.join(SHIMS)}") from None


@dataclass(frozen=True)
class Models:
    item: Any
    item_id: Any
    stock: Any
    price: Any


def handwritten_models() -> Models:
    # Evaluated annotations (no ``from __future__ import annotations`` in this
    # module): the model reads its field types from these local classes.
    from type_bridge import AttributeFlags, Entity, Flag, Integer, Key, String, TypeFlags

    class ItemId(String):
        flags = AttributeFlags(name="item-id")

    class Stock(Integer):
        flags = AttributeFlags(name="stock")

    class Price(Integer):
        flags = AttributeFlags(name="price")

    class Item(Entity):
        flags = TypeFlags(name="item")
        item_id: ItemId = Flag(Key)
        stock: Stock | None = None
        price: Price | None = None

    return Models(Item, ItemId, Stock, Price)


def generated_models(directory: Path) -> Models:
    sys.path.insert(0, str(directory))
    package = importlib.import_module(MODELS_PACKAGE)
    return Models(package.Item, package.ItemId, package.Stock, package.Price)


def run(db: Any, models: Models, *, iterations: int, warmup: int) -> dict[str, list[int]]:
    """Seed the database, then time every op; returns op -> samples in ns."""
    manager = models.item.manager(db)

    def item(key: str, stock: int) -> Any:
        return models.item(
            item_id=models.item_id(key), stock=models.stock(stock), price=models.price(5)
        )

    def fetch(key: str) -> Any:
        found = manager.filter(item_id=models.item_id(key)).first()
        if found is None:
            raise RuntimeError(f"item {key!r} is missing")
        return found

    manager.insert_many([item(f"seed-{number}", number) for number in range(SEEDED_ITEMS)])

    def insert_one(index: int) -> Callable[[], object]:
        # Stock 0 keeps the filtered op at the 100 seeded items.
        return lambda: manager.insert(item(f"one-{index}", 0))

    def fetch_by_key(index: int) -> Callable[[], object]:
        key = models.item_id(f"seed-{index % SEEDED_ITEMS}")
        return lambda: _expect(manager.filter(item_id=key).first() is not None, "fetch_by_key")

    def fetch_filtered(index: int) -> Callable[[], object]:
        stock = FILTER_STOCK
        expected = SEEDED_ITEMS - FILTER_STOCK
        return lambda: _expect(
            len(manager.filter(stock__gte=stock).all()) == expected, "fetch_filtered"
        )

    def update(index: int) -> Callable[[], object]:
        # Items below the filter bound, so the filtered count stays fixed.
        found = fetch(f"seed-{index % FILTER_STOCK}")
        found.stock = models.stock(found.stock.value + 1)
        return lambda: manager.update(found)

    def delete(index: int) -> Callable[[], object]:
        found = fetch(f"one-{index}")
        return lambda: manager.delete(found)

    def batch_insert_1k(index: int) -> Callable[[], object]:
        rows = [item(f"batch-{index}-{number}", 0) for number in range(BATCH_ROWS)]
        return lambda: manager.insert_many(rows)

    prepare: dict[str, Callable[[int], Callable[[], object]]] = {
        "insert_one": insert_one,
        "fetch_by_key": fetch_by_key,
        "fetch_filtered": fetch_filtered,
        "update": update,
        "delete": delete,
        "batch_insert_1k": batch_insert_1k,
    }
    samples: dict[str, list[int]] = {}
    for name in OPS:
        samples[name] = []
        for index in range(warmup + iterations):
            call = prepare[name](index)
            started = time.perf_counter_ns()
            call()
            elapsed = time.perf_counter_ns() - started
            if index >= warmup:
                samples[name].append(elapsed)
    return samples


def split_queries(typeql: str) -> list[str]:
    """The ``define`` queries of a ``migration make`` TypeQL file, in order.

    The file holds one query per blank-line-separated block, each starting
    with ``define``.
    """
    queries: list[str] = []
    for block in typeql.split("\n\n"):
        if block.strip():
            if not block.lstrip().startswith("define"):
                raise ValueError(f"expected a define query, got {block.strip()[:40]!r}")
            queries.append(block.strip())
    return queries


def _expect(condition: bool, op: str) -> None:
    if not condition:
        raise RuntimeError(f"{op}: unexpected answer")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="workload.py", description="Six CRUD ops through one type-bridge version."
    )
    parser.add_argument("--version", required=True, help="compared version, or head")
    parser.add_argument("--database", required=True, help="database to seed and measure")
    parser.add_argument("--out", type=Path, required=True, help="report file to write")
    parser.add_argument("--models", type=Path, help="directory holding release_models")
    parser.add_argument("--schema", type=Path, help="migration TypeQL for migration-typeql")
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    arguments = parser.parse_args(argv)

    import type_bridge
    from type_bridge import Database

    selected = shim(arguments.version)
    address = os.environ.get("TYPEDB_ADDRESS", "localhost:1729")
    options: dict[str, Any] = {
        "address": address,
        "username": os.environ.get("TYPEDB_USERNAME", "admin"),
        "password": os.environ.get("TYPEDB_PASSWORD", "password"),
        "http_port": int(os.environ.get("TYPEDB_HTTP_PORT", "8000")),
    }
    db = Database(database=arguments.database, **options)
    db.connect()
    try:
        if selected.models == "handwritten":
            models = handwritten_models()
        else:
            if arguments.models is None:
                parser.error("a generated-model version needs --models")
            models = generated_models(arguments.models)
        if selected.schema == "schema-manager":
            from type_bridge import SchemaManager

            if not db.database_exists():
                db.create_database()
            schema = SchemaManager(db)
            schema.register(models.item)
            schema.sync_schema()
        elif selected.schema == "migration-typeql":
            if arguments.schema is None:
                parser.error("a migration-typeql version needs --schema")
            if not db.database_exists():
                db.create_database()
            with db.transaction("schema") as tx:
                for query in split_queries(arguments.schema.read_text(encoding="utf-8")):
                    tx.execute(query)
        samples = run(db, models, iterations=arguments.iterations, warmup=arguments.warmup)
    finally:
        db.close()
        for name in (arguments.database, f"{arguments.database}__tbv2_journal"):
            cleanup = Database(database=name, **options)
            cleanup.connect()
            try:
                if cleanup.database_exists():
                    cleanup.delete_database()
            finally:
                cleanup.close()

    report = {
        "format": FORMAT,
        "version": type_bridge.__version__,
        "python": ".".join(map(str, sys.version_info[:3])),
        "ops": [{"name": name, "operations": OPS[name], "samples": samples[name]} for name in OPS],
    }
    arguments.out.parent.mkdir(parents=True, exist_ok=True)
    arguments.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
