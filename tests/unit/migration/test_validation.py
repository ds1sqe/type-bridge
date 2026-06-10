from __future__ import annotations

# pyright: reportMissingImports=false
import hashlib
from pathlib import Path
from typing import ClassVar

import pytest

from type_bridge import _rust_runtime
from type_bridge.migration import operations as ops
from type_bridge.migration.base import Migration
from type_bridge.migration.executor import MigrationError, MigrationExecutor
from type_bridge.migration.loader import LoadedMigration, MigrationLoader
from type_bridge.migration.state import (
    MigrationRecord,
    MigrationState,
    MigrationStateManager,
)


@pytest.fixture(autouse=True)
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


def _graph(*migrations: dict[str, object]) -> dict[str, object]:
    return {"migrations": list(migrations)}


def _spec(
    name: str,
    *,
    dependencies: list[dict[str, str]] | None = None,
    checksum: str | None = "abc",
) -> dict[str, object]:
    return {
        "app_label": "app",
        "name": name,
        "dependencies": dependencies or [],
        "operations": [],
        "checksum": checksum,
        "reversible": True,
    }


def _write_migration(
    path: Path,
    *,
    dependencies: list[tuple[str, str]] | None = None,
    typeql: str = "define attribute validation-name, value string;",
) -> None:
    path.write_text(
        f"""
from typing import ClassVar
from type_bridge.migration import Migration, operations as ops
from type_bridge.migration.operations import Operation


class TestMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = {dependencies or []!r}
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL({typeql!r})
    ]
""".lstrip()
    )


def test_rust_checksum_matches_historical_python_hashlib_prefix() -> None:
    content = "define attribute validation-name, value string;\n"

    assert (
        _rust_runtime.migration_file_checksum(content)
        == hashlib.sha256(content.encode()).hexdigest()[:16]
    )


def test_validate_migration_graph_reports_duplicate_identity() -> None:
    errors = _rust_runtime.validate_migration_graph(
        _graph(_spec("0001_initial"), _spec("0001_initial"))
    )

    assert [error["code"] for error in errors] == ["duplicate_migration"]


def test_validate_migration_graph_reports_duplicate_number() -> None:
    errors = _rust_runtime.validate_migration_graph(
        _graph(_spec("0001_initial"), _spec("0001_other"))
    )

    assert [error["code"] for error in errors] == ["duplicate_migration_number"]


def test_loader_validate_dependencies_preserves_list_of_strings(tmp_path: Path) -> None:
    _write_migration(
        tmp_path / "0001_missing_dep.py",
        dependencies=[("missing", "0001_initial")],
    )

    errors = MigrationLoader(tmp_path).validate_dependencies()

    assert isinstance(errors, list)
    assert errors
    assert "depends on missing.0001_initial which does not exist" in errors[0]


def test_valid_graph_with_applied_records_passes() -> None:
    graph = _graph(
        _spec("0001_initial", checksum="aaa"),
        _spec(
            "0002_next",
            dependencies=[{"app_label": "app", "migration_name": "0001_initial"}],
            checksum="bbb",
        ),
    )
    applied = [
        {
            "app_label": "app",
            "name": "0001_initial",
            "checksum": "aaa",
            "applied_at": "2026-06-05T00:00:00",
        }
    ]

    assert _rust_runtime.validate_migration_graph(graph, applied) == []
    _rust_runtime.check_migration_drift(graph, applied)


def test_drift_gate_rejects_changed_applied_checksum() -> None:
    graph = _graph(_spec("0001_initial", checksum="current"))
    applied = [
        {
            "app_label": "app",
            "name": "0001_initial",
            "checksum": "stored",
            "applied_at": "2026-06-05T00:00:00",
        }
    ]

    with pytest.raises(ValueError, match="checksum drifted"):
        _rust_runtime.check_migration_drift(graph, applied)


def test_executor_preflight_fails_before_apply_on_drift(monkeypatch: pytest.MonkeyPatch) -> None:
    class DriftMigration(Migration):
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL("define attribute drift-validation, value string;")
        ]

    migration = DriftMigration()
    migration.app_label = "app"
    migration.name = "0001_initial"
    loaded = LoadedMigration(migration, Path("0001_initial.py"), "current")

    executor = MigrationExecutor.__new__(MigrationExecutor)
    executor.loader = _StaticLoader([loaded])
    executor.state_manager = _StaticStateManager(
        MigrationState(
            applied=[
                MigrationRecord(
                    app_label="app",
                    name="0001_initial",
                    applied_at="2026-06-05T00:00:00",
                    checksum="stored",
                )
            ]
        )
    )

    called = False

    def fail_if_called(_db: object) -> object:
        nonlocal called
        called = True
        pytest.fail("execution must not start after checksum drift")

    # Execution flows through the Rust runner, not a Python apply method; the
    # preflight drift gate must raise before the runner is ever constructed.
    monkeypatch.setattr(_rust_runtime, "migration_runner_for", fail_if_called)

    with pytest.raises(MigrationError, match="checksum drift"):
        executor.migrate()

    assert not called


class _StaticLoader(MigrationLoader):
    def __init__(self, migrations: list[LoadedMigration]):
        self._migrations = migrations

    def discover(self) -> list[LoadedMigration]:
        return self._migrations


class _StaticStateManager(MigrationStateManager):
    def __init__(self, state: MigrationState):
        self._fixed_state = state

    def load_state(self) -> MigrationState:
        return self._fixed_state
