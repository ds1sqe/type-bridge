# pyright: reportMissingImports=false
"""Unit tests for ops.CopyAttribute — TypeQL text + lowering to a write-typed step.

These tests run without a live TypeDB connection.  The Rust extension is imported
where required; any test that needs it is guarded by ``pytest.importorskip``.
"""

from __future__ import annotations

from typing import ClassVar

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import operations as ops

# ── Test fixtures ──────────────────────────────────────────────────────────────


class CopyAttrSource(String):
    flags = AttributeFlags(name="old-name")


class CopyAttrDest(String):
    flags = AttributeFlags(name="new-name")


class CopyAttrAge(Integer):
    flags = AttributeFlags(name="copy-age")


class CopyAttrPerson(Entity):
    flags = TypeFlags(name="person")

    key: CopyAttrSource = Flag(Key)


# ── to_typeql() ────────────────────────────────────────────────────────────────


def test_copy_attribute_to_typeql_insert_if_absent_pattern() -> None:
    """to_typeql() must emit an insert-if-absent match+insert."""
    op = ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name")
    typeql = op.to_typeql()

    assert "match" in typeql, "forward must contain 'match'"
    assert "insert" in typeql, "forward must contain 'insert'"
    assert "not {" in typeql, "forward must contain the not-guard"
    assert "$x isa person" in typeql, "forward must reference the owner type"
    assert "has old-name $v" in typeql, "forward must reference the source attribute"
    # The destination is written by VALUE assignment (`has <dest> == $v`); a bare
    # `has <dest> $v` fails TypeDB type inference because $v is a source instance.
    assert "has new-name == $v" in typeql, (
        "forward must copy by value (`has new-name == $v`), not bind $v as a dest instance"
    )


def test_copy_attribute_to_typeql_with_filter() -> None:
    """When filter is set it must appear in the forward TypeQL."""
    op = ops.CopyAttribute(
        owner=CopyAttrPerson,
        source="old-name",
        dest="new-name",
        filter="$x has copy-age $a; $a > 0;",
    )
    typeql = op.to_typeql()
    assert "$x has copy-age $a; $a > 0;" in typeql, "filter fragment must appear in forward TypeQL"


def test_copy_attribute_to_typeql_no_filter_default() -> None:
    """When filter is None no extra predicate line should appear."""
    op = ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name")
    typeql = op.to_typeql()
    # The filter placeholder should not be present.
    assert "None" not in typeql, "None must not appear in TypeQL text when filter is not set"


# ── to_rollback_typeql() ───────────────────────────────────────────────────────


def test_copy_attribute_to_rollback_typeql_deletes_dest() -> None:
    """to_rollback_typeql() must emit a match+delete that removes the dest."""
    op = ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name")
    rollback = op.to_rollback_typeql()

    assert rollback is not None, "CopyAttribute must be reversible"
    assert "has new-name" in rollback, "rollback must match the destination attribute"
    assert "person" in rollback, "rollback must reference the owner type"
    # TypeDB 3.x ownership-delete syntax is `delete <attr> of <owner>`; the older
    # `delete $x has <dest> $v` is a 3.x syntax error.
    assert "delete $v of $x" in rollback, "rollback must use 3.x `delete $v of $x` syntax"


def test_copy_attribute_is_reversible() -> None:
    """The reversible property must be True."""
    op = ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name")
    assert op.reversible, "CopyAttribute must be reversible"


# ── Lowering: pure-IR path (_operation_spec) ──────────────────────────────────


def test_copy_attribute_lowers_to_copy_attribute_ir() -> None:
    """_operation_spec carries the op's TypeQL (one source — invariant 2)."""
    from type_bridge.migration._lower import _operation_spec  # type: ignore[attr-defined]

    op = ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name")
    spec = _operation_spec(op)

    assert spec["kind"] == "copy_attribute"
    # The IR carries the forward/reverse TypeQL from to_typeql(), not fields.
    assert spec["forward"] == op.to_typeql()
    assert spec["reverse"] == op.to_rollback_typeql()
    assert "has new-name == $v" in spec["forward"]


def test_copy_attribute_with_filter_carries_filter_in_forward() -> None:
    """The filter is embedded in the carried forward TypeQL, not a separate field."""
    from type_bridge.migration._lower import _operation_spec  # type: ignore[attr-defined]

    op = ops.CopyAttribute(
        owner=CopyAttrPerson,
        source="old-name",
        dest="new-name",
        filter="$x has copy-age $a;",
    )
    spec = _operation_spec(op)

    assert spec["kind"] == "copy_attribute"
    assert "$x has copy-age $a;" in spec["forward"]


# ── Lowering: execution path (lower_execution_migration) ──────────────────────


@pytest.fixture
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


@pytest.mark.usefixtures("_requires_rust_extension")
def test_copy_attribute_execution_lowering_produces_write_typed_step() -> None:
    """lower_execution_migration must produce a step with kind='copy_attribute' for CopyAttribute."""
    from pathlib import Path

    from type_bridge.migration._lower import lower_execution_migration
    from type_bridge.migration.base import Migration
    from type_bridge.migration.loader import LoadedMigration

    class BackfillMigration(Migration):
        operations: ClassVar[list] = [
            ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name"),
        ]

    migration = BackfillMigration()
    migration.app_label = "test"
    migration.name = "0001_backfill"
    loaded = LoadedMigration(
        migration=migration,
        path=Path("0001_backfill.py"),
        checksum="test-csum",
    )

    spec = lower_execution_migration(loaded)

    # The migration must have exactly one operation.
    ops_out = spec["operations"]
    assert len(ops_out) == 1, f"expected 1 operation, got {len(ops_out)}"

    op_spec = ops_out[0]
    # The execution lowering must emit copy_attribute so the Rust planner can
    # assign StepKind::Backfill and TxType::Write.
    assert op_spec["kind"] == "copy_attribute", (
        f"CopyAttribute execution lowering must emit kind='copy_attribute'; got {op_spec['kind']!r}"
    )


@pytest.mark.usefixtures("_requires_rust_extension")
def test_copy_attribute_execution_lowering_in_execution_graph() -> None:
    """lower_execution_graph must emit copy_attribute ops for CopyAttribute."""
    from pathlib import Path

    from type_bridge.migration._lower import lower_execution_graph
    from type_bridge.migration.base import Migration
    from type_bridge.migration.loader import LoadedMigration

    class BackfillMigration2(Migration):
        operations: ClassVar[list] = [
            ops.CopyAttribute(owner=CopyAttrPerson, source="old-name", dest="new-name"),
        ]

    migration = BackfillMigration2()
    migration.app_label = "test"
    migration.name = "0002_backfill"
    loaded = LoadedMigration(
        migration=migration,
        path=Path("0002_backfill.py"),
        checksum="test-csum2",
    )

    graph = lower_execution_graph([loaded])
    migration_ops = graph["migrations"][0]["operations"]
    assert len(migration_ops) == 1

    op_spec = migration_ops[0]
    # The execution graph must carry the copy_attribute op spec so the Rust
    # planner can assign StepKind::Backfill and TxType::Write.
    assert op_spec["kind"] == "copy_attribute", (
        f"execution graph must emit kind='copy_attribute' for CopyAttribute; "
        f"got {op_spec['kind']!r}"
    )
    # The execution graph carries the backfill TypeQL (value-copy form), not fields.
    assert "$x isa person" in op_spec["forward"]
    assert "has new-name == $v" in op_spec["forward"]
