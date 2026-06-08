# pyright: reportMissingImports=false
"""Live-DB integration tests for CopyAttribute backfill counts.

These tests exercise the full CopyAttribute workflow against a real TypeDB
instance:
  - Schema setup via MigrationGenerator + MigrationExecutor.
  - Data seeding via the ORM manager API.
  - A second migration carrying ops.CopyAttribute.
  - Assertion of MigrationResult.backfill counts (matched/inserted/skipped).
  - Post-backfill data verification via manager query.
  - Rollback (dest-delete) and verification that dest values are removed.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from type_bridge import Entity, Flag, Key, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import (
    MigrationExecutor,
    MigrationGenerator,
)
from type_bridge.migration import (
    operations as ops,
)

# ---------------------------------------------------------------------------
# Local model definitions
# These use distinctive TypeDB type names (backfill_person, src_field,
# dst_field) to avoid collisions with other integration tests that register
# the common "person" / "name" types.  Each test gets a clean_db so there
# is no cross-test state, but distinct type names keep the metaclass
# descriptor registry from conflicting.
# ---------------------------------------------------------------------------


class SrcField(String):
    """Source attribute for the backfill test."""

    flags = AttributeFlags(name="src-field")


class DstField(String):
    """Destination attribute for the backfill test."""

    flags = AttributeFlags(name="dst-field")


class BackfillPerson(Entity):
    """Owner entity for the backfill test."""

    flags = TypeFlags(name="backfill-person")
    src_field: SrcField = Flag(Key)
    dst_field: DstField | None = None


# ---------------------------------------------------------------------------
# Helper — hand-author a CopyAttribute migration .py + .json sidecar pair
# ---------------------------------------------------------------------------


def _write_backfill_migration(
    migrations_dir: Path,
    app_label: str,
    initial_migration_name: str,
) -> Path:
    """Write a CopyAttribute migration file + sidecar into *migrations_dir*.

    The migration depends on the initial schema migration so the executor
    applies them in order.  Returns the path of the written .py file.
    """
    from type_bridge import _rust_runtime

    name = "0002_backfill_dst"

    # Build the operation and derive its TypeQL via the frozen op API.
    op = ops.CopyAttribute(owner=BackfillPerson, source="src-field", dest="dst-field")

    # The .py is the checksum source only; execution is driven by the sidecar
    # (execution_spec).  The Migration subclass needs only correct dependencies
    # so the loader builds the graph correctly.  Keeping the .py free of
    # BackfillPerson import avoids a sys.path dependency at load time.
    py_text = f"""\
from typing import ClassVar
from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation


class BackfillDstMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({app_label!r}, {initial_migration_name!r}),
    ]
    operations: ClassVar[list[Operation]] = []
    reversible: ClassVar[bool] = True
"""

    checksum = _rust_runtime.migration_file_checksum(py_text)

    spec: dict = {
        "app_label": app_label,
        "name": name,
        "dependencies": [{"app_label": app_label, "migration_name": initial_migration_name}],
        "operations": [
            {
                "kind": "copy_attribute",
                "forward": op.to_typeql(),
                "reverse": op.to_rollback_typeql(),
            }
        ],
        "checksum": checksum,
        "reversible": True,
    }

    normalized = _rust_runtime.normalize_migration_spec(spec)
    json_text = _rust_runtime.migration_spec_to_json(normalized)

    py_path = migrations_dir / f"{name}.py"
    json_path = migrations_dir / f"{name}.json"
    py_path.write_text(py_text)
    json_path.write_text(json_text)

    return py_path


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


@pytest.mark.integration
@pytest.mark.order(400)
def test_copy_attribute_backfill_counts_and_effect(clean_db, tmp_path: Path):
    """CopyAttribute migration reports correct counts and backfills data in TypeDB.

    Seed layout:
      - 5 BackfillPerson instances, all with src-field set.
      - 2 of those also have dst-field pre-set (they will be SKIPPED).
      - 3 have no dst-field (they will be INSERTED).

    Expected MigrationResult.backfill[0]:
      matched  = 5   (all persons have src-field)
      inserted = 3   (persons without dst-field get it backfilled)
      skipped  = 2   (persons that already had dst-field)
    """
    migrations_dir = tmp_path / "migrations"

    # ── Step 1: generate and apply the initial schema migration ──────────────
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    initial_path = generator.generate(
        models=[BackfillPerson],
        name="initial",
    )
    assert initial_path is not None, "initial migration must be generated"
    initial_name = initial_path.stem  # e.g. "0001_initial"

    schema_results = executor.migrate()
    assert len(schema_results) == 1
    assert schema_results[0].success, f"schema migration failed: {schema_results[0].error}"

    # ── Step 2: seed data via the ORM manager API ────────────────────────────
    # 5 persons with src-field; 2 of them already have dst-field (skipped).
    # 3 do not have dst-field (will be inserted).
    persons_to_insert = [
        # --- will be INSERTED (no dst-field) ---
        BackfillPerson(src_field=SrcField("alice"), dst_field=None),
        BackfillPerson(src_field=SrcField("bob"), dst_field=None),
        BackfillPerson(src_field=SrcField("carol"), dst_field=None),
        # --- will be SKIPPED (dst-field already set) ---
        BackfillPerson(src_field=SrcField("dave"), dst_field=DstField("dave-orig")),
        BackfillPerson(src_field=SrcField("eve"), dst_field=DstField("eve-orig")),
    ]
    BackfillPerson.manager(clean_db).insert_many(persons_to_insert)

    # Verify seed: confirm all 5 are in the DB.
    all_before = BackfillPerson.manager(clean_db).all()
    assert len(all_before) == 5, f"expected 5 seeded persons, got {len(all_before)}"

    # ── Step 3: write the CopyAttribute migration + sidecar and apply ────────
    app_label = migrations_dir.name
    _write_backfill_migration(migrations_dir, app_label, initial_name)

    backfill_results = executor.migrate()
    assert len(backfill_results) == 1, (
        f"expected exactly 1 migration result, got {len(backfill_results)}"
    )
    bf_result = backfill_results[0]
    assert bf_result.success, f"backfill migration failed: {bf_result.error}"
    assert bf_result.name == "0002_backfill_dst"

    # ── Step 4: assert backfill counts ───────────────────────────────────────
    assert bf_result.backfill is not None, (
        "MigrationResult.backfill must be populated for a CopyAttribute migration; "
        "got None.  Check that executor._record_result extracts 'backfill' from rust_result."
    )
    assert len(bf_result.backfill) == 1, (
        f"expected 1 backfill step result, got {len(bf_result.backfill)}"
    )
    step_counts = bf_result.backfill[0]

    # 5 persons all have src-field → matched = 5
    assert step_counts["matched"] == 5, f"matched: expected 5, got {step_counts['matched']}"
    # 3 persons had no dst-field → inserted = 3
    assert step_counts["inserted"] == 3, f"inserted: expected 3, got {step_counts['inserted']}"
    # 2 persons already had dst-field → skipped = 2
    assert step_counts["skipped"] == 2, f"skipped: expected 2, got {step_counts['skipped']}"

    # ── Step 5: assert effect in TypeDB ─────────────────────────────────────
    # The 3 previously-dst-less persons must now have dst-field == src-field.
    all_after = BackfillPerson.manager(clean_db).all()
    assert len(all_after) == 5

    persons_by_src: dict[str, BackfillPerson] = {p.src_field.value: p for p in all_after}

    # Inserted group: dst-field must now equal src-field value.
    for name_val in ("alice", "bob", "carol"):
        p = persons_by_src[name_val]
        assert p.dst_field is not None, f"{name_val}: dst-field must be set after backfill"
        assert p.dst_field.value == name_val, (
            f"{name_val}: dst-field value must equal src-field; got {p.dst_field.value!r}"
        )

    # Skipped group: dst-field must keep the original value (not overwritten).
    dave = persons_by_src["dave"]
    assert dave.dst_field is not None
    assert dave.dst_field.value == "dave-orig", (
        f"dave: original dst-field must be preserved; got {dave.dst_field.value!r}"
    )

    eve = persons_by_src["eve"]
    assert eve.dst_field is not None
    assert eve.dst_field.value == "eve-orig", (
        f"eve: original dst-field must be preserved; got {eve.dst_field.value!r}"
    )

    # ── Step 6: rollback and verify ──────────────────────────────────────────
    # Rolling back to the initial schema migration removes the CopyAttribute
    # migration.  The rollback TypeQL deletes ALL dst-field values from all
    # BackfillPerson instances (including the 2 pre-existing ones).
    rollback_results = executor.migrate(target=initial_name)
    assert len(rollback_results) == 1
    rb = rollback_results[0]
    assert rb.success, f"rollback failed: {rb.error}"
    assert rb.action == "rolled_back"

    # After rollback: NO person should have dst-field.
    all_after_rollback = BackfillPerson.manager(clean_db).all()
    assert len(all_after_rollback) == 5

    for p in all_after_rollback:
        assert p.dst_field is None, (
            f"person {p.src_field.value!r}: dst-field must be removed after rollback; "
            f"got {p.dst_field!r}"
        )
