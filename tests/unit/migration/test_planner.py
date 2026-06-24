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
from typing import TYPE_CHECKING, Any, ClassVar, cast

import pytest

from type_bridge import Entity, Flag, Key, Relation, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import operations as ops
from type_bridge.migration._lower import lower_execution_graph
from type_bridge.migration.base import Migration
from type_bridge.migration.executor import MigrationError, MigrationExecutor
from type_bridge.migration.loader import LoadedMigration
from type_bridge.migration.state import (
    MigrationRecord,
    MigrationRunRecord,
    MigrationState,
    MigrationStateManager,
)

if TYPE_CHECKING:
    from type_bridge.session import Database


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


def test_non_reversible_migration_keeps_flag_for_rust_planner() -> None:
    """A non-reversible migration preserves authored ops and carries the outer flag."""

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

    migration = graph["migrations"][0]
    step = migration["operations"][0]

    assert migration["reversible"] is False
    assert step["kind"] == "run_typeql"
    assert step["forward"] == "define attribute planner-note, value string;"
    assert step["reverse"] == "undefine attribute planner-note;"


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
        self.run_calls: list[tuple[str, str, str, str, str]] = []
        self.raise_on_apply: str | None = None

    def load_state(self) -> MigrationState:
        return MigrationState()

    def record_applied(self, app_label: str, name: str, checksum: str) -> None:
        if self.raise_on_apply == name:
            raise RuntimeError("write transaction failed")
        self.calls.append(("applied", app_label, name))

    def record_unapplied(self, app_label: str, name: str) -> None:
        self.calls.append(("unapplied", app_label, name))

    def record_run_started(
        self,
        app_label: str,
        name: str,
        checksum: str,
        direction: str,
    ) -> MigrationRunRecord:
        record = MigrationRunRecord(
            run_id=f"run-{name}",
            app_label=app_label,
            name=name,
            checksum=checksum,
            direction=direction,
            status="started",
            started_at="2026-06-23T00:00:00.000000",
        )
        self.run_calls.append(("started", app_label, name, checksum, direction))
        return record

    def record_run_finished(
        self,
        record: MigrationRunRecord,
        status: str,
        error: str | None = None,
    ) -> MigrationRunRecord:
        self.run_calls.append((status, record.app_label, record.name, record.checksum, error or ""))
        return MigrationRunRecord(
            run_id=record.run_id,
            app_label=record.app_label,
            name=record.name,
            checksum=record.checksum,
            direction=record.direction,
            status=status,
            started_at=record.started_at,
            finished_at="2026-06-23T00:00:01.000000",
            error=error,
        )


class _RecordingDb:
    def __init__(self) -> None:
        self.queries: list[tuple[str, str]] = []

    def execute_query(self, query: str, *, transaction_type: str) -> None:
        self.queries.append((query, transaction_type))


def _executor_with(
    monkeypatch: pytest.MonkeyPatch,
    *,
    runner: _ScriptedRunner,
    state_manager: _RecordingStateManager,
    loaded: list[LoadedMigration],
) -> MigrationExecutor:
    dummy_db: Any = object()
    executor = MigrationExecutor(db=dummy_db, migrations_dir=Path("migrations"))
    executor.state_manager = cast(MigrationStateManager, state_manager)

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
    class _RunTypeQLMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL(forward=f"define attribute {name}-a, value string;")
        ]

    return _RunTypeQLMigration()


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
    state_manager.load_state = lambda: MigrationState(
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


# ── RunPython execution path ─────────────────────────────────────────────────


def test_run_python_migration_receives_db_and_records_state(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """RunPython executes in Python with the executor DB and records state."""
    dummy_db: Any = object()
    calls: list[Any] = []

    def forwards(db: Any) -> None:
        calls.append(db)

    class PythonMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [ops.RunPython(forwards)]

    state_manager = _RecordingStateManager()
    loaded = [_loaded(PythonMigration(), "app", "0001_python", "csum-py")]
    executor = MigrationExecutor(db=dummy_db, migrations_dir=Path("migrations"))
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)

    def fail_if_called(_db: object) -> object:
        pytest.fail("RunPython migrations must not be sent to the Rust TypeQL runner")

    monkeypatch.setattr("type_bridge._rust_runtime.migration_runner_for", fail_if_called)

    results = executor.migrate()

    assert calls == [dummy_db]
    assert [(result.name, result.action, result.success) for result in results] == [
        ("0001_python", "applied", True)
    ]
    assert state_manager.calls == [("applied", "app", "0001_python")]
    assert state_manager.run_calls == [
        ("started", "app", "0001_python", "csum-py", "apply"),
        ("succeeded", "app", "0001_python", "csum-py", ""),
    ]


def test_run_python_rollback_uses_reverse_callable_and_records_unapplied(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[str] = []

    def forwards(db: Any) -> None:
        calls.append("forwards")

    def backwards(db: Any) -> None:
        calls.append("backwards")

    class BaseMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = []

    class PythonMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("app", "0001_base")]
        operations: ClassVar[list[ops.Operation]] = [ops.RunPython(forwards, reverse=backwards)]

    state_manager = _RecordingStateManager()
    state_manager.load_state = lambda: MigrationState(
        applied=[
            MigrationRecord("app", "0001_base", "2026-01-01T00:00:00", "csum-base"),
            MigrationRecord("app", "0002_python", "2026-01-01T00:00:00", "csum-py"),
        ]
    )
    loaded = [
        _loaded(BaseMigration(), "app", "0001_base", "csum-base"),
        _loaded(PythonMigration(), "app", "0002_python", "csum-py"),
    ]
    executor = MigrationExecutor(db=cast("Database", object()), migrations_dir=Path("migrations"))
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)

    results = executor.migrate(target="0001_base")

    assert calls == ["backwards"]
    assert [(result.name, result.action, result.success) for result in results] == [
        ("0002_python", "rolled_back", True)
    ]
    assert state_manager.calls == [("unapplied", "app", "0002_python")]
    assert state_manager.run_calls == [
        ("started", "app", "0002_python", "csum-py", "rollback"),
        ("succeeded", "app", "0002_python", "csum-py", ""),
    ]


def test_run_python_preflights_declared_resource_and_import(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    resource_dir = tmp_path / "data"
    resource_dir.mkdir()
    (resource_dir / "users.json").write_text("[]")
    calls: list[Any] = []

    def forwards(db: Any) -> None:
        calls.append(db)

    class PythonMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunPython(
                forwards,
                resources=["data/users.json"],
                import_checks=["json"],
            )
        ]

    state_manager = _RecordingStateManager()
    migration = PythonMigration()
    migration.app_label = "app"
    migration.name = "0001_python"
    loaded = [
        LoadedMigration(
            migration=migration,
            path=tmp_path / "0001_python.py",
            checksum="csum-py",
        )
    ]
    executor = MigrationExecutor(db=cast("Database", object()), migrations_dir=tmp_path)
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)

    results = executor.migrate()

    assert calls
    assert [(result.name, result.action, result.success) for result in results] == [
        ("0001_python", "applied", True)
    ]
    assert state_manager.calls == [("applied", "app", "0001_python")]


def test_run_python_missing_resource_preflight_blocks_entire_plan(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    calls: list[str] = []

    def first(db: Any) -> None:
        calls.append("first")

    def second(db: Any) -> None:
        calls.append("second")

    class FirstMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [ops.RunPython(first)]

    class SecondMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("app", "0001_first")]
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunPython(second, resources=["data/missing.json"])
        ]

    state_manager = _RecordingStateManager()
    loaded = [
        LoadedMigration(
            migration=FirstMigration(),
            path=tmp_path / "0001_first.py",
            checksum="csum-first",
        ),
        LoadedMigration(
            migration=SecondMigration(),
            path=tmp_path / "0002_second.py",
            checksum="csum-second",
        ),
    ]
    for item in loaded:
        item.migration.app_label = "app"
        item.migration.name = item.path.stem

    executor = MigrationExecutor(db=cast("Database", object()), migrations_dir=tmp_path)
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)

    with pytest.raises(MigrationError, match="missing resource"):
        executor.migrate()

    assert calls == []
    assert state_manager.calls == []


def test_run_python_missing_import_check_fails_before_user_code(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[str] = []

    def forwards(db: Any) -> None:
        calls.append("forwards")

    class PythonMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunPython(
                forwards,
                import_checks=["type_bridge_missing_test_module"],
            )
        ]

    state_manager = _RecordingStateManager()
    loaded = [_loaded(PythonMigration(), "app", "0001_python", "csum-py")]
    executor = MigrationExecutor(db=cast("Database", object()), migrations_dir=Path("migrations"))
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)

    with pytest.raises(MigrationError, match="failed import check"):
        executor.migrate()

    assert calls == []
    assert state_manager.calls == []


def test_run_python_history_uses_python_path_for_later_typeql_migration(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def forwards(db: Any) -> None:
        pass

    class PythonMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [ops.RunPython(forwards)]

    class TypeQLMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("app", "0001_python")]
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL("define attribute later-typeql, value string;")
        ]

    state_manager = _RecordingStateManager()
    state_manager.load_state = lambda: MigrationState(
        applied=[
            MigrationRecord("app", "0001_python", "2026-01-01T00:00:00", "csum-py"),
        ]
    )
    loaded = [
        _loaded(PythonMigration(), "app", "0001_python", "csum-py"),
        _loaded(TypeQLMigration(), "app", "0002_typeql", "csum-typeql"),
    ]
    db = _RecordingDb()
    executor = MigrationExecutor(db=cast("Database", db), migrations_dir=Path("migrations"))
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)

    runner = _ScriptedRunner(
        [
            {
                "app_label": "app",
                "name": "0002_typeql",
                "action": "apply",
                "success": True,
                "error": None,
            },
        ]
    )

    monkeypatch.setattr("type_bridge._rust_runtime.migration_runner_for", lambda db: runner)

    results = executor.migrate()

    assert [(result.name, result.action, result.success) for result in results] == [
        ("0002_typeql", "applied", True)
    ]
    assert len(runner.apply_calls) == 1
    assert runner.apply_calls[0][2] == "0002_typeql"
    assert len(runner.apply_calls[0][0]["migrations"]) == 2
    assert db.queries == []
    assert state_manager.calls == [("applied", "app", "0002_typeql")]


def test_run_python_plan_executes_sidecar_migration_via_rust(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    calls: list[Any] = []

    def forwards(db: Any) -> None:
        calls.append("run_python")

    class PythonMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("app", "0001_initial")]
        operations: ClassVar[list[ops.Operation]] = [ops.RunPython(forwards)]

    sidecar_migration = _loaded(
        _runtypeql_migration("unused"), "app", "0001_initial", "csum-initial"
    )
    sidecar_migration.migration.name = "0001_initial"
    sidecar_migration.migration.app_label = "app"
    sidecar_migration.execution_spec = {
        "app_label": "app",
        "name": "0001_initial",
        "dependencies": [],
        "operations": [
            {
                "kind": "run_typeql",
                "forward": "define attribute sidecar-attr, value string;",
                "reverse": "undefine attribute sidecar-attr;",
            }
        ],
        "checksum": "csum-initial",
        "reversible": True,
    }

    runner = _ScriptedRunner(
        [
            {
                "app_label": "app",
                "name": "0001_initial",
                "action": "apply",
                "success": True,
                "error": None,
            },
        ]
    )
    loaded = [
        sidecar_migration,
        _loaded(PythonMigration(), "app", "0002_python", "csum-python"),
    ]

    state_manager = _RecordingStateManager()
    dummy_db: Any = object()
    executor = MigrationExecutor(db=dummy_db, migrations_dir=Path("migrations"))
    executor.state_manager = cast(MigrationStateManager, state_manager)
    monkeypatch.setattr(executor.loader, "discover", lambda: loaded)
    monkeypatch.setattr("type_bridge._rust_runtime.migration_runner_for", lambda db: runner)

    results = executor.migrate()

    assert calls == ["run_python"]
    assert [(result.name, result.action, result.success) for result in results] == [
        ("0001_initial", "applied", True),
        ("0002_python", "applied", True),
    ]
    assert state_manager.calls == [
        ("applied", "app", "0001_initial"),
        ("applied", "app", "0002_python"),
    ]
    assert len(runner.apply_calls) == 1
    assert runner.apply_calls[0][2] == "0001_initial"


def test_run_python_sqlmigrate_preview_is_comment(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def forwards(db: Any) -> None:
        pass

    def backwards(db: Any) -> None:
        pass

    class PythonMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunPython(
                forwards,
                reverse=backwards,
                resources=["data/users.json"],
                import_checks=["json"],
            )
        ]

    loaded = _loaded(PythonMigration(), "app", "0001_python", "csum-py")
    executor = MigrationExecutor(db=cast("Database", object()), migrations_dir=Path("migrations"))
    monkeypatch.setattr(executor.loader, "get_by_name", lambda name: loaded)

    preview = executor.sqlmigrate("0001_python")
    assert "RunPython" in preview
    assert "data/users.json" in preview
    assert "json" in preview
    assert "RunPython reverse" in executor.sqlmigrate("0001_python", reverse=True)


def test_run_python_sqlmigrate_reverse_requires_reverse_callable(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def forwards(db: Any) -> None:
        pass

    class PythonMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [ops.RunPython(forwards)]

    loaded = _loaded(PythonMigration(), "app", "0001_python", "csum-py")
    executor = MigrationExecutor(db=cast("Database", object()), migrations_dir=Path("migrations"))
    monkeypatch.setattr(executor.loader, "get_by_name", lambda name: loaded)

    with pytest.raises(MigrationError, match="not reversible"):
        executor.sqlmigrate("0001_python", reverse=True)


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

    dummy_db: Any = object()
    executor = MigrationExecutor(db=dummy_db, migrations_dir=Path("migrations"))
    monkeypatch.setattr(executor.loader, "get_by_name", lambda name: loaded)

    forward = executor.sqlmigrate("0001_preview")
    expected = (
        "define attribute planner-x, value string;\n\ndefine attribute planner-y, value string;"
    )
    assert forward == expected

    # Reverse preview joins reverses in reverse step order.
    reverse = executor.sqlmigrate("0001_preview", reverse=True)
    assert reverse == "undefine attribute planner-y;\n\nundefine attribute planner-x;"


def test_sqlmigrate_preview_uses_rust_planner_for_typed_ops(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Typed OperationSpec previews are lowered by Rust, not Python to_typeql()."""

    class PreviewTypedMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.AddAttribute(PlannerName),
            ops.AddEntity(PlannerPerson),
        ]

    loaded = _loaded(PreviewTypedMigration(), "planner", "0001_typed", "csum")

    dummy_db: Any = object()
    executor = MigrationExecutor(db=dummy_db, migrations_dir=Path("migrations"))
    monkeypatch.setattr(executor.loader, "get_by_name", lambda name: loaded)

    forward = executor.sqlmigrate("0001_typed")

    assert "attribute planner-name, value string;" in forward
    assert "entity planner-person," in forward
    assert "owns planner-name @key;" in forward

    reverse = executor.sqlmigrate("0001_typed", reverse=True)
    assert reverse == "undefine\nplanner-person;\n\nundefine\nplanner-name;"
