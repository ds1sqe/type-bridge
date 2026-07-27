# pyright: reportMissingImports=false
"""Unit tests for the migration file loader sidecar feature (Phase 2, sub-plan 07).

Covers:
- Generator writes a JSON sidecar beside the .py
- Sidecar-backed .py files are not executed during discovery
- Sidecar checksum drift is rejected before Python import
- Legacy .py with no sidecar → execution_spec is None, lowering falls back
- .py still contains ops.RunTypeQL text after generation
"""

from __future__ import annotations

import codecs
import os
from pathlib import Path

import pytest

from type_bridge import _rust_runtime
from type_bridge.migration._lower import lower_execution_migration
from type_bridge.migration.loader import MigrationLoader, MigrationLoadError

# ---------------------------------------------------------------------------
# Shared fixture: require the Rust extension for all tests in this module.
# ---------------------------------------------------------------------------


@pytest.fixture(autouse=True)
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


_MIGRATION_PY_TEMPLATE = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


class AutoMigration(Migration):
    \"\"\"Migration: add loader-name\"\"\"

    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL(
            forward="define attribute loader-name, value string;",
            reverse="undefine attribute loader-name;",
        ),
    ]
"""


def _write_py(tmp_path: Path, filename: str, content: str = _MIGRATION_PY_TEMPLATE) -> Path:
    """Write a migration .py file and return its path."""
    tmp_path.mkdir(parents=True, exist_ok=True)
    py_path = tmp_path / filename
    py_path.write_text(content)
    return py_path


def _build_and_write_sidecar(
    py_path: Path,
    forward: str,
    reverse: str | None,
    app_label: str,
    migration_name: str,
    dependencies: list[tuple[str, str]],
) -> Path:
    """Produce and write the JSON sidecar beside py_path.

    Mirrors the logic in MigrationGenerator._write_sidecar so the test does
    not depend on a live database.
    """
    py_content = py_path.read_text()
    checksum = _rust_runtime.migration_file_checksum(py_content)

    op_spec: dict = {"kind": "run_typeql", "forward": forward, "reverse": reverse}
    spec: dict = {
        "app_label": app_label,
        "name": migration_name,
        "dependencies": [
            {"app_label": dep_app, "migration_name": dep_name} for dep_app, dep_name in dependencies
        ],
        "operations": [op_spec],
        "checksum": checksum,
        "reversible": True,
    }
    normalized = _rust_runtime.normalize_migration_spec(spec)
    json_text = _rust_runtime.migration_spec_to_json(normalized)
    sidecar_path = py_path.with_suffix(".json")
    sidecar_path.write_text(json_text)
    return sidecar_path


# ---------------------------------------------------------------------------
# Test 1: sidecar file exists beside the .py after generation
# ---------------------------------------------------------------------------


def test_sidecar_file_exists_beside_py(tmp_path: Path) -> None:
    """After writing a sidecar, the NNNN_<name>.json exists beside NNNN_<name>.py."""
    py_path = _write_py(tmp_path, "0001_auto.py")
    sidecar_path = _build_and_write_sidecar(
        py_path,
        forward="define attribute loader-name, value string;",
        reverse="undefine attribute loader-name;",
        app_label=tmp_path.name,
        migration_name="0001_auto",
        dependencies=[],
    )

    assert py_path.exists(), ".py file must exist"
    assert sidecar_path.exists(), ".json sidecar must exist beside the .py"
    assert sidecar_path == py_path.with_suffix(".json")


# ---------------------------------------------------------------------------
# Test 2: .py still contains ops.RunTypeQL text
# ---------------------------------------------------------------------------


def test_py_contains_run_typeql(tmp_path: Path) -> None:
    """The .py file must still contain 'ops.RunTypeQL' (invariant 9 / test_autogenerate)."""
    py_path = _write_py(tmp_path, "0001_auto.py")
    content = py_path.read_text()
    assert "ops.RunTypeQL" in content


# ---------------------------------------------------------------------------
# Test 3: sidecar loaded via discover() populates execution_spec
# ---------------------------------------------------------------------------


def test_discover_populates_execution_spec_from_sidecar(tmp_path: Path) -> None:
    """LoadedMigration.execution_spec is populated when a .json sidecar is present."""
    py_path = _write_py(tmp_path, "0001_auto.py")
    _build_and_write_sidecar(
        py_path,
        forward="define attribute loader-name, value string;",
        reverse="undefine attribute loader-name;",
        app_label=tmp_path.name,
        migration_name="0001_auto",
        dependencies=[],
    )

    migrations = MigrationLoader(tmp_path).discover()
    assert len(migrations) == 1
    loaded = migrations[0]

    assert loaded.execution_spec is not None, "execution_spec must be populated from sidecar"
    assert loaded.execution_spec["name"] == "0001_auto"
    assert loaded.execution_spec["app_label"] == tmp_path.name


# ---------------------------------------------------------------------------
# Test 4: sidecar-backed discovery does not execute .py
# ---------------------------------------------------------------------------


def test_sidecar_backed_discovery_skips_python_execution(tmp_path: Path) -> None:
    """A valid sidecar lets discovery succeed even if .py imports are stale."""
    py_path = _write_py(
        tmp_path,
        "0001_auto.py",
        """
import generated_models_that_no_longer_exists
""".lstrip(),
    )
    _build_and_write_sidecar(
        py_path,
        forward="define attribute loader-name, value string;",
        reverse="undefine attribute loader-name;",
        app_label=tmp_path.name,
        migration_name="0001_auto",
        dependencies=[],
    )

    migrations = MigrationLoader(tmp_path).discover()
    loaded = migrations[0]

    assert loaded.execution_spec is not None
    assert loaded.migration.operations == []
    assert lower_execution_migration(loaded)["operations"][0]["kind"] == "run_typeql"


def test_sidecar_drift_rejected_before_python_execution(tmp_path: Path) -> None:
    py_path = _write_py(tmp_path, "0001_auto.py")
    _build_and_write_sidecar(
        py_path,
        forward="define attribute loader-name, value string;",
        reverse="undefine attribute loader-name;",
        app_label=tmp_path.name,
        migration_name="0001_auto",
        dependencies=[],
    )
    py_path.write_text(
        """
import generated_models_that_no_longer_exists
""".lstrip()
    )

    with pytest.raises(MigrationLoadError, match="sidecar drift"):
        MigrationLoader(tmp_path).discover()


# ---------------------------------------------------------------------------
# Test 5: lower_execution_migration uses sidecar spec directly when present
# ---------------------------------------------------------------------------


def test_lower_execution_migration_uses_sidecar_when_present(tmp_path: Path) -> None:
    """lower_execution_migration returns the sidecar spec (normalized) when execution_spec != None."""
    py_path = _write_py(tmp_path, "0001_auto.py")
    _build_and_write_sidecar(
        py_path,
        forward="define attribute loader-name, value string;",
        reverse="undefine attribute loader-name;",
        app_label=tmp_path.name,
        migration_name="0001_auto",
        dependencies=[],
    )

    migrations = MigrationLoader(tmp_path).discover()
    loaded = migrations[0]

    assert loaded.execution_spec is not None
    result = lower_execution_migration(loaded)

    # The result must carry the run_typeql op from the sidecar.
    assert result["operations"][0]["kind"] == "run_typeql"
    assert result["operations"][0]["forward"] == "define attribute loader-name, value string;"
    assert result["operations"][0]["reverse"] == "undefine attribute loader-name;"


# ---------------------------------------------------------------------------
# Test 6: legacy .py with no sidecar → execution_spec is None, fallback works
# ---------------------------------------------------------------------------


def test_legacy_migration_no_sidecar_execution_spec_is_none(tmp_path: Path) -> None:
    """A hand-written .py with no .json sibling loads with execution_spec=None."""
    py_path = _write_py(tmp_path, "0001_legacy.py")
    # Deliberately do NOT write a sidecar.
    assert not py_path.with_suffix(".json").exists(), "sidecar must not exist for this test"

    migrations = MigrationLoader(tmp_path).discover()
    assert len(migrations) == 1
    loaded = migrations[0]

    assert loaded.execution_spec is None, "execution_spec must be None for a legacy migration"


def test_legacy_migration_fallback_produces_valid_spec(tmp_path: Path) -> None:
    """lower_execution_migration falls back to to_typeql() for a no-sidecar migration."""
    py_path = _write_py(tmp_path, "0001_legacy.py")
    # No sidecar.

    migrations = MigrationLoader(tmp_path).discover()
    loaded = migrations[0]
    assert loaded.execution_spec is None

    spec = lower_execution_migration(loaded)

    assert spec["app_label"] == tmp_path.name
    assert spec["name"] == "0001_legacy"
    assert len(spec["operations"]) == 1
    op = spec["operations"][0]
    assert op["kind"] == "run_typeql"
    assert op["forward"] == "define attribute loader-name, value string;"
    assert op["reverse"] == "undefine attribute loader-name;"


# ---------------------------------------------------------------------------
# Test 7: both sidecar and no-sidecar migrations load through one discover()
# ---------------------------------------------------------------------------


def test_mixed_sidecar_and_legacy_both_load_through_discover(tmp_path: Path) -> None:
    """discover() handles a mix of sidecar-bearing and legacy migrations correctly."""
    # 0001: sidecar-bearing
    py1 = _write_py(tmp_path, "0001_new.py")
    _build_and_write_sidecar(
        py1,
        forward="define attribute loader-name, value string;",
        reverse="undefine attribute loader-name;",
        app_label=tmp_path.name,
        migration_name="0001_new",
        dependencies=[],
    )

    # 0002: legacy, no sidecar
    py2_content = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


class LegacyMigration(Migration):
    \"\"\"Migration: add loader-age\"\"\"

    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL(
            forward="define attribute loader-age, value long;",
        ),
    ]
"""
    _write_py(tmp_path, "0002_legacy.py", py2_content)
    # No sidecar for 0002.

    migrations = MigrationLoader(tmp_path).discover()
    assert len(migrations) == 2

    # Check by name ordering (sorted)
    new_loaded = migrations[0]
    legacy_loaded = migrations[1]

    assert new_loaded.migration.name == "0001_new"
    assert new_loaded.execution_spec is not None

    assert legacy_loaded.migration.name == "0002_legacy"
    assert legacy_loaded.execution_spec is None

    # Both produce a valid spec via lower_execution_migration.
    spec_new = lower_execution_migration(new_loaded)
    spec_legacy = lower_execution_migration(legacy_loaded)

    assert spec_new["operations"][0]["kind"] == "run_typeql"
    assert spec_legacy["operations"][0]["kind"] == "run_typeql"


def test_migration_source_read_is_bounded(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(MigrationLoader, "MAX_MIGRATION_FILE_BYTES", 32)
    (tmp_path / "0001_oversized.py").write_bytes(b"x" * 33)

    with pytest.raises(MigrationLoadError, match="byte ceiling"):
        MigrationLoader(tmp_path, adoption_limits=True).discover()


def test_migration_directory_enumeration_is_bounded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(MigrationLoader, "MAX_MIGRATION_FILES", 1)
    (tmp_path / "README.md").write_text("history")
    _write_py(tmp_path, "0001_initial.py")

    with pytest.raises(MigrationLoadError, match="file entries"):
        MigrationLoader(tmp_path, adoption_limits=True).discover()


def test_migration_source_aggregate_is_bounded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source_bytes = len(_MIGRATION_PY_TEMPLATE.encode())
    monkeypatch.setattr(MigrationLoader, "MAX_MIGRATION_HISTORY_BYTES", source_bytes + 1)
    _write_py(tmp_path, "0001_initial.py")
    _write_py(tmp_path, "0002_next.py")

    with pytest.raises(MigrationLoadError, match="history exceeds"):
        MigrationLoader(tmp_path, adoption_limits=True).discover()


def test_released_loader_accepts_valid_source_larger_than_16_mib(tmp_path: Path) -> None:
    oversized_but_valid = _MIGRATION_PY_TEMPLATE + "\n#" + ("x" * (16 * 1024 * 1024))
    _write_py(tmp_path, "0001_large.py", oversized_but_valid)

    migrations = MigrationLoader(tmp_path).discover()

    assert [loaded.migration.name for loaded in migrations] == ["0001_large"]


def test_released_loader_ignores_more_than_65536_unrelated_entries(tmp_path: Path) -> None:
    # Hard links provide real directory entries without duplicating payloads
    # or spending the runtime needed to create 65k independent file bodies.
    # Cycle across enough seed inodes to stay below conservative per-file
    # hard-link ceilings (notably NTFS) on every release platform.
    seeds = [tmp_path / f"unrelated-seed-{index:03}.txt" for index in range(128)]
    for seed in seeds:
        seed.write_text("not a migration")
    for index in range(65_537):
        os.link(seeds[index % len(seeds)], tmp_path / f"unrelated-{index:05}.txt")
    _write_py(tmp_path, "0001_initial.py")

    migrations = MigrationLoader(tmp_path).discover()

    assert [loaded.migration.name for loaded in migrations] == ["0001_initial"]


def test_released_loader_follows_source_symlink_but_adoption_reader_rejects_it(
    tmp_path: Path,
) -> None:
    source = _write_py(tmp_path, "source.py")
    linked = tmp_path / "0001_linked.py"
    try:
        linked.symlink_to(source.name)
    except OSError as error:
        pytest.skip(f"symlink creation unavailable: {error}")

    migrations = MigrationLoader(tmp_path).discover()

    assert [loaded.migration.name for loaded in migrations] == ["0001_linked"]
    with pytest.raises(MigrationLoadError, match="regular file"):
        MigrationLoader(tmp_path, adoption_limits=True).discover()


def test_adoption_rejects_a_source_replaced_after_its_bounded_capture(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original_source = _MIGRATION_PY_TEMPLATE
    replacement_source = original_source.replace("loader-name", "swapped-name")
    path = _write_py(tmp_path, "0001_initial.py", original_source)
    read_adoption_source = MigrationLoader._read_adoption_source

    def read_then_replace(loader: MigrationLoader, source_path: Path) -> bytes:
        captured = read_adoption_source(loader, source_path)
        replacement = tmp_path / "replacement.py"
        replacement.write_text(replacement_source)
        replacement.replace(source_path)
        return captured

    monkeypatch.setattr(MigrationLoader, "_read_adoption_source", read_then_replace)

    with pytest.raises(
        MigrationLoadError,
        match="authority changed during bounded adoption discovery",
    ):
        MigrationLoader(tmp_path, use_sidecars=False, adoption_limits=True).discover()

    assert path.read_text() == replacement_source


def test_adoption_rejects_in_place_source_drift_after_directory_enumeration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original_source = _MIGRATION_PY_TEMPLATE
    replacement_source = original_source.replace("loader-name", "reader-name")
    assert len(replacement_source.encode()) == len(original_source.encode())
    path = _write_py(tmp_path, "0001_initial.py", original_source)
    read_adoption_source = MigrationLoader._read_adoption_source
    mutated = False

    def mutate_then_read(loader: MigrationLoader, source_path: Path) -> bytes:
        nonlocal mutated
        if not mutated:
            mutated = True
            path.write_text(replacement_source)
        return read_adoption_source(loader, source_path)

    monkeypatch.setattr(MigrationLoader, "_read_adoption_source", mutate_then_read)

    with pytest.raises(MigrationLoadError, match="changed or is not a regular file"):
        MigrationLoader(tmp_path, use_sidecars=False, adoption_limits=True).discover()


def test_adoption_private_mirror_preserves_utf8_bom_and_never_writes_source_bytecode(
    tmp_path: Path,
) -> None:
    path = tmp_path / "0001_bom.py"
    path.write_bytes(codecs.BOM_UTF8 + _MIGRATION_PY_TEMPLATE.encode("utf-8"))

    [loaded] = MigrationLoader(
        tmp_path,
        use_sidecars=False,
        adoption_limits=True,
    ).discover()

    assert loaded.checksum == _rust_runtime.migration_file_checksum(path.read_text())
    assert loaded.source_sha256 is not None
    assert not (tmp_path / "__pycache__").exists()


def test_adoption_private_mirror_honors_utf8_pep263_source(tmp_path: Path) -> None:
    source = "# -*- coding: utf-8 -*-\n# café\n" + _MIGRATION_PY_TEMPLATE
    _write_py(tmp_path, "0001_cookie.py", source)

    [loaded] = MigrationLoader(
        tmp_path,
        use_sidecars=False,
        adoption_limits=True,
    ).discover()

    assert loaded.migration.name == "0001_cookie"


def test_released_and_adoption_discovery_accept_empty_migration_suffix(tmp_path: Path) -> None:
    _write_py(tmp_path, "0001_.py")

    released = MigrationLoader(tmp_path, use_sidecars=False).discover()
    adopted = MigrationLoader(
        tmp_path,
        use_sidecars=False,
        adoption_limits=True,
    ).discover()

    assert [item.migration.name for item in released] == ["0001_"]
    assert [item.migration.name for item in adopted] == ["0001_"]
