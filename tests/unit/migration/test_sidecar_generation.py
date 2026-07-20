# pyright: reportMissingImports=false
"""Unit tests for checked sidecar generation over py-only histories.

Covers:
- A py-only RunTypeQL migration converts to a sidecar the loader then
  prefers over dynamic import, with the .py checksum embedded
- Re-running over a fully converted history writes nothing
- A history containing ops.RunPython fails with a blocker report and
  writes no files at all (all-or-nothing)
- Mixed histories convert only the py-only members
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from type_bridge import _rust_runtime
from type_bridge.migration.sidecar import SidecarConversionError, generate_sidecars

pytest.importorskip("type_bridge_core")


_RUN_TYPEQL_MIGRATION = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


class LegacyMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = {dependencies}
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL(
            forward="define attribute {attr}, value string;",
            reverse="undefine attribute {attr};",
        ),
    ]
"""

_RUN_PYTHON_MIGRATION = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


def _forward(database):
    pass


class HandAuthoredMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(_forward),
    ]
"""


def _write_run_typeql(
    migrations_dir: Path,
    filename: str,
    attr: str,
    dependencies: list[tuple[str, str]] | None = None,
) -> Path:
    migrations_dir.mkdir(parents=True, exist_ok=True)
    path = migrations_dir / filename
    path.write_text(_RUN_TYPEQL_MIGRATION.format(attr=attr, dependencies=repr(dependencies or [])))
    return path


def test_py_only_migration_converts_to_checked_sidecar(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    py_path = _write_run_typeql(migrations_dir, "0001_initial.py", "legacy-name")

    written = generate_sidecars(migrations_dir)

    sidecar_path = migrations_dir / "0001_initial.json"
    assert written == [sidecar_path]
    spec = json.loads(sidecar_path.read_text())
    assert spec["name"] == "0001_initial"
    assert spec["app_label"] == "migrations"
    assert spec["checksum"] == _rust_runtime.migration_file_checksum(py_path.read_text())
    assert spec["operations"][0]["kind"] == "run_typeql"
    assert "legacy-name" in spec["operations"][0]["forward"]


def test_generated_sidecar_is_preferred_by_the_released_loader(tmp_path: Path) -> None:
    from type_bridge.migration.loader import MigrationLoader

    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "loader-pref")
    generate_sidecars(migrations_dir)

    loaded = MigrationLoader(migrations_dir).discover()
    assert len(loaded) == 1
    assert loaded[0].execution_spec is not None
    assert loaded[0].execution_spec["operations"][0]["kind"] == "run_typeql"


def test_second_run_is_a_no_op(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "idempotent-name")

    first = generate_sidecars(migrations_dir)
    assert len(first) == 1
    assert generate_sidecars(migrations_dir) == []


def test_run_python_history_fails_closed_and_writes_nothing(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "convertible-name")
    (migrations_dir / "0002_backfill.py").write_text(_RUN_PYTHON_MIGRATION)

    with pytest.raises(SidecarConversionError) as excinfo:
        generate_sidecars(migrations_dir)

    assert "0002_backfill" in excinfo.value.blockers
    assert "RunPython" in str(excinfo.value)
    assert list(migrations_dir.glob("*.json")) == []


def test_mixed_history_converts_only_py_only_members(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "mixed-first")
    generate_sidecars(migrations_dir)
    _write_run_typeql(
        migrations_dir,
        "0002_next.py",
        "mixed-second",
        dependencies=[("migrations", "0001_initial")],
    )

    written = generate_sidecars(migrations_dir)

    assert written == [migrations_dir / "0002_next.json"]
    spec = json.loads((migrations_dir / "0002_next.json").read_text())
    assert spec["dependencies"] == [{"app_label": "migrations", "migration_name": "0001_initial"}]
