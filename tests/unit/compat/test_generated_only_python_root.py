"""Public-root absence checks for the generated-only Python cutover."""

from __future__ import annotations

import ast
import importlib
from importlib.util import find_spec
from pathlib import Path

import type_bridge_core

import type_bridge
import type_bridge.attribute as attribute
import type_bridge.crud as crud
import type_bridge.fields as fields
import type_bridge.migration as migration
import type_bridge.models as models

ROOT = Path(__file__).parents[3]
PUBLIC_REFERENCE_MODULES = {
    "type_bridge",
    "type_bridge.crud",
    "type_bridge.expressions",
    "type_bridge.migration",
    "type_bridge.proxy",
    "type_bridge.query",
    "type_bridge.session",
    "type_bridge.typed",
    "type_bridge.typedb_driver",
}

REMOVED_ROOT_AUTHORING_NAMES = {
    "Attribute",
    "AttributeFlags",
    "Boolean",
    "BreakingChangeAnalyzer",
    "Card",
    "ChangeCategory",
    "Date",
    "DateTime",
    "DateTimeTZ",
    "Decimal",
    "Distinct",
    "Doc",
    "Double",
    "Duration",
    "Entity",
    "Flag",
    "Integer",
    "Key",
    "Meta",
    "Migration",
    "MigrationError",
    "MigrationExecutor",
    "MigrationManager",
    "ModelRegistry",
    "Ordered",
    "Relation",
    "Role",
    "RolePlayerChange",
    "SchemaInfo",
    "SchemaManager",
    "String",
    "TypeDBManager",
    "TypeDBType",
    "TypeFlags",
    "TypeNameCase",
    "Unique",
    "migration_ops",
}

REMOVED_ATTRIBUTE_MODULE_NAMES = {
    "type_bridge.attribute.base": {"Attribute"},
    "type_bridge.attribute.boolean": {"Boolean"},
    "type_bridge.attribute.date": {"Date"},
    "type_bridge.attribute.datetime": {"DateTime"},
    "type_bridge.attribute.datetimetz": {"DateTimeTZ"},
    "type_bridge.attribute.decimal": {"Decimal"},
    "type_bridge.attribute.double": {"Double"},
    "type_bridge.attribute.duration": {"Duration"},
    "type_bridge.attribute.integer": {"Integer"},
    "type_bridge.attribute.string": {"String"},
    "type_bridge.attribute.flags": {
        "AttributeFlags",
        "Card",
        "Distinct",
        "Doc",
        "Flag",
        "Key",
        "Meta",
        "Ordered",
        "TypeFlags",
        "TypeNameCase",
        "Unique",
    },
}

REMOVED_MODEL_MODULE_NAMES = {
    "type_bridge.models.base": {"TypeDBType"},
    "type_bridge.models.entity": {"Entity"},
    "type_bridge.models.relation": {"Relation"},
    "type_bridge.models.registry": {"ModelRegistry"},
    "type_bridge.models.role": {"Role"},
    "type_bridge.models.schema_scanner": {"SchemaScanner"},
    "type_bridge.models.utils": {
        "FieldInfo",
        "MatchClauseInfo",
        "ModelAttrInfo",
        "WriteQueryInfo",
    },
}

REMOVED_FIELD_MODULE_NAMES = {
    "type_bridge.fields.base": {
        "FieldDescriptor",
        "FieldRef",
        "NumericFieldRef",
        "OrderedFieldRef",
        "StringFieldRef",
    },
    "type_bridge.fields.role": {
        "RolePlayerFieldRef",
        "RolePlayerNumericFieldRef",
        "RolePlayerStringFieldRef",
        "RoleRef",
    },
}

REMOVED_MANAGER_MODULE_NAMES = {
    "type_bridge.crud.rust_manager": {"RustTypeDBManager"},
    "type_bridge.crud.strategies": {
        "EntityStrategy",
        "ModelStrategy",
        "RelationStrategy",
    },
    "type_bridge.crud.typedb_manager": {"TypeDBManager"},
}

REMOVED_MIGRATION_ROOT_NAMES = {
    "AttributeFlagChange",
    "AuthoredMigration",
    "BreakingChangeAnalyzer",
    "ChangeCategory",
    "ClassifiedChange",
    "CopyAttribute",
    "EntityChanges",
    "Migration",
    "MigrationDependency",
    "MigrationExecutor",
    "MigrationGenerator",
    "MigrationPlan",
    "MigrationResult",
    "MigrationStateStore",
    "ModelRegistry",
    "RelationChanges",
    "RoleCardinalityChange",
    "RolePlayerChange",
    "RunPython",
    "SchemaDiff",
    "SchemaInfo",
    "SchemaManager",
    "SimpleMigrationManager",
    "author_migration",
    "operations",
    "ref",
}

REMOVED_MIGRATION_MODULES = {
    "type_bridge.migration._generate",
    "type_bridge.migration._ir_dump",
    "type_bridge.migration.author",
    "type_bridge.migration.base",
    "type_bridge.migration.breaking",
    "type_bridge.migration.diff",
    "type_bridge.migration.executor",
    "type_bridge.migration.generator",
    "type_bridge.migration.info",
    "type_bridge.migration.registry",
    "type_bridge.migration.schema_manager",
    "type_bridge.migration.simple_migration",
}

REMOVED_NATIVE_AUTHORING_NAMES = {
    "AuthoredMigration",
    "CrudQueryBuilder",
    "PyDescriptorRegistry",
    "PyDynamicEntityManager",
    "PyDynamicRelationManager",
    "PyMigrationRunner",
    "PyMigrationStateManager",
    "TypeSchema",
    "author_migration",
    "build_has_lookup_query",
    "classify_schema_diff",
    "compute_schema_diff",
    "generate_define_block",
    "generated_declared_descriptors_json",
    "render_models_json",
    "migration_runner",
    "migration_state_manager",
    "run_legacy_migration_cli",
    "schema_diff_is_breaking",
}


def test_python_root_has_no_handwritten_authoring_exports() -> None:
    assert REMOVED_ROOT_AUTHORING_NAMES.isdisjoint(type_bridge.__all__)
    for name in REMOVED_ROOT_AUTHORING_NAMES:
        assert not hasattr(type_bridge, name), name


def test_crud_root_retains_queries_but_not_handwritten_manager() -> None:
    assert "TypeDBManager" not in crud.__all__
    assert not hasattr(crud, "TypeDBManager")
    assert {"TypeDBQuery", "GroupByQuery"} <= set(crud.__all__)


def test_handwritten_authoring_package_barrels_are_empty() -> None:
    assert attribute.__all__ == []
    assert models.__all__ == []
    assert fields.__all__ == []

    for module, names in (
        (attribute, {"Attribute", "String", "Integer", "Flag", "TypeFlags"}),
        (models, {"TypeDBType", "Entity", "Relation", "Role", "FieldInfo"}),
        (fields, {"FieldDescriptor", "FieldRef", "RoleRef"}),
    ):
        for name in names:
            assert not hasattr(module, name), f"{module.__name__}.{name}"


def test_handwritten_attribute_defining_module_identities_are_absent() -> None:
    for module_name, names in REMOVED_ATTRIBUTE_MODULE_NAMES.items():
        module = importlib.import_module(module_name)
        for name in names:
            assert not hasattr(module, name), f"{module_name}.{name}"


def test_handwritten_model_defining_module_identities_are_absent() -> None:
    for module_name, names in REMOVED_MODEL_MODULE_NAMES.items():
        module = importlib.import_module(module_name)
        for name in names:
            assert not hasattr(module, name), f"{module_name}.{name}"


def test_handwritten_field_defining_module_identities_are_absent() -> None:
    for module_name, names in REMOVED_FIELD_MODULE_NAMES.items():
        module = importlib.import_module(module_name)
        for name in names:
            assert not hasattr(module, name), f"{module_name}.{name}"


def test_handwritten_manager_defining_module_identities_are_absent() -> None:
    for module_name, names in REMOVED_MANAGER_MODULE_NAMES.items():
        module = importlib.import_module(module_name)
        for name in names:
            assert not hasattr(module, name), f"{module_name}.{name}"


def test_programmatic_generator_package_is_absent() -> None:
    assert find_spec("type_bridge.generator") is None


def test_migration_root_retains_recovery_but_not_authoring() -> None:
    assert REMOVED_MIGRATION_ROOT_NAMES.isdisjoint(migration.__all__)
    for name in REMOVED_MIGRATION_ROOT_NAMES:
        assert not hasattr(migration, name), name

    assert {
        "MigrationLoader",
        "MigrationState",
        "SchemaIntrospector",
        "generate_sidecars",
    } <= set(migration.__all__)


def test_migration_authoring_defining_identities_are_absent() -> None:
    assert find_spec("type_bridge.migration.operations") is None
    assert find_spec("type_bridge.migration.ref") is None
    for module_name in REMOVED_MIGRATION_MODULES:
        assert find_spec(module_name) is None, module_name


def test_snapshot_recovery_has_no_writer() -> None:
    snapshots = importlib.import_module("type_bridge.migration.snapshots")
    assert snapshots.__all__ == ["get_snapshot_metadata"]
    assert not hasattr(snapshots, "generate_snapshot")


def test_native_authoring_bindings_are_absent() -> None:
    native_extension = type_bridge_core.type_bridge_core
    for module in (type_bridge_core, native_extension):
        for name in REMOVED_NATIVE_AUTHORING_NAMES:
            assert not hasattr(module, name), f"{module.__name__}.{name}"

    assert hasattr(type_bridge_core, "run_v2_cli")
    assert hasattr(type_bridge_core, "PyMigrationStateReader")
    reader_type = type_bridge_core.PyMigrationStateReader
    for name in ("ensure_schema", "record_applied", "record_unapplied", "record_run"):
        assert not hasattr(reader_type, name), name


def test_transaction_context_has_no_handwritten_manager_factory() -> None:
    assert not hasattr(type_bridge.TransactionContext, "manager")


def test_raw_query_facade_remains_public() -> None:
    assert {"Query", "QueryBuilder"} <= set(type_bridge.__all__)
    assert type_bridge.Query().match("$person isa person").fetch("$person").build() == (
        'match\n$person isa person;\nfetch {\n  "person": $person.*\n};'
    )


def test_generated_api_reference_has_an_exact_public_barrel_allowlist() -> None:
    source = (ROOT / "scripts/gen_ref_pages.py").read_text(encoding="utf-8")
    module = ast.parse(source)
    assignment = next(
        node
        for node in module.body
        if isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "PUBLIC_MODULES"
            for target in node.targets
        )
    )
    assert isinstance(assignment.value, ast.Set)
    modules = {
        element.value for element in assignment.value.elts if isinstance(element, ast.Constant)
    }
    assert modules == PUBLIC_REFERENCE_MODULES
