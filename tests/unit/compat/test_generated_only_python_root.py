"""Public boundaries after removing the Python compatibility implementation."""

import ast
import subprocess
import sys
from importlib.util import find_spec
from pathlib import Path

import pytest

import type_bridge
import type_bridge.session as session


@pytest.mark.parametrize(
    "name",
    [
        "models",
        "crud",
        "fields",
        "attribute",
        "expressions",
        "query",
        "rules",
        "validation",
        "coercion",
        "reserved_words",
        "migration.introspection",
        "migration.exceptions",
        "migration.loader",
        "migration.sidecar",
        "migration.snapshots",
        "migration.state",
        "migration._archive_imports",
    ],
)
def test_legacy_modules_are_absent(name: str) -> None:
    assert find_spec(f"type_bridge.{name}") is None


@pytest.mark.parametrize(
    "name",
    [
        "Query",
        "QueryBuilder",
        "CrudEvent",
        "CrudHook",
        "HookCancelled",
        "EntityNotFoundError",
        "RelationNotFoundError",
        "NotUniqueError",
        "KeyAttributeError",
        "SchemaIntrospector",
    ],
)
def test_legacy_exports_are_absent(name: str) -> None:
    assert name not in type_bridge.__all__
    assert not hasattr(type_bridge, name)


def test_native_transaction_boundary() -> None:
    assert not hasattr(session, "Transaction")
    assert not hasattr(session, "ConnectionExecutor")
    assert not hasattr(type_bridge.TransactionContext, "manager")
    assert callable(type_bridge.Database.staged_transaction)
    assert callable(type_bridge.Database.execute_query)


def test_toml_schema_input_is_absent() -> None:
    import type_bridge_core

    assert not hasattr(type_bridge_core, "toml_to_typeql")


@pytest.mark.parametrize("name", ["MigrationLoader", "SchemaIntrospector", "generate_sidecars"])
def test_migration_recovery_exports_are_absent(name: str) -> None:
    import type_bridge.migration as migration

    assert name not in migration.__all__
    assert not hasattr(migration, name)


def test_cli_import_does_not_project_migration_state() -> None:
    subprocess.run(
        [
            sys.executable,
            "-c",
            "import sys; import type_bridge.migration; "
            "assert 'type_bridge.migration.state_schema' not in sys.modules",
        ],
        check=True,
    )


def test_generated_api_reference_has_an_exact_public_barrel_allowlist() -> None:
    source = (Path(__file__).resolve().parents[3] / "scripts/gen_ref_pages.py").read_text()
    assignment = next(
        node
        for node in ast.parse(source).body
        if isinstance(node, ast.Assign)
        and any(
            isinstance(target, ast.Name) and target.id == "PUBLIC_MODULES"
            for target in node.targets
        )
    )
    assert ast.literal_eval(assignment.value) == {
        "type_bridge",
        "type_bridge.migration",
        "type_bridge.proxy",
        "type_bridge.session",
        "type_bridge.typedb_driver",
    }


@pytest.mark.parametrize(
    "name",
    [
        "DynamicSort",
        "DynamicSortDir",
        "_QueryDynamicEntityManager",
        "_QueryDynamicRelationManager",
        "_query_build_has_lookup_query",
        "_QueryCrudQueryBuilder",
        "_generated_declared_descriptors_json",
    ],
)
def test_unused_native_compatibility_exports_are_absent(name: str) -> None:
    import type_bridge_core

    assert not hasattr(type_bridge_core, name)
    assert not hasattr(type_bridge_core.type_bridge_core, name)

