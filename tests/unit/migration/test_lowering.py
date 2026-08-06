from __future__ import annotations

# pyright: reportMissingImports=false
from pathlib import Path
from typing import ClassVar

import pytest

from tests.utils.handwritten import (
    AttributeFlags,
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.migration import _operations as ops
from type_bridge.migration._archive_base import _ArchivedMigration as Migration
from type_bridge.migration._lower import (
    _schema_info_for_models,
    lower_loaded_migration,
    lower_migration,
    lower_migration_graph,
    lower_operation,
)
from type_bridge.migration.loader import LoadedMigration, MigrationLoader


class LowerName(String):
    flags = AttributeFlags(name="lower-name")


class LowerAge(Integer):
    flags = AttributeFlags(name="lower-age")


class LowerCode(String):
    flags = AttributeFlags(name="lower-code")
    regex_pattern = r"^[A-Z]{3}$"


class LowerPerson(Entity):
    flags = TypeFlags(name="lower-person")

    name: LowerName = Flag(Key)


class LowerPersonWithAge(Entity):
    flags = TypeFlags(name="lower-person")

    name: LowerName = Flag(Key)
    age: LowerAge | None = None


class LowerCompany(Entity):
    flags = TypeFlags(name="lower-company")

    name: LowerName = Flag(Key)


class LowerEmployment(Relation):
    flags = TypeFlags(name="lower-employment")

    employee: Role[LowerPerson] = Role("employee", LowerPerson)
    employer: Role[LowerCompany] = Role("employer", LowerCompany)


@pytest.fixture(autouse=True)
def _requires_rust_extension() -> None:
    pytest.importorskip("type_bridge_core")


@pytest.mark.parametrize(
    ("operation", "kind"),
    [
        (ops.AddAttribute(LowerAge), "add_attribute"),
        (ops.RemoveAttribute(LowerAge), "remove_attribute"),
        (ops.AddEntity(LowerPerson), "add_entity"),
        (ops.RemoveEntity(LowerPerson), "remove_entity"),
        (ops.AddOwnership(LowerPerson, LowerAge, optional=True), "add_ownership"),
        (ops.RemoveOwnership(LowerPerson, LowerAge), "remove_ownership"),
        (
            ops.ModifyOwnership(
                LowerPerson,
                LowerAge,
                old_annotations="@card(0..1)",
                new_annotations="@card(1..1)",
            ),
            "modify_ownership",
        ),
        (ops.AddRelation(LowerEmployment), "add_relation"),
        (ops.RemoveRelation(LowerEmployment), "remove_relation"),
        (ops.AddRole(LowerEmployment, "reviewer", ["lower-person"]), "add_role"),
        (ops.RemoveRole(LowerEmployment, "reviewer"), "remove_role"),
        (ops.AddRolePlayer(LowerEmployment, "employee", "lower-company"), "add_role_player"),
        (
            ops.RemoveRolePlayer(LowerEmployment, "employee", "lower-company"),
            "remove_role_player",
        ),
        (ops.RunTypeQL("define attribute lower-nick, value string;"), "run_typeql"),
        (ops.RenameAttribute("lower-name", "lower-full-name", "string"), "rename_attribute"),
        (
            ops.ModifyTypeAnnotations(
                LowerPerson,
                new_doc="A person.",
                new_meta={"owner": "core"},
            ),
            "modify_type_annotations",
        ),
        (
            ops.ModifyRoleAnnotations(
                LowerEmployment,
                "employee",
                new_doc="The employed party.",
            ),
            "modify_role_annotations",
        ),
    ],
)
def test_every_current_operation_lowers_to_rust_normalized_spec(
    operation: ops.Operation, kind: str
) -> None:
    spec = lower_operation(operation)

    assert spec["kind"] == kind


def test_schema_bearing_operations_lower_to_typed_payloads() -> None:
    entity = lower_operation(ops.AddEntity(LowerPerson))
    relation = lower_operation(ops.AddRelation(LowerEmployment))
    attribute = lower_operation(ops.AddAttribute(LowerAge))
    ownership = lower_operation(ops.AddOwnership(LowerPerson, LowerAge, optional=True))

    assert entity["entity"]["type_name"] == "lower-person"
    assert entity["entity"]["owned_attributes"] == [
        {
            "attr_name": "lower-name",
            "value_type": "string",
            "annotations": ["Key"],
            "is_ordered": False,
        }
    ]
    assert relation["relation"]["roles"] == [
        {
            "role_name": "employee",
            "player_type_names": ["lower-person"],
            "cardinality": None,
            "overrides": None,
            "is_abstract": False,
            "ordered": False,
            "distinct": False,
        },
        {
            "role_name": "employer",
            "player_type_names": ["lower-company"],
            "cardinality": None,
            "overrides": None,
            "is_abstract": False,
            "ordered": False,
            "distinct": False,
        },
    ]
    assert attribute["attribute"] == {
        "attr_name": "lower-age",
        "value_type": "long",
    }
    assert ownership["attribute"] == {
        "attr_name": "lower-age",
        "value_type": "long",
        "annotations": [{"Card": (0, 1)}],
        "is_ordered": False,
    }


def test_model_based_migration_lowers_to_define_schema() -> None:
    class InitialMigration(Migration):
        models: ClassVar[list[type[Entity | Relation]]] = [
            LowerPerson,
            LowerCompany,
            LowerEmployment,
        ]

    migration = InitialMigration()
    migration.app_label = "lower"
    migration.name = "0001_initial"

    spec = lower_migration(migration)

    assert spec["app_label"] == "lower"
    assert spec["operations"][0]["kind"] == "define_schema"
    schema = spec["operations"][0]["schema"]
    assert set(schema["entities"]) == {"lower-company", "lower-person"}
    assert set(schema["relations"]) == {"lower-employment"}
    assert schema["attributes"]["lower-name"] == {
        "attr_name": "lower-name",
        "value_type": "string",
    }


def test_model_based_migration_preserves_attribute_type_annotations() -> None:
    class LowerAnnotated(Entity):
        flags = TypeFlags(name="lower-annotated")

        code: LowerCode

    class InitialMigration(Migration):
        models: ClassVar[list[type[Entity | Relation]]] = [LowerAnnotated]

    migration = InitialMigration()
    migration.app_label = "lower"
    migration.name = "0001_initial"

    spec = lower_migration(migration)

    assert spec["operations"][0]["schema"]["attributes"]["lower-code"] == {
        "attr_name": "lower-code",
        "value_type": "string",
        "regex": r"^[A-Z]{3}$",
    }


def test_loaded_migration_checksum_is_carried() -> None:
    class CustomMigration(Migration):
        dependencies: ClassVar[list[tuple[str, str]]] = [("lower", "0001_initial")]
        operations: ClassVar[list[ops.Operation]] = [
            ops.RunTypeQL(
                forward="define attribute lower-custom, value string;",
                reverse="undefine attribute lower-custom;",
            )
        ]

    migration = CustomMigration()
    migration.app_label = "lower"
    migration.name = "0002_custom"
    loaded = LoadedMigration(
        migration=migration,
        path=Path("0002_custom.py"),
        checksum="abc123",
    )

    spec = lower_loaded_migration(loaded)

    assert spec["checksum"] == "abc123"
    assert spec["dependencies"] == [{"app_label": "lower", "migration_name": "0001_initial"}]
    assert spec["operations"][0] == {
        "kind": "run_typeql",
        "forward": "define attribute lower-custom, value string;",
        "reverse": "undefine attribute lower-custom;",
    }


def test_migration_graph_preserves_loaded_order() -> None:
    first = Migration()
    first.app_label = "lower"
    first.name = "0001_initial"
    second = Migration()
    second.app_label = "lower"
    second.name = "0002_next"

    graph = lower_migration_graph(
        [
            LoadedMigration(first, Path("0001_initial.py"), "aaa"),
            LoadedMigration(second, Path("0002_next.py"), "bbb"),
        ]
    )

    assert [migration["name"] for migration in graph["migrations"]] == [
        "0001_initial",
        "0002_next",
    ]


def test_generated_run_typeql_migration_lowers_without_model_imports(tmp_path: Path) -> None:
    migration_file = tmp_path / "0001_generated.py"
    migration_file.write_text(
        """
from typing import ClassVar
from type_bridge.migration import Migration, operations as ops
from type_bridge.migration.operations import Operation


class GeneratedMigration(Migration):
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL(
            forward="define attribute generated-lower, value string;",
            reverse="undefine attribute generated-lower;",
        )
    ]
""".lstrip()
    )

    loaded = MigrationLoader(tmp_path).discover()[0]
    spec = lower_loaded_migration(loaded)

    assert spec["operations"] == [
        {
            "kind": "run_typeql",
            "forward": "define attribute generated-lower, value string;",
            "reverse": "undefine attribute generated-lower;",
        }
    ]


def test_unsupported_operation_raises_type_error() -> None:
    class UnknownOperation(ops.Operation):
        def to_typeql(self) -> str:
            return ""

        def to_rollback_typeql(self) -> str | None:
            return None

    with pytest.raises(TypeError, match="Unsupported migration operation type"):
        lower_operation(UnknownOperation())


# ---------------------------------------------------------------------------
# Projection-collapse regression pins (Phase 2: registry path)
# ---------------------------------------------------------------------------


class PlaysCardPlayer(Entity):
    flags = TypeFlags(name="plays-card-player")


class PlaysCardRelation(Relation):
    flags = TypeFlags(name="plays-card-relation")

    participant: Role[PlaysCardPlayer] = Role(
        "participant", PlaysCardPlayer, plays_cardinality=Card(0, 1)
    )


def test_migration_lowering_carries_plays_cardinality_overlay() -> None:
    """plays_cardinality on a Role reaches the player entry's plays_cardinalities via Rust from_descriptors."""
    schema = _schema_info_for_models([PlaysCardPlayer, PlaysCardRelation])

    assert schema["entities"]["plays-card-player"]["plays_cardinalities"] == {
        "plays-card-relation:participant": (0, 1)
    }
    assert schema["relations"]["plays-card-relation"]["plays_cardinalities"] == {}


class ForeignParentBase(Entity):
    flags = TypeFlags(name="foreign-parent-base")


class ForeignParentChild(ForeignParentBase):
    flags = TypeFlags(name="foreign-parent-child")


def test_migration_lowering_nulls_foreign_parent() -> None:
    """A model whose parent is absent from the lowered set gets parent_type=null from Rust from_descriptors."""
    # Register only the child — the parent type "foreign-parent-base" is not in the set.
    schema = _schema_info_for_models([ForeignParentChild])

    entry = schema["entities"]["foreign-parent-child"]
    assert entry["parent_type"] is None


def test_modify_type_annotations_lowers_full_payload() -> None:
    spec = lower_operation(
        ops.ModifyTypeAnnotations(
            LowerPerson,
            old_doc="old",
            new_doc="new",
            old_meta={"gone": "1"},
            new_meta={"added": "2"},
        )
    )
    assert spec["kind"] == "modify_type_annotations"
    assert spec["type_name"] == "lower-person"
    assert spec["old_doc"] == "old"
    assert spec["new_doc"] == "new"
    assert spec["old_meta"] == {"gone": "1"}
    assert spec["new_meta"] == {"added": "2"}


def test_modify_type_annotations_accepts_attribute_subject() -> None:
    spec = lower_operation(ops.ModifyTypeAnnotations(LowerAge, new_doc="An age."))
    assert spec["kind"] == "modify_type_annotations"
    assert spec["type_name"] == "lower-age"


def test_modify_role_annotations_lowers_relation_and_role() -> None:
    spec = lower_operation(ops.ModifyRoleAnnotations(LowerEmployment, "employee", new_doc="doc"))
    assert spec["kind"] == "modify_role_annotations"
    assert spec["relation_type"] == "lower-employment"
    assert spec["role_name"] == "employee"


def test_annotation_operations_emit_stepwise_typeql() -> None:
    operation = ops.ModifyTypeAnnotations(
        LowerPerson,
        old_doc="old doc",
        new_doc="new doc",
        old_meta={"gone": "1"},
        new_meta={"added": "2"},
    )
    assert operation.to_typeql_steps() == [
        'undefine\n@meta("gone") from lower-person;',
        'redefine\nlower-person @doc("new doc");',
        'define\nlower-person @meta("added", "2");',
    ]
    assert operation.to_rollback_typeql_steps() == [
        'undefine\n@meta("added") from lower-person;',
        'redefine\nlower-person @doc("old doc");',
        'define\nlower-person @meta("gone", "1");',
    ]
    assert operation.reversible


def test_role_annotation_operation_uses_relates_subject() -> None:
    operation = ops.ModifyRoleAnnotations(
        LowerEmployment, "employee", new_doc="The employed party."
    )
    assert operation.to_typeql_steps() == [
        'define\nlower-employment relates employee @doc("The employed party.");',
    ]
    assert operation.to_rollback_typeql_steps() == [
        "undefine\n@doc from lower-employment relates employee;",
    ]
