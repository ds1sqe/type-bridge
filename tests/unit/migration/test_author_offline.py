"""Offline artifact authoring tests (#166).

Everything here runs without a TypeDB connection, ``Database``, model
registry, or generated application package: authoring consumes serialized
``SchemaInfo`` dictionaries and returns the complete artifact set in
memory. Model classes below exist only to build the schema fixtures.
"""

from __future__ import annotations

# pyright: reportMissingImports=false
import json
from pathlib import Path

import pytest

from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags, _rust_runtime
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import author_migration
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.snapshots import generate_snapshot


class AoName(String):
    flags = AttributeFlags(name="ao-name")


class AoLinkId(String):
    flags = AttributeFlags(name="ao-link-id")


class AoPerson(Entity):
    flags = TypeFlags(name="ao-person")
    name: AoName = Flag(Key)


class AoBadge(Entity):
    flags = TypeFlags(name="ao-badge")
    name: AoName = Flag(Key)


class AoLegacyLink(Relation):
    flags = TypeFlags(name="ao-legacy-link")
    subject: Role[AoPerson] = Role("subject", AoPerson)
    badge: Role[AoBadge] = Role("badge", AoBadge)
    link_id: AoLinkId = Flag(Key)


class AoLegacyTag(String):
    flags = AttributeFlags(name="ao-legacy-tag")


class AoTag(String):
    flags = AttributeFlags(name="ao-tag")


class AoItemV1(Entity):
    flags = TypeFlags(name="ao-item")
    tag: AoLegacyTag = Flag(Key)


class AoItemV2(Entity):
    flags = TypeFlags(name="ao-item")
    tag: AoTag = Flag(Key)


@pytest.fixture(autouse=True)
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


def _v1() -> SchemaInfo:
    info = SchemaInfo()
    info.entities = [AoPerson, AoBadge]
    info.attribute_classes = {AoName}
    return info


def _v2() -> SchemaInfo:
    info = SchemaInfo()
    info.entities = [AoPerson, AoBadge]
    info.relations = [AoLegacyLink]
    info.attribute_classes = {AoName, AoLinkId}
    return info


def _author(base: SchemaInfo, target: SchemaInfo, **overrides):
    kwargs = {
        "app_label": "migrations",
        "name": "0002_add_link",
        "dependencies": [("migrations", "0001_initial")],
        "snapshot_version": "v0002",
        "previous_snapshot_version": "v0001",
        "generated_at": "2026-07-13T00:00:00+00:00",
    }
    kwargs.update(overrides)
    return author_migration(
        base.to_rust_schema_info(),
        target.to_rust_schema_info(),
        **kwargs,
    )


def test_authors_complete_artifact_set_in_memory() -> None:
    authored = _author(_v1(), _v2())

    assert authored is not None
    paths = [path for path, _ in authored.files]
    assert paths == [
        "0002_add_link.py",
        "0002_add_link.json",
        "snapshots/__init__.py",
        "snapshots/v0002/__init__.py",
        "snapshots/v0002/attributes.py",
        "snapshots/v0002/declared-schema.json",
        "snapshots/v0002/entities.py",
        "snapshots/v0002/registry.py",
        "snapshots/v0002/relations.py",
        "snapshots/v0002/schema.tql",
        "snapshots/v0002/snapshot.json",
    ]


def test_authoring_is_deterministic() -> None:
    first = _author(_v1(), _v2())
    second = _author(_v1(), _v2())

    assert first is not None and second is not None
    assert first.python_source == second.python_source
    assert first.files == second.files


def test_no_op_diff_returns_none() -> None:
    assert _author(_v1(), _v1()) is None


def test_sidecar_checksum_matches_returned_py_bytes() -> None:
    authored = _author(_v1(), _v2())

    assert authored is not None
    sidecar = json.loads(dict(authored.files)["0002_add_link.json"])
    expected = _rust_runtime.migration_file_checksum(authored.python_source)
    assert sidecar["checksum"] == expected


def test_py_and_sidecar_represent_the_same_operations_in_order() -> None:
    authored = _author(
        _v1(),
        _v2(),
        before_schema=[{"kind": "run_typeql", "forward": "match $x isa legacy; delete $x;"}],
        after_schema=[{"kind": "run_typeql", "forward": "insert $p isa ao-person;"}],
    )

    assert authored is not None
    kinds = [op["kind"] for op in authored.spec["operations"]]
    assert kinds == ["run_typeql", "add_attribute", "add_relation", "run_typeql"]

    source = authored.python_source
    cleanup = source.index("match $x isa legacy")
    add_link = source.index("ops.AddRelation(")
    backfill = source.index("insert $p isa ao-person")
    assert cleanup < add_link < backfill


def test_offline_flow_passes_offline_validation_and_planning(tmp_path: Path) -> None:
    authored = _author(_v1(), _v2(), dependencies=[])
    assert authored is not None
    migrations_dir = tmp_path / "migrations"
    authored.write_to(migrations_dir)

    graph = {
        "migrations": [
            _rust_runtime.rust_core().load_migration_sidecar(
                str(migrations_dir / "0002_add_link.py")
            )
        ]
    }
    assert _rust_runtime.rust_core().validate_migration_graph(graph, []) == []
    plan = _rust_runtime.plan_migration_graph(graph, [], None)
    assert len(plan["to_apply"]) == 1


def test_write_to_is_idempotent_and_detects_drift(tmp_path: Path) -> None:
    authored = _author(_v1(), _v2())
    assert authored is not None
    migrations_dir = tmp_path / "migrations"
    path = authored.write_to(migrations_dir)
    assert path == migrations_dir / "0002_add_link.py"

    # Identical rewrite is fine (append-only snapshots validate as identical).
    authored.write_to(migrations_dir)

    # A drifted snapshot blocks the whole write before anything is touched.
    (migrations_dir / "snapshots" / "v0002" / "schema.tql").write_text("drift")
    with pytest.raises(ValueError, match="different contents"):
        authored.write_to(migrations_dir)

    with pytest.raises(ValueError, match="already exists"):
        authored.write_to(migrations_dir, on_existing="fail")


def test_snapshot_files_match_the_python_snapshot_pipeline(tmp_path: Path) -> None:
    """Rust-rendered snapshot bytes equal the historical Python pipeline's."""
    target = _v2()
    authored = _author(_v1(), target)
    assert authored is not None
    rust_files = {
        path.removeprefix("snapshots/v0002/"): contents
        for path, contents in authored.files
        if path.startswith("snapshots/v0002/")
    }

    migrations_dir = tmp_path / "migrations"
    migrations_dir.mkdir()
    snapshot_dir = generate_snapshot(
        migrations_dir=migrations_dir,
        version="v0002",
        migration_name="0002_add_link",
        schema_text=target.to_typeql(),
    )
    python_files = {
        file.name: file.read_bytes() for file in sorted(snapshot_dir.glob("*")) if file.is_file()
    }

    assert set(rust_files) == set(python_files)
    for name in sorted(python_files):
        if name == "snapshot.json":
            # Key order can differ between serializers; compare the payload.
            assert json.loads(rust_files[name]) == json.loads(python_files[name])
            continue
        assert rust_files[name] == python_files[name], f"{name} bytes differ"


def test_whole_relation_removal_stays_single_operation() -> None:
    """The canonical mapper enforces #168 in the offline path too."""
    authored = _author(_v2(), _v1(), name="0003_drop_link", snapshot_version="v0003")

    assert authored is not None
    kinds = [op["kind"] for op in authored.spec["operations"]]
    assert kinds == ["remove_relation", "remove_attribute"]


def test_structured_copy_attribute_authors_portably() -> None:
    """A structured copy_attribute dict renders a faithful ops.CopyAttribute
    and its sidecar TypeQL is byte-identical to the frozen Python lowering.

    This is the parity pin for the Rust synthesis: if either template
    drifts, this test fails.
    """
    from type_bridge.migration import operations as ops

    authored = _author(
        _v1(),
        _v2(),
        after_schema=[
            {
                "kind": "copy_attribute",
                "owner": "ao-person",
                "source": "ao-name",
                "dest": "ao-link-id",
                "filter": None,
            }
        ],
    )

    assert authored is not None
    assert (
        "ops.CopyAttribute(AoPerson, source='ao-name', dest='ao-link-id')" in authored.python_source
    )

    python_op = ops.CopyAttribute(owner=AoPerson, source="ao-name", dest="ao-link-id")
    copy = authored.spec["operations"][-1]
    assert copy["kind"] == "copy_attribute"
    assert copy["forward"] == python_op.to_typeql()
    assert copy["reverse"] == python_op.to_rollback_typeql()


def test_incomplete_copy_attribute_is_rejected() -> None:
    with pytest.raises(ValueError, match="owner/source/dest"):
        _author(
            _v1(),
            _v2(),
            after_schema=[{"kind": "copy_attribute", "owner": "ao-person"}],
        )


def _item_schema(version: type) -> SchemaInfo:
    info = SchemaInfo()
    info.entities = [version]
    info.attribute_classes = {AoLegacyTag if version is AoItemV1 else AoTag}
    return info


def test_attribute_rename_authors_the_staged_expansion() -> None:
    """A rename directive replaces the diff's remove+add with the staged
    data-preserving primitive sequence."""
    authored = _author(
        _item_schema(AoItemV1),
        _item_schema(AoItemV2),
        name="0002_rename_tag",
        attribute_renames=[("ao-legacy-tag", "ao-tag")],
    )

    assert authored is not None
    kinds = [op["kind"] for op in authored.spec["operations"]]
    assert kinds == [
        "add_attribute",  # ao-tag with the target definition
        "add_ownership",  # ao-item owns ao-tag — plain, staged
        "copy_attribute",  # backfill ao-legacy-tag -> ao-tag
        "modify_ownership",  # tighten to @key after the backfill
        "modify_ownership",  # loosen @key on ao-legacy-tag pre-delete
        "run_typeql",  # delete ao-legacy-tag instances (irreversible)
        "remove_ownership",  # detach the emptied ownership
        "remove_attribute",  # undefine ao-legacy-tag
    ]

    # The .py is the reviewable primitive recipe, not an opaque marker.
    assert "ops.CopyAttribute(AoItem, source='ao-legacy-tag', dest='ao-tag')" in (
        authored.python_source
    )
    assert "ops.RenameAttribute" not in authored.python_source

    # Without the directive the same schemas map to a data-destroying
    # remove+add.
    plain = _author(_item_schema(AoItemV1), _item_schema(AoItemV2), name="0002_rename_tag")
    assert plain is not None
    assert [op["kind"] for op in plain.spec["operations"]] == [
        "add_attribute",
        "add_ownership",
        "remove_ownership",
        "remove_attribute",
    ]


def test_inconsistent_rename_directive_is_rejected() -> None:
    with pytest.raises(ValueError, match="does not exist in the base schema"):
        _author(
            _item_schema(AoItemV1),
            _item_schema(AoItemV2),
            attribute_renames=[("ghost", "ao-tag")],
        )


def test_rename_attribute_placeholder_refuses_to_execute() -> None:
    """The historical placeholder half-executed (defined the new attribute,
    swallowed the data migration in comments); it must fail loudly now."""
    from type_bridge.migration import operations as ops

    op = ops.RenameAttribute("ao-legacy-tag", "ao-tag", "string")
    with pytest.raises(NotImplementedError, match="attribute_renames"):
        op.to_typeql()


def test_unsupported_operations_error_instead_of_dropping() -> None:
    # The TypeQL-only lowered form cannot round-trip to a faithful
    # ops.CopyAttribute(...); only the structured form is authorable.
    with pytest.raises(ValueError, match="no .py authoring form"):
        _author(
            _v1(),
            _v2(),
            before_schema=[{"kind": "copy_attribute", "forward": "match ..."}],
        )
