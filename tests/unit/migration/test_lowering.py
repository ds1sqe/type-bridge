from __future__ import annotations

# pyright: reportMissingImports=false
import importlib
import sys
from pathlib import Path
from typing import ClassVar

import pytest

from type_bridge import (
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
    _rust_runtime,
)
from type_bridge.attribute import AttributeFlags
from type_bridge.generator import generate_models
from type_bridge.migration import operations as ops
from type_bridge.migration._lower import (
    _schema_info_for_models,
    lower_loaded_migration,
    lower_migration,
    lower_migration_graph,
    lower_operation,
)
from type_bridge.migration.base import Migration
from type_bridge.migration.generator import MigrationGenerator
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedSchema,
)
from type_bridge.migration.loader import LoadedMigration, MigrationLoader
from type_bridge.migration.registry import ModelRegistry
from type_bridge.migration.schema_manager import SchemaManager


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


def test_generator_renders_typed_operation_source() -> None:
    generator = MigrationGenerator.__new__(MigrationGenerator)

    rendered = generator._render_operations(
        [
            ops.AddAttribute(LowerAge),
            ops.AddEntity(LowerPerson),
            ops.AddOwnership(LowerPerson, LowerAge, optional=True),
            ops.RunTypeQL(
                forward="define attribute lower-nick, value string;",
                reverse="undefine attribute lower-nick;",
            ),
        ]
    )

    assert "ops.AddAttribute(LowerAge)" in rendered
    assert "ops.AddEntity(LowerPerson)" in rendered
    assert "ops.AddOwnership(LowerPerson, LowerAge, optional=True)" in rendered
    assert "ops.RunTypeQL(" in rendered


def test_generator_sidecar_preserves_typed_operation_specs(tmp_path: Path) -> None:
    generator = MigrationGenerator.__new__(MigrationGenerator)
    generator.migrations_dir = tmp_path
    py_path = tmp_path / "0001_initial.py"
    py_path.write_text("# generated migration\n")

    generator._write_sidecar(
        py_path,
        [
            ops.AddAttribute(LowerAge),
            ops.AddEntity(LowerPerson),
            ops.AddOwnership(LowerPerson, LowerAge, optional=True),
            ops.RunTypeQL(
                forward="define attribute lower-nick, value string;",
                reverse="undefine attribute lower-nick;",
            ),
        ],
        dependencies=[],
        migration_name="0001_initial",
    )

    sidecar = _rust_runtime.migration_spec_from_json(py_path.with_suffix(".json").read_text())

    assert [operation["kind"] for operation in sidecar["operations"]] == [
        "add_attribute",
        "add_entity",
        "add_ownership",
        "run_typeql",
    ]
    assert sidecar["operations"][0]["attribute"]["attr_name"] == "lower-age"
    assert sidecar["operations"][1]["entity"]["type_name"] == "lower-person"
    assert sidecar["operations"][2]["owner_type"] == "lower-person"


def test_bindgen_package_can_render_importable_typed_migration(tmp_path: Path) -> None:
    schema_path = tmp_path / "schema.toml"
    schema_path.write_text(
        """
[attributes.customer-name]
value = "string"

[attributes.email]
value = "string"

[entities.customer]
owns = [
    { attribute = "customer-name", key = true },
    { attribute = "email", card = "0..1" },
]
""".lstrip()
    )
    package_dir = tmp_path / "generated_models"
    generate_models(schema_path, package_dir)

    sys.path.insert(0, str(tmp_path))
    try:
        models = ModelRegistry.discover("generated_models", register=False)
        package = importlib.import_module("generated_models")
        email = next(
            attribute
            for attribute in package.ATTRIBUTES
            if attribute.get_attribute_name() == "email"
        )
        customer = models[0]
        generator = MigrationGenerator.__new__(MigrationGenerator)
        operations = [ops.AddAttribute(email), ops.AddOwnership(customer, email, optional=True)]
        operations_code = generator._render_operations(operations)
        imports_code = generator._generate_operations_imports(operations)
        migrations_dir = tmp_path / "migrations"
        migrations_dir.mkdir()
        migration_file = migrations_dir / "0001_add_email.py"
        migration_file.write_text(
            generator._render_migration(
                class_name="AddEmailMigration",
                dependencies=[],
                operations_code=operations_code,
                models_code="",
                imports_code=imports_code,
                description="add email",
            )
        )

        loaded = MigrationLoader(migrations_dir).discover()[0]
    finally:
        sys.path.remove(str(tmp_path))
        ModelRegistry.clear()
        for module_name in list(sys.modules):
            if module_name == "generated_models" or module_name.startswith("generated_models."):
                del sys.modules[module_name]

    add_attribute = loaded.migration.operations[0]
    add_ownership = loaded.migration.operations[1]
    assert isinstance(add_attribute, ops.AddAttribute)
    assert isinstance(add_ownership, ops.AddOwnership)
    assert add_attribute.attribute.get_attribute_name() == "email"
    assert add_ownership.owner.get_type_name() == "customer"


def test_bindgen_optional_cardinality_survives_generated_diff(tmp_path: Path) -> None:
    schema_path = tmp_path / "schema.toml"
    schema_path.write_text(
        """
[attributes.customer-name]
value = "string"

[attributes.email]
value = "string"

[entities.customer]
owns = [
    { attribute = "customer-name", key = true },
    { attribute = "email", card = "0..1" },
]
""".lstrip()
    )
    package_dir = tmp_path / "generated_models"
    generate_models(schema_path, package_dir)

    sys.path.insert(0, str(tmp_path))
    try:
        models = ModelRegistry.discover("generated_models", register=False)
        schema_mgr = SchemaManager(None)  # type: ignore[arg-type]
        schema_mgr.register(*models)
        model_info = schema_mgr.collect_schema_info()

        current_schema = IntrospectedSchema(
            entities={"customer": IntrospectedEntity(name="customer")},
            attributes={
                "customer-name": IntrospectedAttribute(
                    name="customer-name",
                    value_type="string",
                )
            },
            ownerships=[
                IntrospectedOwnership(
                    owner_name="customer",
                    attribute_name="customer-name",
                    annotations=["@key"],
                )
            ],
        )

        generator = MigrationGenerator.__new__(MigrationGenerator)
        operations = generator._introspected_to_operations(current_schema, model_info)
        add_ownership = next(op for op in operations if isinstance(op, ops.AddOwnership))
        rendered = generator._render_operations([add_ownership])
    finally:
        sys.path.remove(str(tmp_path))
        ModelRegistry.clear()
        for module_name in list(sys.modules):
            if module_name == "generated_models" or module_name.startswith("generated_models."):
                del sys.modules[module_name]

    assert add_ownership.attribute.get_attribute_name() == "email"
    assert add_ownership.optional is True
    assert "ops.AddOwnership(Customer, Email, optional=True)" in rendered


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


class SelfDiffName(String):
    flags = AttributeFlags(name="self-diff-name")


class SelfDiffPerson(Entity):
    flags = TypeFlags(name="self-diff-person")

    name: SelfDiffName = Flag(Key)


class SelfDiffEmployment(Relation):
    flags = TypeFlags(name="self-diff-employment")

    employee: Role[SelfDiffPerson] = Role("employee", SelfDiffPerson)


def test_schema_diff_self_is_empty() -> None:
    """SchemaInfo compared against itself after the registry-path collapse yields an empty diff."""
    schema = SchemaInfo()
    schema.entities.append(SelfDiffPerson)
    schema.relations.append(SelfDiffEmployment)

    diff = schema.compare(schema)

    assert not diff.has_changes()
