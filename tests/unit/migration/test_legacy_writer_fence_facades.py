"""Regression tests for V1 facade cutover ordering."""

from __future__ import annotations

from pathlib import Path
from typing import Any

import pytest

from type_bridge import _rust_runtime
from type_bridge.migration.executor import MigrationError, MigrationExecutor
from type_bridge.migration.schema_manager import SchemaManager
from type_bridge.migration.simple_migration import MigrationManager


class _Transaction:
    def __init__(
        self,
        events: list[str],
        transaction_type: str,
        query_results: list[list[dict[str, object]]],
    ) -> None:
        self.events = events
        self.transaction_type = transaction_type
        self.query_results = query_results

    def __enter__(self) -> _Transaction:
        self.events.append("enter")
        return self

    def __exit__(self, *_: object) -> None:
        self.events.append("exit")

    def execute(self, query: str) -> list[dict[str, object]]:
        self.events.append(f"execute:{query}")
        if self.query_results:
            return self.query_results.pop(0)
        return []

    def commit(self) -> None:
        self.events.append("commit")


class _Database:
    def __init__(
        self,
        events: list[str],
        *,
        exists: bool = True,
        query_results: list[list[dict[str, object]]] | None = None,
    ) -> None:
        self.events = events
        self.exists = exists
        self.query_results = [] if query_results is None else query_results

    def transaction(self, transaction_type: str) -> _Transaction:
        self.events.append(f"transaction:{transaction_type}")
        return _Transaction(self.events, transaction_type, self.query_results)

    def database_exists(self) -> bool:
        self.events.append("database_exists")
        return self.exists

    def delete_database(self) -> None:
        self.events.append("delete_database")
        self.exists = False

    def create_database(self) -> None:
        self.events.append("create_database")
        self.exists = True

    def check_schema_annotation_support(self, schema: str) -> None:
        self.events.append(f"annotation_check:{schema}")


def _simple_manager(database: Any) -> MigrationManager:
    with pytest.warns(DeprecationWarning):
        return MigrationManager(database)


def test_simple_manager_guards_the_same_transaction_before_schema_typeql(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []
    database = _Database(events)
    manager = _simple_manager(database)
    manager.add_migration("0001", "define entity person;")
    monkeypatch.setattr(
        _rust_runtime,
        "require_legacy_writer_open_in_transaction",
        lambda _tx: events.append("guard"),
    )

    manager.apply_migrations()

    assert events == [
        "transaction:schema",
        "enter",
        "guard",
        "execute:define entity person;",
        "commit",
        "exit",
    ]


def test_simple_manager_rejects_before_schema_typeql(monkeypatch: pytest.MonkeyPatch) -> None:
    events: list[str] = []
    manager = _simple_manager(_Database(events))
    manager.add_migration("0001", "define entity person;")

    def reject(_tx: object) -> None:
        events.append("guard")
        raise ValueError("cutover")

    monkeypatch.setattr(_rust_runtime, "require_legacy_writer_open_in_transaction", reject)

    with pytest.raises(ValueError, match="cutover"):
        manager.apply_migrations()

    assert not any(event.startswith("execute:") for event in events)
    assert "commit" not in events


def test_force_schema_sync_rejects_before_database_deletion(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []
    manager = SchemaManager(_Database(events))  # type: ignore[arg-type]

    def reject(_database: object) -> None:
        events.append("preflight")
        raise ValueError("cutover")

    monkeypatch.setattr(_rust_runtime, "require_legacy_writer_open", reject)

    with pytest.raises(ValueError, match="cutover"):
        manager.sync_schema(force=True)

    assert events == ["database_exists", "preflight"]


def test_drop_schema_rejects_before_database_deletion(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []
    manager = SchemaManager(_Database(events))  # type: ignore[arg-type]

    def reject(_database: object) -> None:
        events.append("preflight")
        raise RuntimeError("cutover")

    monkeypatch.setattr(_rust_runtime, "require_legacy_writer_open", reject)

    with pytest.raises(RuntimeError, match="cutover"):
        manager.drop_schema()

    assert events == ["database_exists", "preflight"]


def test_schema_sync_guards_schema_transaction_before_typeql(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []
    manager = SchemaManager(_Database(events))  # type: ignore[arg-type]
    monkeypatch.setattr(manager, "generate_schema", lambda: "define entity person;")
    monkeypatch.setattr(
        _rust_runtime,
        "require_legacy_writer_open_in_transaction",
        lambda _tx: events.append("guard"),
    )

    manager.sync_schema(skip_if_exists=True)

    assert events[-7:] == [
        "annotation_check:define entity person;",
        "transaction:schema",
        "enter",
        "guard",
        "execute:define entity person;",
        "commit",
        "exit",
    ]


def test_schema_conflict_introspection_guards_both_temporary_writes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []
    database = _Database(
        events,
        query_results=[
            [],
            [],
            [{"t": "iid", "temporary-name": "value"}],
            [],
        ],
    )
    manager = SchemaManager(database)  # type: ignore[arg-type]
    monkeypatch.setattr(
        _rust_runtime,
        "require_legacy_writer_open_in_transaction",
        lambda tx: events.append(f"guard:{tx.transaction_type}"),
    )

    attributes = manager._get_owned_attributes("temporary-person", "entity")

    assert attributes == {"temporary-name"}
    insert = next(index for index, event in enumerate(events) if "insert" in event)
    delete = next(index for index, event in enumerate(events) if "delete" in event)
    write_guards = [index for index, event in enumerate(events) if event == "guard:write"]
    assert len(write_guards) == 2
    assert write_guards[0] < insert
    assert write_guards[1] < delete


def test_schema_conflict_introspection_rejects_before_temporary_insert(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    events: list[str] = []
    database = _Database(events, query_results=[[]])
    manager = SchemaManager(database)  # type: ignore[arg-type]

    def reject(tx: _Transaction) -> None:
        events.append(f"guard:{tx.transaction_type}")
        raise RuntimeError("cutover")

    monkeypatch.setattr(_rust_runtime, "require_legacy_writer_open_in_transaction", reject)

    assert manager._get_owned_attributes("temporary-person", "entity") == set()
    assert not any("insert" in event or "delete" in event for event in events)


def test_executor_guards_the_same_transaction_before_typeql(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from type_bridge.session import Database

    events: list[str] = []
    database = object.__new__(Database)
    query_results: list[list[dict[str, object]]] = []
    database.transaction = lambda kind: _Transaction(events, kind, query_results)  # type: ignore[method-assign]
    executor = MigrationExecutor(
        database,
        migrations_dir=Path("migrations"),
        state_manager=object(),  # type: ignore[arg-type]
    )
    monkeypatch.setattr(
        _rust_runtime,
        "require_legacy_writer_open_in_transaction",
        lambda tx: events.append(f"guard:{tx.transaction_type}"),
    )

    executor._execute_typeql("define entity person;", "schema")

    assert events == [
        "enter",
        "guard:schema",
        "execute:define entity person;",
        "commit",
        "exit",
    ]


def test_executor_preserves_migration_error_and_rejects_before_typeql(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from type_bridge.session import Database

    events: list[str] = []
    database = object.__new__(Database)
    query_results: list[list[dict[str, object]]] = []
    database.transaction = lambda kind: _Transaction(events, kind, query_results)  # type: ignore[method-assign]
    executor = MigrationExecutor(
        database,
        migrations_dir=Path("migrations"),
        state_manager=object(),  # type: ignore[arg-type]
    )

    def reject(_tx: _Transaction) -> None:
        events.append("guard")
        raise RuntimeError("cutover")

    monkeypatch.setattr(_rust_runtime, "require_legacy_writer_open_in_transaction", reject)

    with pytest.raises(MigrationError, match="cutover"):
        executor._execute_typeql("define entity person;", "schema")

    assert not any(event.startswith("execute:") for event in events)
    assert "commit" not in events
