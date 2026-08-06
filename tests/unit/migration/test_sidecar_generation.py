# pyright: reportMissingImports=false
"""Unit tests for checked sidecar generation over py-only histories.

Covers:
- A py-only RunTypeQL migration converts to a sidecar the loader then
  prefers over dynamic import, with the .py checksum embedded
- Re-running over a fully converted history writes nothing
- A history containing ops.RunPython receives non-executable archival
  adoption metadata without pretending the operation can run natively
- Released empty migrations inherit one exact parent snapshot authority
- Mixed histories convert only the py-only members
"""

from __future__ import annotations

import hashlib
import importlib.abc
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from type_bridge import _rust_runtime
from type_bridge.migration import _adoption_import as adoption_import_module
from type_bridge.migration import sidecar as sidecar_module
from type_bridge.migration._adoption_authority import (
    AdoptionDirectoryAuthority,
    AdoptionDirectoryEntry,
)
from type_bridge.migration._archive_base import _ArchivedMigration as Migration
from type_bridge.migration._lower import lower_migration
from type_bridge.migration.loader import LoadedMigration, MigrationLoader
from type_bridge.migration.sidecar import SidecarConversionError, generate_sidecars


def test_documented_module_command_does_not_preimport_its_runpy_target() -> None:
    environment = os.environ.copy()
    environment["PYTHONWARNINGS"] = "error"
    result = subprocess.run(
        [sys.executable, "-m", "type_bridge.migration.sidecar", "--help"],
        check=False,
        capture_output=True,
        text=True,
        env=environment,
    )

    assert result.returncode == 0, result.stderr
    assert result.stderr == ""
    assert "Generate checked adoption metadata" in result.stdout


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
    dependencies: ClassVar[list[tuple[str, str]]] = {dependencies}
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(_forward),
    ]
"""

_EMPTY_MIGRATION = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation


class EmptyMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = {dependencies}
    operations: ClassVar[list[Operation]] = []
"""

_COPY_ATTRIBUTE_MIGRATION = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops
from type_bridge import Entity, TypeFlags


class CopyPerson(Entity):
    flags = TypeFlags(name="person")


class CopyMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = {dependencies}
    operations: ClassVar[list[Operation]] = [
        ops.CopyAttribute(owner=CopyPerson, source="old-name", dest="new-name"),
    ]
"""

_IGNORED_NOTES = '"""Historical migration notes retained beside the released history."""\n'

_IGNORED_DISABLED = """\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation


class _DisabledMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = []
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


def _write_run_python(
    migrations_dir: Path,
    filename: str,
    *,
    dependencies: list[tuple[str, str]],
) -> Path:
    migrations_dir.mkdir(parents=True, exist_ok=True)
    path = migrations_dir / filename
    path.write_text(_RUN_PYTHON_MIGRATION.format(dependencies=repr(dependencies)))
    return path


def _write_empty(
    migrations_dir: Path,
    filename: str,
    *,
    dependencies: list[tuple[str, str]],
) -> Path:
    migrations_dir.mkdir(parents=True, exist_ok=True)
    path = migrations_dir / filename
    path.write_text(_EMPTY_MIGRATION.format(dependencies=repr(dependencies)))
    return path


def _write_copy_attribute(
    migrations_dir: Path,
    filename: str,
    *,
    dependencies: list[tuple[str, str]],
) -> Path:
    migrations_dir.mkdir(parents=True, exist_ok=True)
    path = migrations_dir / filename
    path.write_text(_COPY_ATTRIBUTE_MIGRATION.format(dependencies=repr(dependencies)))
    return path


def _write_snapshot(
    migrations_dir: Path,
    version: str,
    source_migration: str,
    schema: str,
) -> str:
    schema_hash = hashlib.sha256(schema.encode()).hexdigest()
    snapshot = migrations_dir / "snapshots" / version
    snapshot.mkdir(parents=True)
    (snapshot / "schema.tql").write_text(schema)
    (snapshot / "snapshot.json").write_text(
        json.dumps(
            {
                "version": version,
                "source_migration": source_migration,
                "schema_hash": schema_hash,
                "file_hashes": {"schema.tql": schema_hash},
                "type_bridge_version": "1.5.11",
                "type_bridge_core_version": "1.5.11",
            }
        )
    )
    return schema_hash


def _write_ignored(migrations_dir: Path, filename: str, source: str) -> Path:
    migrations_dir.mkdir(parents=True, exist_ok=True)
    path = migrations_dir / filename
    path.write_text(source)
    return path


def _loaded_empty(name: str, dependencies: list[tuple[str, str]]) -> LoadedMigration:
    migration_type = type(
        f"Empty_{name}",
        (Migration,),
        {"dependencies": dependencies, "models": [], "operations": []},
    )
    migration = migration_type()
    migration.app_label = "migrations"
    migration.name = name
    return LoadedMigration(
        migration=migration,
        path=Path("/not-read") / f"{name}.py",
        checksum="0123456789abcdef",
    )


def test_py_only_migration_converts_to_checked_sidecar(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    py_path = _write_run_typeql(migrations_dir, "0001_initial.py", "legacy-name")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute legacy-name, value string;\n",
    )

    written = generate_sidecars(migrations_dir)

    sidecar_path = migrations_dir / "0001_initial.json"
    assert written == [migrations_dir / "0001_initial.adoption.json", sidecar_path]
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
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute loader-pref, value string;\n",
    )
    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()

    loaded = MigrationLoader(migrations_dir).discover()
    assert len(loaded) == 1
    assert loaded[0].execution_spec is not None
    assert loaded[0].execution_spec["operations"][0]["kind"] == "run_typeql"


def test_second_run_is_a_no_op(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "idempotent-name")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute idempotent-name, value string;\n",
    )

    first = generate_sidecars(migrations_dir)
    assert first == [
        migrations_dir / "0001_initial.adoption.json",
        migrations_dir / "0001_initial.json",
    ]
    assert generate_sidecars(migrations_dir) == []


def test_v1_ignored_sources_are_checksum_bound_but_stay_out_of_history(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    notes_path = _write_ignored(migrations_dir, "0000_notes.py", _IGNORED_NOTES)
    _write_run_typeql(migrations_dir, "0001_initial.py", "with-ignored-sources")
    disabled_path = _write_ignored(
        migrations_dir,
        "0002_disabled.py",
        _IGNORED_DISABLED,
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute with-ignored-sources, value string;\n",
    )

    written = generate_sidecars(migrations_dir)

    assert written == [
        migrations_dir / "0000_notes.adoption.json",
        migrations_dir / "0001_initial.adoption.json",
        migrations_dir / "0001_initial.json",
        migrations_dir / "0002_disabled.adoption.json",
    ]
    for source_path in [notes_path, disabled_path]:
        metadata = json.loads(source_path.with_suffix(".adoption.json").read_text())
        assert metadata == {
            "checksum": _rust_runtime.migration_file_checksum(source_path.read_text()),
            "format": "typebridge.migration-adoption-ignored-source/v1",
            "metadata_digest": metadata["metadata_digest"],
            "name": source_path.stem,
            "source_sha256": hashlib.sha256(source_path.read_bytes()).hexdigest(),
        }
        assert len(metadata["metadata_digest"]) == 64
        assert not source_path.with_suffix(".json").exists()

    from type_bridge.migration.loader import MigrationLoader

    released_history = MigrationLoader(migrations_dir).discover()
    assert [loaded.migration.name for loaded in released_history] == ["0001_initial"]
    assert generate_sidecars(migrations_dir) == []


def test_ignored_source_tampering_never_overwrites_bound_evidence(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    notes_path = _write_ignored(migrations_dir, "0000_notes.py", _IGNORED_NOTES)
    _write_run_typeql(migrations_dir, "0001_initial.py", "ignored-tamper")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute ignored-tamper, value string;\n",
    )
    generate_sidecars(migrations_dir)
    archive_path = notes_path.with_suffix(".adoption.json")
    original_archive = archive_path.read_bytes()

    notes_path.write_text(_IGNORED_NOTES + "# changed after conversion\n")

    with pytest.raises(SidecarConversionError, match="existing artifact has different contents"):
        generate_sidecars(migrations_dir)
    assert archive_path.read_bytes() == original_archive


def test_forged_ignored_source_record_blocks_all_conversion_writes(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    notes_path = _write_ignored(migrations_dir, "0000_notes.py", _IGNORED_NOTES)
    forged_path = notes_path.with_suffix(".adoption.json")
    forged_path.write_text(
        '{"checksum":"0123456789abcdef","format":'
        '"typebridge.migration-adoption-ignored-source/v1",'
        '"metadata_digest":"' + "0" * 64 + '","name":"0000_notes"}\n'
    )
    _write_run_typeql(migrations_dir, "0001_initial.py", "ignored-forgery")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute ignored-forgery, value string;\n",
    )

    with pytest.raises(SidecarConversionError, match="existing artifact has different contents"):
        generate_sidecars(migrations_dir)

    assert not (migrations_dir / "0001_initial.adoption.json").exists()
    assert not (migrations_dir / "0001_initial.json").exists()


def test_orphan_adoption_metadata_blocks_all_conversion_writes(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "orphan-archive")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute orphan-archive, value string;\n",
    )
    orphan = migrations_dir / "9999_deleted.adoption.json"
    orphan.write_text("{}\n")

    with pytest.raises(
        SidecarConversionError,
        match="adoption metadata has no Python source to verify",
    ):
        generate_sidecars(migrations_dir)

    assert orphan.read_text() == "{}\n"
    assert not (migrations_dir / "0001_initial.adoption.json").exists()
    assert not (migrations_dir / "0001_initial.json").exists()
    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()


def test_schema_binding_resolves_a_deep_reverse_ordered_history_iteratively() -> None:
    node_count = 1_200
    names = [f"{index:04d}_node" for index in range(node_count)]
    loaded_history = [
        _loaded_empty(
            name,
            [] if index == node_count - 1 else [("migrations", names[index + 1])],
        )
        for index, name in enumerate(names)
    ]
    resolver = getattr(sidecar_module, "_resolve_schema_bindings")

    bindings = resolver(loaded_history, {names[-1]: "a" * 64})

    assert len(bindings) == node_count
    assert bindings[("migrations", names[0])].authority.migration_name == names[-1]
    assert bindings[("migrations", names[0])].effect == "unchanged_noop"


def test_schema_binding_cycle_is_a_typed_conversion_error() -> None:
    loaded_history = [
        _loaded_empty("0001_left", [("migrations", "0002_right")]),
        _loaded_empty("0002_right", [("migrations", "0001_left")]),
    ]
    resolver = getattr(sidecar_module, "_resolve_schema_bindings")

    with pytest.raises(SidecarConversionError, match="could not be resolved"):
        resolver(loaded_history, {})


def test_run_python_history_gets_archival_adoption_metadata(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "convertible-name")
    schema_hash = _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute convertible-name, value string;\n",
    )
    _write_run_python(
        migrations_dir,
        "0002_backfill.py",
        dependencies=[("migrations", "0001_initial")],
    )

    written = generate_sidecars(migrations_dir)

    archive = migrations_dir / "0002_backfill.adoption.json"
    assert archive in written
    assert not (migrations_dir / "0002_backfill.json").exists()
    metadata = json.loads(archive.read_text())
    assert metadata["format"] == "typebridge.migration-adoption-metadata/v2"
    assert metadata["name"] == "0002_backfill"
    assert metadata["schema_effect"] == "unchanged_run_python"
    assert metadata["schema_source"] == {
        "app_label": "migrations",
        "migration_name": "0001_initial",
    }
    assert metadata["snapshot_schema_hash"] == schema_hash
    assert len(metadata["metadata_digest"]) == 64


def test_released_empty_migration_inherits_snapshot_authority(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "before-empty")
    schema_hash = _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute before-empty, value string;\n",
    )
    empty_path = _write_empty(
        migrations_dir,
        "0002_empty.py",
        dependencies=[("migrations", "0001_initial")],
    )

    written = generate_sidecars(migrations_dir)

    archive_path = migrations_dir / "0002_empty.adoption.json"
    sidecar_path = migrations_dir / "0002_empty.json"
    assert archive_path in written
    assert sidecar_path in written
    archive = json.loads(archive_path.read_text())
    assert archive["schema_effect"] == "unchanged_noop"
    assert archive["schema_source"] == {
        "app_label": "migrations",
        "migration_name": "0001_initial",
    }
    assert archive["snapshot_schema_hash"] == schema_hash
    assert archive["checksum"] == _rust_runtime.migration_file_checksum(empty_path.read_text())
    sidecar = json.loads(sidecar_path.read_text())
    assert sidecar["operations"] == []
    assert sidecar["dependencies"] == [
        {"app_label": "migrations", "migration_name": "0001_initial"}
    ]


def test_empty_merge_inherits_one_converged_parent_authority(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "merge-base")
    schema_hash = _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute merge-base, value string;\n",
    )
    _write_empty(
        migrations_dir,
        "0002_left.py",
        dependencies=[("migrations", "0001_initial")],
    )
    _write_empty(
        migrations_dir,
        "0003_right.py",
        dependencies=[("migrations", "0001_initial")],
    )
    _write_empty(
        migrations_dir,
        "0004_merge.py",
        dependencies=[("migrations", "0002_left"), ("migrations", "0003_right")],
    )

    generate_sidecars(migrations_dir)

    archive = json.loads((migrations_dir / "0004_merge.adoption.json").read_text())
    assert archive["schema_effect"] == "unchanged_noop"
    assert archive["schema_source"] == {
        "app_label": "migrations",
        "migration_name": "0001_initial",
    }
    assert archive["snapshot_schema_hash"] == schema_hash


def test_empty_merge_uses_deterministic_owner_for_equal_distinct_snapshots(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_empty(migrations_dir, "0001_left.py", dependencies=[])
    _write_empty(migrations_dir, "0002_right.py", dependencies=[])
    schema = "define\nattribute shared-merge-schema, value string;\n"
    schema_hash = _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_left",
        schema,
    )
    _write_snapshot(migrations_dir, "v0002", "0002_right", schema)
    _write_empty(
        migrations_dir,
        "0003_merge.py",
        dependencies=[("migrations", "0001_left"), ("migrations", "0002_right")],
    )

    generate_sidecars(migrations_dir)

    archive = json.loads((migrations_dir / "0003_merge.adoption.json").read_text())
    assert archive["schema_source"] == {
        "app_label": "migrations",
        "migration_name": "0001_left",
    }
    assert archive["snapshot_schema_hash"] == schema_hash


def test_root_empty_without_snapshot_fails_closed(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_empty(migrations_dir, "0001_empty.py", dependencies=[])

    with pytest.raises(SidecarConversionError, match="no snapshot-bound dependency"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))
    assert not list(migrations_dir.glob("*.json"))


def test_root_empty_with_explicit_empty_snapshot_is_authoritative(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_empty(migrations_dir, "0001_baseline.py", dependencies=[])
    schema_hash = _write_snapshot(migrations_dir, "v0001", "0001_baseline", "define\n")

    generate_sidecars(migrations_dir)

    archive = json.loads((migrations_dir / "0001_baseline.adoption.json").read_text())
    assert archive["schema_effect"] == "snapshot"
    assert archive["snapshot_schema_hash"] == schema_hash


def test_head_archive_binds_the_immutable_snapshot_schema(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "snapshot-name")
    schema = "define\nattribute snapshot-name, value string;\n"
    schema_hash = _write_snapshot(migrations_dir, "v0001", "0001_initial", schema)

    generate_sidecars(migrations_dir)

    archive = json.loads((migrations_dir / "0001_initial.adoption.json").read_text())
    assert archive["snapshot_schema_hash"] == schema_hash
    assert archive["metadata_digest"] == (
        "268a1a09328bf87482f53818d93d1b613c415694138caccc7f8a5f6df1f0abe4"
    )


def test_mixed_history_converts_only_py_only_members(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "mixed-first")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute mixed-first, value string;\n",
    )
    generate_sidecars(migrations_dir)
    _write_run_typeql(
        migrations_dir,
        "0002_next.py",
        "mixed-second",
        dependencies=[("migrations", "0001_initial")],
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "0002_next",
        "define\nattribute mixed-first, value string;\nattribute mixed-second, value string;\n",
    )

    written = generate_sidecars(migrations_dir)

    assert written == [
        migrations_dir / "0002_next.adoption.json",
        migrations_dir / "0002_next.json",
    ]
    spec = json.loads((migrations_dir / "0002_next.json").read_text())
    assert spec["dependencies"] == [{"app_label": "migrations", "migration_name": "0001_initial"}]


def test_schema_affecting_migration_without_snapshot_fails_before_writes(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "missing-snapshot")

    with pytest.raises(SidecarConversionError, match="no exact immutable snapshot"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))
    assert not list(migrations_dir.glob("*.json"))


def test_run_python_merge_rejects_divergent_parent_snapshots(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_left.py", "left")
    _write_run_typeql(migrations_dir, "0002_right.py", "right")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_left",
        "define\nattribute left, value string;\n",
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "0002_right",
        "define\nattribute right, value string;\n",
    )
    _write_run_python(
        migrations_dir,
        "0003_merge.py",
        dependencies=[("migrations", "0001_left"), ("migrations", "0002_right")],
    )

    with pytest.raises(SidecarConversionError, match="divergent snapshot authority"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))
    assert not list(migrations_dir.glob("*.json"))


def test_empty_merge_rejects_divergent_parent_snapshots(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_left.py", "left-empty-merge")
    _write_run_typeql(migrations_dir, "0002_right.py", "right-empty-merge")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_left",
        "define\nattribute left-empty-merge, value string;\n",
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "0002_right",
        "define\nattribute right-empty-merge, value string;\n",
    )
    _write_empty(
        migrations_dir,
        "0003_merge.py",
        dependencies=[("migrations", "0001_left"), ("migrations", "0002_right")],
    )

    with pytest.raises(SidecarConversionError, match="divergent snapshot authority"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))
    assert not list(migrations_dir.glob("*.json"))


def test_equivalent_duplicate_snapshot_source_names_are_one_authority(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "shared-source")
    schema = "define\nattribute shared-source, value string;\n"
    expected_hash = _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        schema,
    )
    _write_snapshot(migrations_dir, "v0002", "0001_initial", schema)

    generate_sidecars(migrations_dir)

    archive = json.loads((migrations_dir / "0001_initial.adoption.json").read_text())
    assert archive["snapshot_schema_hash"] == expected_hash


def test_snapshot_manifest_preserves_released_python_json_semantics(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "python-json")
    schema = "define\nattribute python-json, value string;\n"
    schema_hash = _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        schema,
    )
    huge_integer = "9" * 400
    manifest = (
        '{"version":"v0001",'
        '"source_migration":"forged-first-value",'
        '"source_migration":"0001_initial",'
        f'"schema_hash":"{schema_hash}",'
        f'"file_hashes":{{"schema.tql":"{"0" * 64}",'
        f'"schema.tql":"{schema_hash}"}},'
        '"type_bridge_version":NaN,'
        '"type_bridge_core_version":Infinity,'
        '"ignored_negative":-Infinity,'
        f'"ignored_huge":{huge_integer},'
        '"ignored_string":"NaN Infinity -Infinity"}'
    )
    (migrations_dir / "snapshots/v0001/snapshot.json").write_text(manifest)

    generate_sidecars(migrations_dir)

    archive = json.loads((migrations_dir / "0001_initial.adoption.json").read_text())
    assert archive["schema_source"]["migration_name"] == "0001_initial"
    assert archive["snapshot_schema_hash"] == schema_hash


def test_non_equivalent_duplicate_snapshot_source_names_are_rejected(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "ambiguous-source")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute first-name, value string;\n",
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "0001_initial",
        "define\nattribute second-name, value string;\n",
    )

    with pytest.raises(SidecarConversionError, match="ambiguous across non-equivalent schemas"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))


def test_snapshot_packages_are_manifest_captured_without_child_enumeration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "relevant-source")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute relevant-source, value string;\n",
    )
    for index in range(2, 130):
        snapshot = migrations_dir / "snapshots" / f"v{index:04}"
        _write_snapshot(
            migrations_dir,
            f"v{index:04}",
            "9999_irrelevant",
            "define\nattribute irrelevant-source, value string;\n",
        )
        for extra in range(8):
            (snapshot / f"extra-{extra}.txt").write_text("not recognized")

    original_entries = sidecar_module.AdoptionDirectoryAuthority.entries
    original_read = sidecar_module.AdoptionDirectoryAuthority.read_bounded
    schema_reads: list[Path] = []

    def guarded_entries(
        authority: AdoptionDirectoryAuthority,
        relative: Path = Path("."),
        *,
        maximum_entries: int = 65_536,
        expected_directory: AdoptionDirectoryEntry | None = None,
        reject_non_utf8: bool = False,
    ) -> tuple[AdoptionDirectoryEntry, ...]:
        assert len(relative.parts) <= 1 or relative == Path("snapshots/v0001")
        return original_entries(
            authority,
            relative,
            maximum_entries=maximum_entries,
            expected_directory=expected_directory,
            reject_non_utf8=reject_non_utf8,
        )

    def observed_read(
        authority: AdoptionDirectoryAuthority,
        relative: Path,
        limit: int,
        *,
        expected: AdoptionDirectoryEntry | None = None,
    ) -> bytes:
        if relative.name == "schema.tql":
            schema_reads.append(relative)
        return original_read(authority, relative, limit, expected=expected)

    monkeypatch.setattr(sidecar_module.AdoptionDirectoryAuthority, "entries", guarded_entries)
    monkeypatch.setattr(sidecar_module.AdoptionDirectoryAuthority, "read_bounded", observed_read)

    generate_sidecars(migrations_dir)

    assert set(schema_reads) == {
        Path(f"snapshots/v{index:04}/schema.tql") for index in range(1, 130)
    }


def test_converter_publication_stays_on_retained_root_after_ambient_swap(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    migrations_dir = tmp_path / "migrations"
    held = tmp_path / "held"
    _write_run_typeql(migrations_dir, "0001_initial.py", "held-source")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute held-source, value string;\n",
    )
    require = sidecar_module._require_absent_or_identical
    swapped = False

    def swap_before_collision_validation(
        authority: AdoptionDirectoryAuthority,
        path: Path,
        contents: str,
    ) -> bool:
        nonlocal swapped
        if not swapped:
            swapped = True
            migrations_dir.rename(held)
            migrations_dir.mkdir()
        return require(authority, path, contents)

    monkeypatch.setattr(
        sidecar_module,
        "_require_absent_or_identical",
        swap_before_collision_validation,
    )

    generate_sidecars(migrations_dir)

    assert (held / "0001_initial.adoption.json").is_file()
    assert (held / "0001_initial.json").is_file()
    assert not (migrations_dir / "0001_initial.adoption.json").exists()
    assert not (migrations_dir / "0001_initial.json").exists()


def test_converter_accepts_root_symlink_and_replacement_cannot_redirect_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    original = tmp_path / "original"
    replacement = tmp_path / "replacement"
    configured = tmp_path / "configured-migrations"
    _write_run_typeql(original, "0001_initial.py", "symlink-source")
    _write_snapshot(
        original,
        "v0001",
        "0001_initial",
        "define\nattribute symlink-source, value string;\n",
    )
    replacement.mkdir()
    try:
        configured.symlink_to(original, target_is_directory=True)
    except OSError as error:
        pytest.skip(f"directory symlink creation unavailable: {error}")
    require = sidecar_module._require_absent_or_identical
    swapped = False

    def replace_root_symlink(
        authority: AdoptionDirectoryAuthority,
        path: Path,
        contents: str,
    ) -> bool:
        nonlocal swapped
        if not swapped:
            swapped = True
            configured.unlink()
            configured.symlink_to(replacement, target_is_directory=True)
        return require(authority, path, contents)

    monkeypatch.setattr(
        sidecar_module,
        "_require_absent_or_identical",
        replace_root_symlink,
    )

    generate_sidecars(configured)

    assert (original / "0001_initial.adoption.json").is_file()
    assert (original / "0001_initial.json").is_file()
    assert not (replacement / "0001_initial.adoption.json").exists()
    assert not (replacement / "0001_initial.json").exists()


@pytest.mark.parametrize("checksum_form", ["present", "null", "omitted"])
def test_released_execution_sidecar_is_preserved_semantically_without_raw_digest(
    tmp_path: Path,
    checksum_form: str,
) -> None:
    migrations_dir = tmp_path / "migrations"
    py_path = _write_run_typeql(migrations_dir, "0001_initial.py", "legacy-sidecar")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute legacy-sidecar, value string;\n",
    )
    [loaded] = MigrationLoader(migrations_dir, use_sidecars=False).discover()
    spec = lower_migration(loaded.migration, checksum=loaded.checksum)
    # Released Python discovery ignores this additive field and applies only
    # the optional legacy checksum rule. Conversion must preserve that fact.
    spec["source_sha256"] = "0" * 64
    if checksum_form == "null":
        spec["checksum"] = None
    elif checksum_form == "omitted":
        spec.pop("checksum")
    sidecar_path = py_path.with_suffix(".json")
    sidecar_path.write_text(json.dumps(spec, indent=2) + "\n")
    before = sidecar_path.read_bytes()

    written = generate_sidecars(migrations_dir)

    assert written == [py_path.with_suffix(".adoption.json")]
    assert sidecar_path.read_bytes() == before
    archive = json.loads(py_path.with_suffix(".adoption.json").read_text())
    assert archive["format"] == "typebridge.migration-adoption-sidecar/v1"
    assert archive["source_sha256"] == hashlib.sha256(py_path.read_bytes()).hexdigest()
    assert archive["sidecar_sha256"] == hashlib.sha256(before).hexdigest()
    assert MigrationLoader(migrations_dir).discover()[0].migration.name == "0001_initial"


def test_nonimportable_python_with_released_execution_sidecar_is_adopted(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    source = _write_ignored(
        migrations_dir,
        "0001_notes.py",
        "raise RuntimeError('released sidecar must prevent this execution')\n",
    )
    checksum = _rust_runtime.migration_file_checksum(source.read_text())
    sidecar = {
        "app_label": "migrations",
        "name": "0001_notes",
        "dependencies": [],
        "operations": [],
        "checksum": checksum,
        "reversible": True,
    }
    source.with_suffix(".json").write_text(_rust_runtime.migration_spec_to_json(sidecar))
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_notes",
        "define\nattribute sidecar-only, value string;\n",
    )

    assert [item.migration.name for item in MigrationLoader(migrations_dir).discover()] == [
        "0001_notes"
    ]
    written = generate_sidecars(migrations_dir)

    assert written == [source.with_suffix(".adoption.json")]
    archive = json.loads(source.with_suffix(".adoption.json").read_text())
    assert archive["format"] == "typebridge.migration-adoption-sidecar/v1"
    assert archive["source_name"] == source.stem
    assert archive["app_label"] == "migrations"
    assert archive["name"] == "0001_notes"
    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()


def test_sidecar_effective_identity_and_dependencies_override_source_paths(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    parent_source = _write_ignored(
        migrations_dir,
        "0001_raw.py",
        "raise RuntimeError('sidecar owns parent')\n",
    )
    child_source = _write_ignored(
        migrations_dir,
        "0002_raw.py",
        "raise RuntimeError('sidecar owns child')\n",
    )
    parent_checksum = _rust_runtime.migration_file_checksum(parent_source.read_text())
    child_checksum = _rust_runtime.migration_file_checksum(child_source.read_text())
    parent_source.with_suffix(".json").write_text(
        _rust_runtime.migration_spec_to_json(
            {
                "app_label": "legacy_app",
                "name": "0001_effective",
                "dependencies": [],
                "operations": [],
                "checksum": parent_checksum,
                "reversible": True,
            }
        )
    )
    child_source.with_suffix(".json").write_text(
        _rust_runtime.migration_spec_to_json(
            {
                "app_label": "legacy_app",
                "name": "0002_effective",
                "dependencies": [
                    {
                        "app_label": "legacy_app",
                        "migration_name": "0001_effective",
                    }
                ],
                "operations": [],
                "checksum": child_checksum,
                "reversible": True,
            }
        )
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_effective",
        "define\nattribute effective-sidecar, value string;\n",
    )

    written = generate_sidecars(migrations_dir)

    assert written == [
        parent_source.with_suffix(".adoption.json"),
        child_source.with_suffix(".adoption.json"),
    ]
    parent = json.loads(parent_source.with_suffix(".adoption.json").read_text())
    child = json.loads(child_source.with_suffix(".adoption.json").read_text())
    assert (parent["source_name"], parent["app_label"], parent["name"]) == (
        "0001_raw",
        "legacy_app",
        "0001_effective",
    )
    assert child["dependencies"] == [
        {"app_label": "legacy_app", "migration_name": "0001_effective"}
    ]
    assert child["schema_effect"] == "unchanged_noop"
    assert child["schema_source"]["migration_name"] == "0001_effective"


def test_cross_app_claims_on_one_snapshot_source_are_rejected(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    for source_name, app_label in [("0001_a", "app_a"), ("0001_b", "app_b")]:
        source = migrations_dir / f"{source_name}.py"
        source.write_text("raise RuntimeError('released sidecar wins')\n")
        checksum = _rust_runtime.migration_file_checksum(source.read_text())
        source.with_suffix(".json").write_text(
            _rust_runtime.migration_spec_to_json(
                {
                    "app_label": app_label,
                    "name": "0001_shared",
                    "dependencies": [],
                    "operations": [],
                    "checksum": checksum,
                    "reversible": True,
                }
            )
        )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_shared",
        "define\nattribute shared-source, value string;\n",
    )

    with pytest.raises(SidecarConversionError, match="ambiguous across app labels"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))
    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()


def test_empty_sidecar_identity_uses_released_directory_and_source_fallbacks(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    source = _write_ignored(
        migrations_dir,
        "0001_fallback.py",
        "raise RuntimeError('sidecar prevents import')\n",
    )
    checksum = _rust_runtime.migration_file_checksum(source.read_text())
    source.with_suffix(".json").write_text(
        _rust_runtime.migration_spec_to_json(
            {
                "app_label": "",
                "name": "",
                "dependencies": [],
                "operations": [],
                "checksum": checksum,
                "reversible": True,
            }
        )
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_fallback",
        "define\nattribute fallback-identity, value string;\n",
    )

    generate_sidecars(migrations_dir)

    archive = json.loads(source.with_suffix(".adoption.json").read_text())
    assert archive["source_name"] == "0001_fallback"
    assert archive["app_label"] == "migrations"
    assert archive["name"] == "0001_fallback"


def test_dynamic_copy_attribute_inherits_converged_parent_snapshot(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "copy-parent")
    child = _write_copy_attribute(
        migrations_dir,
        "0002_backfill.py",
        dependencies=[("migrations", "0001_initial")],
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute copy-parent, value string;\n",
    )

    generate_sidecars(migrations_dir)

    archive = json.loads(child.with_suffix(".adoption.json").read_text())
    assert archive["format"] == "typebridge.migration-adoption-sidecar/v1"
    assert archive["schema_effect"] == "unchanged_copy_attribute"
    assert archive["schema_source"]["migration_name"] == "0001_initial"


def test_preexisting_copy_attribute_sidecar_needs_no_exact_snapshot(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "copy-sidecar-parent")
    child = _write_ignored(
        migrations_dir,
        "0002_backfill.py",
        "raise RuntimeError('copy sidecar prevents import')\n",
    )
    checksum = _rust_runtime.migration_file_checksum(child.read_text())
    child.with_suffix(".json").write_text(
        _rust_runtime.migration_spec_to_json(
            {
                "app_label": "migrations",
                "name": "0002_backfill",
                "dependencies": [
                    {
                        "app_label": "migrations",
                        "migration_name": "0001_initial",
                    }
                ],
                "operations": [
                    {
                        "kind": "copy_attribute",
                        "forward": (
                            "match $x isa person, has old-name $v; insert $x has new-name == $v;"
                        ),
                        "reverse": "match $x has new-name $v; delete $v of $x;",
                    }
                ],
                "checksum": checksum,
                "reversible": True,
            }
        )
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute copy-sidecar-parent, value string;\n",
    )

    generate_sidecars(migrations_dir)

    archive = json.loads(child.with_suffix(".adoption.json").read_text())
    assert archive["schema_effect"] == "unchanged_copy_attribute"


def test_mixed_copy_and_schema_sidecar_without_snapshot_is_rejected(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "mixed-copy-parent")
    child = _write_ignored(migrations_dir, "0002_mixed.py", "# sidecar authority\n")
    checksum = _rust_runtime.migration_file_checksum(child.read_text())
    child.with_suffix(".json").write_text(
        _rust_runtime.migration_spec_to_json(
            {
                "app_label": "migrations",
                "name": "0002_mixed",
                "dependencies": [
                    {
                        "app_label": "migrations",
                        "migration_name": "0001_initial",
                    }
                ],
                "operations": [
                    {
                        "kind": "copy_attribute",
                        "forward": "match $x isa person; insert $x has new-name 'x';",
                        "reverse": None,
                    },
                    {
                        "kind": "run_typeql",
                        "forward": "define attribute ambiguous-schema, value string;",
                        "reverse": None,
                    },
                ],
                "checksum": checksum,
                "reversible": False,
            }
        )
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute mixed-copy-parent, value string;\n",
    )

    with pytest.raises(SidecarConversionError, match="not exclusively schema-neutral"):
        generate_sidecars(migrations_dir)

    assert not child.with_suffix(".adoption.json").exists()


def test_existing_sidecar_tamper_never_rewrites_bound_archive(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    source = _write_run_typeql(migrations_dir, "0001_initial.py", "sidecar-tamper")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute sidecar-tamper, value string;\n",
    )
    generate_sidecars(migrations_dir)
    archive_path = source.with_suffix(".adoption.json")
    archive_before = archive_path.read_bytes()
    sidecar_path = source.with_suffix(".json")
    sidecar = json.loads(sidecar_path.read_text())
    sidecar["reversible"] = not sidecar["reversible"]
    sidecar_path.write_text(json.dumps(sidecar))

    with pytest.raises(SidecarConversionError, match="existing artifact has different contents"):
        generate_sidecars(migrations_dir)

    assert archive_path.read_bytes() == archive_before


def test_mirror_imports_manifest_bound_snapshot_not_sourced_by_loaded_history(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    migration_source = (
        "from migrations.snapshots.v0009 import MARKER\n"
        "assert MARKER == 'retained'\n"
        + _RUN_TYPEQL_MIGRATION.format(attr="orphan-import", dependencies=[])
    )
    (migrations_dir / "0001_initial.py").write_text(migration_source)
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute orphan-import, value string;\n",
    )
    snapshots = migrations_dir / "snapshots"
    (snapshots / "__init__.py").write_text("")
    orphan = snapshots / "v0009"
    orphan.mkdir()
    init_bytes = b"MARKER = 'retained'\n"
    schema_bytes = b"define\nattribute unrelated, value string;\n"
    (orphan / "__init__.py").write_bytes(init_bytes)
    (orphan / "schema.tql").write_bytes(schema_bytes)
    (orphan / "snapshot.json").write_text(
        json.dumps(
            {
                "version": "v0009",
                "source_migration": "9999_not_loaded",
                "schema_hash": hashlib.sha256(schema_bytes).hexdigest(),
                "file_hashes": {
                    "__init__.py": hashlib.sha256(init_bytes).hexdigest(),
                    "schema.tql": hashlib.sha256(schema_bytes).hexdigest(),
                },
                "type_bridge_version": "1.5.11",
                "type_bridge_core_version": "1.5.11",
            }
        )
    )

    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()


def test_retained_mirror_preserves_regular_package_resources(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    (migrations_dir / "data.json").write_text('{"marker":"retained-resource"}\n')
    source = (
        "import json\n"
        "from pathlib import Path\n"
        "payload = json.loads(Path(__file__).with_name('data.json').read_text())\n"
        "assert payload['marker'] == 'retained-resource'\n"
        + _RUN_TYPEQL_MIGRATION.format(attr="resource-backed", dependencies=[])
    )
    (migrations_dir / "0001_initial.py").write_text(source)
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute resource-backed, value string;\n",
    )

    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()


def test_retained_mirror_materializes_empty_package_directories(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    (migrations_dir / "empty_resource").mkdir()
    source = (
        "from pathlib import Path\n"
        "resource = Path(__file__).with_name('empty_resource')\n"
        "assert resource.is_dir() and not list(resource.iterdir())\n"
        + _RUN_TYPEQL_MIGRATION.format(attr="empty-resource", dependencies=[])
    )
    (migrations_dir / "0001_initial.py").write_text(source)
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute empty-resource, value string;\n",
    )

    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()


def test_retained_conversion_preserves_released_parent_helper_imports(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    helper_name = "legacy_parent_helper_170"
    (tmp_path / f"{helper_name}.py").write_text("MARKER = 'released-parent'\n")
    source = (
        f"from {helper_name} import MARKER\n"
        "assert MARKER == 'released-parent'\n"
        + _RUN_TYPEQL_MIGRATION.format(attr="parent-helper", dependencies=[])
    )
    (migrations_dir / "0001_initial.py").write_text(source)
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute parent-helper, value string;\n",
    )
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    monkeypatch.chdir(elsewhere)
    sys.modules.pop(helper_name, None)

    try:
        generate_sidecars(migrations_dir)
    finally:
        sys.modules.pop(helper_name, None)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()


def test_hostile_meta_path_finder_cannot_intercept_retained_package_imports(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    (migrations_dir / "helper.py").write_text("VALUE = 'retained'\n")
    source = (
        "from migrations.helper import VALUE\n"
        "assert VALUE == 'retained'\n"
        + _RUN_TYPEQL_MIGRATION.format(attr="finder-safe", dependencies=[])
    )
    (migrations_dir / "0001_initial.py").write_text(source)
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute finder-safe, value string;\n",
    )

    class HostileFinder(importlib.abc.MetaPathFinder):
        scoped_calls = 0

        def find_spec(
            self,
            fullname: str,
            path: object = None,
            target: object = None,
        ) -> None:
            del path, target
            if fullname == "migrations" or fullname.startswith("migrations."):
                self.scoped_calls += 1
            return None

    hostile = HostileFinder()
    sys.meta_path.insert(0, hostile)
    try:
        generate_sidecars(migrations_dir)
    finally:
        sys.meta_path.remove(hostile)

    assert hostile.scoped_calls == 0
    assert not list(migrations_dir.rglob("__pycache__"))


def test_unrelated_symlinked_helper_does_not_break_conversion(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "unrelated-helper")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute unrelated-helper, value string;\n",
    )
    target = tmp_path / "ambient-helper.py"
    target.write_text("raise AssertionError('must not import')\n")
    try:
        (migrations_dir / "helper.py").symlink_to(target)
    except OSError as error:
        pytest.skip(f"symlink creation unavailable: {error}")

    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()


@pytest.mark.skipif(os.name == "nt", reason="these names are not valid Windows entries")
def test_unix_released_names_remain_publishable_on_the_current_filesystem(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_con.py", "unix-con")
    _write_run_typeql(
        migrations_dir,
        "0002_a:b.py",
        "unix-colon",
        [("migrations", "0001_con")],
    )
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_con",
        "define\nattribute unix-con, value string;\n",
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "0002_a:b",
        "define\nattribute unix-con, value string;\nattribute unix-colon, value string;\n",
    )

    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_con.adoption.json").is_file()
    assert (migrations_dir / "0002_a:b.adoption.json").is_file()


@pytest.mark.skipif(os.name == "nt", reason="non-UTF-8 filenames are Unix-only")
def test_non_utf8_unrelated_name_is_ignored_but_migration_name_is_typed_rejection(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "non-utf-boundary")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute non-utf-boundary, value string;\n",
    )
    root = os.fsencode(migrations_dir)
    unrelated = root + b"/notes-\xff.txt"
    relevant = root + b"/0002_\xff.py"
    with open(unrelated, "wb") as file:
        file.write(b"ignored")

    generate_sidecars(migrations_dir)

    with open(relevant, "wb") as file:
        file.write(_IGNORED_NOTES.encode())
    with pytest.raises(SidecarConversionError, match="not valid UTF-8"):
        generate_sidecars(migrations_dir)


def test_selected_snapshot_ignores_unbound_pycache_like_released_validation(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "snapshot-extra")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute snapshot-extra, value string;\n",
    )
    pycache = migrations_dir / "snapshots/v0001/__pycache__"
    pycache.mkdir()
    (pycache / "entities.cpython-313.pyc").write_bytes(b"ambient cache")

    generate_sidecars(migrations_dir)

    assert (migrations_dir / "0001_initial.adoption.json").is_file()


def test_selected_snapshot_rehashes_every_manifest_bound_file(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "snapshot-bound-drift")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute snapshot-bound-drift, value string;\n",
    )
    snapshot = migrations_dir / "snapshots/v0001"
    generated = snapshot / "entities.py"
    original = b"class Person: pass\n"
    generated.write_bytes(original)
    manifest_path = snapshot / "snapshot.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    manifest["file_hashes"]["entities.py"] = hashlib.sha256(original).hexdigest()
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    generated.write_bytes(b"class Replaced: pass\n")

    with pytest.raises(SidecarConversionError, match="manifest file entities.py changed"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))


def test_unicode_digit_snapshot_version_cannot_authorize_conversion(tmp_path: Path) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "unicode-version")
    _write_snapshot(
        migrations_dir,
        "v٠٠٠١",
        "0001_initial",
        "define\nattribute unicode-version, value string;\n",
    )

    with pytest.raises(SidecarConversionError, match="no exact immutable snapshot"):
        generate_sidecars(migrations_dir)

    assert not list(migrations_dir.glob("*.adoption.json"))
    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()


def test_snapshot_manifest_inspections_share_the_global_entry_ceiling(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "entry-budget")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute entry-budget, value string;\n",
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "9999_orphan",
        "define\nattribute orphan, value string;\n",
    )
    monkeypatch.setattr(adoption_import_module, "_MAX_DIRECTORY_ENTRIES", 7)

    with pytest.raises(SidecarConversionError, match="entry ceiling"):
        generate_sidecars(migrations_dir)

    monkeypatch.setattr(adoption_import_module, "_MAX_DIRECTORY_ENTRIES", 8)
    generate_sidecars(migrations_dir)


def test_interrupted_publication_resumes_only_the_same_journaled_plan(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "resume-plan")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute resume-plan, value string;\n",
    )
    write = sidecar_module._write_atomic_no_replace
    calls = 0

    def interrupt_third_write(
        authority: AdoptionDirectoryAuthority,
        path: Path,
        contents: str,
    ) -> None:
        nonlocal calls
        calls += 1
        if calls == 3:
            raise SidecarConversionError({"test": "interrupted"})
        write(authority, path, contents)

    monkeypatch.setattr(sidecar_module, "_write_atomic_no_replace", interrupt_third_write)
    with pytest.raises(SidecarConversionError, match="interrupted"):
        generate_sidecars(migrations_dir)

    journal = migrations_dir / sidecar_module._CONVERSION_JOURNAL
    assert journal.is_file()
    assert (migrations_dir / "0001_initial.adoption.json").is_file()
    monkeypatch.setattr(sidecar_module, "_write_atomic_no_replace", write)

    generate_sidecars(migrations_dir)

    assert not journal.exists()
    assert (migrations_dir / "0001_initial.json").is_file()


@pytest.mark.parametrize(
    "temporary_kind",
    ["pub", "rm"],
)
def test_retry_accepts_only_plan_bound_native_crash_litter(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    temporary_kind: str,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "resume-with-crash-litter")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute resume-with-crash-litter, value string;\n",
    )
    write = sidecar_module._write_atomic_no_replace
    calls = 0

    def interrupt_third_write(
        authority: AdoptionDirectoryAuthority,
        path: Path,
        contents: str,
    ) -> None:
        nonlocal calls
        calls += 1
        if calls == 3:
            raise SidecarConversionError({"test": "interrupted"})
        write(authority, path, contents)

    monkeypatch.setattr(sidecar_module, "_write_atomic_no_replace", interrupt_third_write)
    with pytest.raises(SidecarConversionError, match="interrupted"):
        generate_sidecars(migrations_dir)

    journal = migrations_dir / sidecar_module._CONVERSION_JOURNAL
    journal_bytes = journal.read_bytes()
    target_sha256 = hashlib.sha256(sidecar_module._CONVERSION_JOURNAL.encode()).hexdigest()
    contents_sha256 = hashlib.sha256(journal_bytes).hexdigest()
    temporary_name = f".tb-adopt-{temporary_kind}-{target_sha256}-{contents_sha256}-0.tmp"
    litter = migrations_dir / temporary_name
    litter.write_bytes(journal_bytes)
    monkeypatch.setattr(sidecar_module, "_write_atomic_no_replace", write)

    generate_sidecars(migrations_dir)

    assert not litter.exists()
    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()
    assert (migrations_dir / "0001_initial.json").is_file()

    _write_run_typeql(
        migrations_dir,
        "0002_later.py",
        "after-recovered-litter",
        dependencies=[("migrations", "0001_initial")],
    )
    _write_snapshot(
        migrations_dir,
        "v0002",
        "0002_later",
        (
            "define\n"
            "attribute resume-with-crash-litter, value string;\n"
            "attribute after-recovered-litter, value string;\n"
        ),
    )
    generate_sidecars(migrations_dir)
    assert (migrations_dir / "0002_later.adoption.json").is_file()


def test_proof_shaped_temporary_not_owned_by_current_plan_fails_closed(
    tmp_path: Path,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "unowned-temp")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute unowned-temp, value string;\n",
    )
    body = b"ordinary package resource"
    target_sha256 = hashlib.sha256(b"not-a-planned-output").hexdigest()
    contents_sha256 = hashlib.sha256(body).hexdigest()
    temporary = migrations_dir / (f".tb-adopt-pub-{target_sha256}-{contents_sha256}-0.tmp")
    temporary.write_bytes(body)

    with pytest.raises(SidecarConversionError, match="not owned by the current conversion plan"):
        generate_sidecars(migrations_dir)

    assert temporary.read_bytes() == body
    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()


def test_retry_after_sidecar_publication_keeps_the_same_journal_plan(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "resume-after-sidecar")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute resume-after-sidecar, value string;\n",
    )
    validate_membership = sidecar_module._validate_recognized_root_membership

    def interrupt_after_outputs(
        authority: AdoptionDirectoryAuthority,
        expected_names: set[str],
    ) -> None:
        validate_membership(authority, expected_names)
        raise SidecarConversionError({"test": "after-sidecar"})

    monkeypatch.setattr(
        sidecar_module,
        "_validate_recognized_root_membership",
        interrupt_after_outputs,
    )
    with pytest.raises(SidecarConversionError, match="after-sidecar"):
        generate_sidecars(migrations_dir)

    journal = migrations_dir / sidecar_module._CONVERSION_JOURNAL
    assert journal.is_file()
    assert (migrations_dir / "0001_initial.adoption.json").is_file()
    assert (migrations_dir / "0001_initial.json").is_file()
    monkeypatch.setattr(
        sidecar_module,
        "_validate_recognized_root_membership",
        validate_membership,
    )

    assert generate_sidecars(migrations_dir) == []
    assert not journal.exists()


def test_output_body_race_is_rejected_before_journal_removal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "output-race")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute output-race, value string;\n",
    )
    validate_membership = sidecar_module._validate_recognized_root_membership

    def mutate_archive(
        authority: AdoptionDirectoryAuthority,
        expected_names: set[str],
    ) -> None:
        archive = migrations_dir / "0001_initial.adoption.json"
        body = archive.read_bytes()
        archive.write_bytes(b"[" + body[1:])
        validate_membership(authority, expected_names)

    monkeypatch.setattr(
        sidecar_module,
        "_validate_recognized_root_membership",
        mutate_archive,
    )

    with pytest.raises(SidecarConversionError):
        generate_sidecars(migrations_dir)

    assert (migrations_dir / sidecar_module._CONVERSION_JOURNAL).is_file()


def test_new_migration_member_race_is_rejected_before_journal_removal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "membership-race")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute membership-race, value string;\n",
    )
    validate_membership = sidecar_module._validate_recognized_root_membership

    def add_source(
        authority: AdoptionDirectoryAuthority,
        expected_names: set[str],
    ) -> None:
        _write_ignored(migrations_dir, "0002_added.py", _IGNORED_NOTES)
        validate_membership(authority, expected_names)

    monkeypatch.setattr(
        sidecar_module,
        "_validate_recognized_root_membership",
        add_source,
    )

    with pytest.raises(SidecarConversionError, match="membership changed"):
        generate_sidecars(migrations_dir)

    assert (migrations_dir / sidecar_module._CONVERSION_JOURNAL).is_file()


@pytest.mark.parametrize("mutation", ["add", "remove"])
def test_nested_resource_membership_race_is_rejected_before_journal_removal(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    mutation: str,
) -> None:
    migrations_dir = tmp_path / "migrations"
    _write_run_typeql(migrations_dir, "0001_initial.py", "nested-membership-race")
    resources = migrations_dir / "resources"
    resources.mkdir()
    stable = resources / "stable.txt"
    stable.write_text("stable\n")
    _write_snapshot(
        migrations_dir,
        "v0001",
        "0001_initial",
        "define\nattribute nested-membership-race, value string;\n",
    )
    validate_membership = sidecar_module._validate_recognized_root_membership

    def mutate_nested_directory(
        authority: AdoptionDirectoryAuthority,
        expected_names: set[str],
    ) -> None:
        validate_membership(authority, expected_names)
        if mutation == "add":
            (resources / "late.txt").write_text("late\n")
        else:
            stable.unlink()

    monkeypatch.setattr(
        sidecar_module,
        "_validate_recognized_root_membership",
        mutate_nested_directory,
    )

    with pytest.raises(SidecarConversionError, match="directory membership changed"):
        generate_sidecars(migrations_dir)

    assert (migrations_dir / sidecar_module._CONVERSION_JOURNAL).is_file()


def test_aggregate_budget_rejects_before_journal_publication(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    migrations_dir = tmp_path / "migrations"
    source = _write_run_typeql(migrations_dir, "0001_initial.py", "aggregate-budget")
    schema = "define\nattribute aggregate-budget, value string;\n"
    _write_snapshot(migrations_dir, "v0001", "0001_initial", schema)
    manifest = migrations_dir / "snapshots/v0001/snapshot.json"
    captured_size = len(source.read_bytes()) + len(schema.encode()) + len(manifest.read_bytes())
    monkeypatch.setattr(sidecar_module, "_MAX_HISTORY_BYTES", captured_size + 1)

    with pytest.raises(SidecarConversionError, match="aggregate ceiling"):
        generate_sidecars(migrations_dir)

    assert not (migrations_dir / sidecar_module._CONVERSION_JOURNAL).exists()
    assert not list(migrations_dir.glob("*.adoption.json"))
