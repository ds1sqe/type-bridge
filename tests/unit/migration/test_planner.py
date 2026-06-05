"""Unit tests for the execution-step lowering and the executor cutover.

These cover the Python boundary of sub-plan 05 (#137): the execution-step
lowering (`_lower.lower_execution_graph`), and `MigrationExecutor.migrate()`'s
result-list-to-state-record coupling once execution moved into Rust. The Rust
runner is monkeypatched here; the live-execution path is exercised by
`tests/integration/migration/test_autogenerate.py`.
"""

from __future__ import annotations

# pyright: reportMissingImports=false
from pathlib import Path
from typing import Any, ClassVar

import pytest

from type_bridge import Entity, Flag, Key, Relation, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import operations as ops
from type_bridge.migration._lower import lower_execution_graph
from type_bridge.migration.base import Migration
from type_bridge.migration.executor import MigrationError, MigrationExecutor
from type_bridge.migration.loader import LoadedMigration
from type_bridge.migration.state import MigrationRecord, MigrationState


class PlannerName(String):
    flags = AttributeFlags(name="planner-name")


class PlannerPerson(Entity):
    flags = TypeFlags(name="planner-person")

    name: PlannerName = Flag(Key)


@pytest.fixture(autouse=True)
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


def _loaded(migration: Migration, app_label: str, name: str, checksum: str) -> LoadedMigration:
    migration.app_label = app_label
    migration.name = name
    return LoadedMigration(migration=migration, path=Path(f"{name}.py"), checksum=checksum)


# ── execution-step lowering ──────────────────────────────────────────────────


def test_mixed_migration_lowers_to_execution_steps() -> None:
    """A model-initial migration and a RunTypeQL migration lower to the right kinds."""

    class InitialMigration(Migration):
        models: ClassVar[list[type[Entity | Relation]]] = [PlannerPerson]

    class AddAttrMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("planner", "0001_initial")]
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL(
                forward="define attribute planner-age, value long;",
                reverse="undefine attribute planner-age;",
            )
        ]

    graph = lower_execution_graph(
        [
            _loaded(InitialMigration(), "planner", "0001_initial", "csum1"),
            _loaded(AddAttrMigration(), "planner", "0002_add_age", "csum2"),
        ]
    )

    migrations = graph["migrations"]
    assert [m["name"] for m in migrations] == ["0001_initial", "0002_add_age"]

    # Model-initial migration carries a DefineSchema step (non-reversible).
    initial_ops = migrations[0]["operations"]
    assert len(initial_ops) == 1
    assert initial_ops[0]["kind"] == "define_schema"
    assert "planner-person" in initial_ops[0]["schema"]["entities"]

    # Incremental migration carries a RunTypeQL step with forward + reverse.
    add_ops = migrations[1]["operations"]
    assert add_ops == [
        {
            "kind": "run_typeql",
            "forward": "define attribute planner-age, value long;",
            "reverse": "undefine attribute planner-age;",
        }
    ]


def test_non_reversible_migration_drops_reverses() -> None:
    """A migration flagged non-reversible carries None reverses on every step."""

    class IrreversibleMigration(Migration):
        reversible: ClassVar[bool] = False
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL(
                forward="define attribute planner-note, value string;",
                reverse="undefine attribute planner-note;",
            )
        ]

    graph = lower_execution_graph(
        [_loaded(IrreversibleMigration(), "planner", "0001_note", "csum")]
    )

    step = graph["migrations"][0]["operations"][0]
    assert step["kind"] == "run_typeql"
    assert step["forward"] == "define attribute planner-note, value string;"
    assert step["reverse"] is None


# ── migrate() ↔ state-record coupling (oracle risk 2) ────────────────────────


class _ScriptedRunner:
    """Stand-in PyMigrationRunner returning a fixed, ordered result list."""

    def __init__(self, results: list[dict[str, Any]]):
        self._results = results
        self.apply_calls: list[Any] = []

    def apply(self, graph: Any, applied_records: Any, target: Any) -> list[dict[str, Any]]:
        self.apply_calls.append((graph, applied_records, target))
        return self._results


class _RecordingStateManager:
    """Records record_applied / record_unapplied calls in order."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, str, str]] = []
        self.raise_on_apply: str | None = None

    def load_state(self) -> MigrationState:
        return MigrationState()

    def record_applied(self, app_label: str, name: str, checksum: str) -> None:
        if self.raise_on_apply == name:
            raise RuntimeError("write transaction failed")
        self.calls.append(("applied", app_label, name))

    def record_unapplied(self, app_label: str, name: str) -> None:
        self.calls.append(("unapplied", app_label, name))


def _executor_with(
    monkeypatch: pytest.MonkeyPatch,
    *,
    runner: _ScriptedRunner,
    state_manager: _RecordingStateManager,
    loaded: list[LoadedMigration],
) -> MigrationExecutor:
    executor = MigrationExecutor(db=object(), migrations_dir=Path("migrations"))  # type: ignore[arg-type]
    executor.state_manager = state_manager  # type: ignore[assignment]

    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)
    # Preflight and execution lowering call into Rust serde / validation; the
    # coupling under test is purely the result-list → state-record mapping, so
    # bypass the live-graph preflight and lowering here.
    monkeypatch.setattr(executor, "_preflight_migrations", lambda *a, **k: None)
    monkeypatch.setattr(
        "type_bridge.migration._lower.lower_execution_graph", lambda loaded: {"migrations": []}
    )
    monkeypatch.setattr("type_bridge._rust_runtime.migration_runner_for", lambda db: runner)
    return executor


def _runtypeql_migration(name: str) -> Migration:
    migration = Migration()
    migration.operations = [  # type: ignore[misc]
        ops.RunTypeQL(forward=f"define attribute {name}-a, value string;")
    ]
    return migration


def test_migrate_records_state_in_result_order(monkeypatch: pytest.MonkeyPatch) -> None:
    """record_applied is called once per applied result, in result order."""
    runner = _ScriptedRunner(
        [
            {
                "app_label": "app",
                "name": "0001_a",
                "action": "apply",
                "success": True,
                "error": None,
            },
            {
                "app_label": "app",
                "name": "0002_b",
                "action": "apply",
                "success": True,
                "error": None,
            },
        ]
    )
    state_manager = _RecordingStateManager()
    loaded = [
        _loaded(_runtypeql_migration("a"), "app", "0001_a", "csum-a"),
        _loaded(_runtypeql_migration("b"), "app", "0002_b", "csum-b"),
    ]
    executor = _executor_with(
        monkeypatch, runner=runner, state_manager=state_manager, loaded=loaded
    )

    results = executor.migrate()

    assert [r.name for r in results] == ["0001_a", "0002_b"]
    assert all(r.action == "applied" and r.success for r in results)
    # One record_applied per result, in result order, with carried checksums.
    assert state_manager.calls == [
        ("applied", "app", "0001_a"),
        ("applied", "app", "0002_b"),
    ]


def test_migrate_records_unapplied_for_rollback_results(monkeypatch: pytest.MonkeyPatch) -> None:
    """record_unapplied is called once per successful rollback result, in order."""
    runner = _ScriptedRunner(
        [
            {
                "app_label": "app",
                "name": "0002_b",
                "action": "rollback",
                "success": True,
                "error": None,
            },
        ]
    )
    state_manager = _RecordingStateManager()
    state_manager.load_state = lambda: MigrationState(  # type: ignore[method-assign]
        applied=[
            MigrationRecord(
                app_label="app", name="0002_b", applied_at="2026-01-01T00:00:00", checksum="csum-b"
            )
        ]
    )
    loaded = [
        _loaded(_runtypeql_migration("a"), "app", "0001_a", "csum-a"),
        _loaded(_runtypeql_migration("b"), "app", "0002_b", "csum-b"),
    ]
    executor = _executor_with(
        monkeypatch, runner=runner, state_manager=state_manager, loaded=loaded
    )

    results = executor.migrate(target="0001_a")

    assert [r.name for r in results] == ["0002_b"]
    assert results[0].action == "rolled_back" and results[0].success
    assert state_manager.calls == [("unapplied", "app", "0002_b")]


def test_migrate_raises_on_failed_result(monkeypatch: pytest.MonkeyPatch) -> None:
    """A Rust result with success=False raises MigrationError naming the failure."""
    runner = _ScriptedRunner(
        [
            {
                "app_label": "app",
                "name": "0001_a",
                "action": "apply",
                "success": True,
                "error": None,
            },
            {
                "app_label": "app",
                "name": "0002_b",
                "action": "apply",
                "success": False,
                "error": "query failed: boom",
            },
        ]
    )
    state_manager = _RecordingStateManager()
    loaded = [
        _loaded(_runtypeql_migration("a"), "app", "0001_a", "csum-a"),
        _loaded(_runtypeql_migration("b"), "app", "0002_b", "csum-b"),
    ]
    executor = _executor_with(
        monkeypatch, runner=runner, state_manager=state_manager, loaded=loaded
    )

    with pytest.raises(MigrationError, match="boom"):
        executor.migrate()

    # First result still recorded before the failure halted the loop.
    assert state_manager.calls == [("applied", "app", "0001_a")]


# ── record-failure surfacing (05/06 boundary) ────────────────────────────────


def test_record_applied_failure_surfaces_migration_error(monkeypatch: pytest.MonkeyPatch) -> None:
    """A record_applied failure after a successful Rust apply names the migration."""
    runner = _ScriptedRunner(
        [
            {
                "app_label": "app",
                "name": "0001_a",
                "action": "apply",
                "success": True,
                "error": None,
            },
        ]
    )
    state_manager = _RecordingStateManager()
    state_manager.raise_on_apply = "0001_a"
    loaded = [_loaded(_runtypeql_migration("a"), "app", "0001_a", "csum-a")]
    executor = _executor_with(
        monkeypatch, runner=runner, state_manager=state_manager, loaded=loaded
    )

    with pytest.raises(MigrationError, match="0001_a"):
        executor.migrate()


# ── sqlmigrate preview parity ────────────────────────────────────────────────


def test_sqlmigrate_preview_equals_lowered_forward(monkeypatch: pytest.MonkeyPatch) -> None:
    """sqlmigrate output equals the forward TypeQL of the lowered execution steps."""

    class PreviewMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL(
                forward="define attribute planner-x, value string;",
                reverse="undefine attribute planner-x;",
            ),
            ops.RunTypeQL(
                forward="define attribute planner-y, value string;",
                reverse="undefine attribute planner-y;",
            ),
        ]

    loaded = _loaded(PreviewMigration(), "planner", "0001_preview", "csum")

    executor = MigrationExecutor(db=object(), migrations_dir=Path("migrations"))  # type: ignore[arg-type]
    monkeypatch.setattr(executor.loader, "get_by_name", lambda name: loaded)

    forward = executor.sqlmigrate("0001_preview")
    expected = (
        "define attribute planner-x, value string;\n\ndefine attribute planner-y, value string;"
    )
    assert forward == expected

    # Reverse preview joins reverses in reverse step order.
    reverse = executor.sqlmigrate("0001_preview", reverse=True)
    assert reverse == "undefine attribute planner-y;\n\nundefine attribute planner-x;"
