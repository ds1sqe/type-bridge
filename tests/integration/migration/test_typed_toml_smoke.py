from __future__ import annotations

import sys
from pathlib import Path

import pytest

from type_bridge.generator import generate_models
from type_bridge.migration import MigrationExecutor, MigrationGenerator, ModelRegistry
from type_bridge.migration.executor import MigrationError

_SCHEMA_V1 = """
[attributes.smoke152-account-id]
value = "string"

[attributes.smoke152-display-name]
value = "string"

[entities.smoke152-account]
owns = [
    { attribute = "smoke152-account-id", key = true },
    "smoke152-display-name",
]
""".lstrip()


_SCHEMA_V2 = """
[attributes.smoke152-account-id]
value = "string"

[attributes.smoke152-display-name]
value = "string"

[attributes.smoke152-email]
value = "string"
regex = "^[^@]+@[^@]+\\\\.[^@]+$"

[entities.smoke152-account]
owns = [
    { attribute = "smoke152-account-id", key = true },
    "smoke152-display-name",
    { attribute = "smoke152-email", card = "0..1" },
]
""".lstrip()


def _clear_generated_modules(package_name: str) -> None:
    for module_name in list(sys.modules):
        if module_name == package_name or module_name.startswith(f"{package_name}."):
            del sys.modules[module_name]


def _generate_and_discover_models(
    tmp_path: Path,
    schema_text: str,
    *,
    package_name: str = "generated_models",
) -> list[type]:
    schema_path = tmp_path / "schema.toml"
    schema_path.write_text(schema_text)
    package_dir = tmp_path / package_name
    generate_models(schema_path, package_dir)
    _clear_generated_modules(package_name)
    return ModelRegistry.discover(package_name, register=False)


def _write_data_migration(migrations_dir: Path, name: str, dependency: str, typeql: str) -> Path:
    migration_name = f"0003_{name}" if name == "seed_email" else f"0004_{name}"
    class_name = "".join(part.capitalize() for part in name.split("_")) + "Migration"
    py_path = migrations_dir / f"{migration_name}.py"
    py_path.write_text(
        f"""\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


class {class_name}(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({migrations_dir.name!r}, {dependency!r}),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL(forward={typeql!r}),
    ]
"""
    )
    return py_path


def _fetch_email_rows(clean_db) -> list[dict]:
    return clean_db.execute_query(
        """
match
  $a isa smoke152-account,
    has smoke152-account-id $id,
    has smoke152-display-name $name,
    has smoke152-email $email;
fetch { "id": $id, "name": $name, "email": $email };
""",
        transaction_type="read",
    )


def _fetch_account_rows(clean_db) -> list[dict]:
    return clean_db.execute_query(
        """
match
  $a isa smoke152-account,
    has smoke152-account-id $id,
    has smoke152-display-name $name;
fetch { "id": $id, "name": $name };
""",
        transaction_type="read",
    )


def _field_value(row: dict, key: str) -> object:
    value = row[key]
    if isinstance(value, dict) and "value" in value:
        return value["value"]
    return value


@pytest.mark.integration
@pytest.mark.order(412)
def test_toml_typed_migration_smoke_with_data_script_and_failure(clean_db, tmp_path: Path):
    migrations_dir = tmp_path / "migrations"
    executor = MigrationExecutor(clean_db, migrations_dir)

    sys.path.insert(0, str(tmp_path))
    try:
        # Step 1: schema.toml v1 -> bindgen -> typed initial migration.
        models_v1 = _generate_and_discover_models(tmp_path, _SCHEMA_V1)
        initial_path = MigrationGenerator(clean_db, migrations_dir).generate(
            models=models_v1,
            name="initial",
        )
        assert initial_path is not None
        initial_source = initial_path.read_text()
        assert "ops.AddAttribute(" in initial_source
        assert "ops.AddEntity(" in initial_source
        assert "ops.RunTypeQL" not in initial_source

        results = executor.migrate()
        assert [result.name for result in results] == ["0001_initial"]
        assert all(result.success for result in results)

        # Step 2: real data before the delta migration.
        clean_db.execute_query(
            """
insert $a isa smoke152-account,
  has smoke152-account-id "acct-001",
  has smoke152-display-name "Northwind Support";
""",
            transaction_type="write",
        )
        account_rows = _fetch_account_rows(clean_db)
        assert len(account_rows) == 1
        assert _field_value(account_rows[0], "id") == "acct-001"
        assert _field_value(account_rows[0], "name") == "Northwind Support"

        # Step 3: schema.toml v2 -> bindgen -> typed delta migration.
        models_v2 = _generate_and_discover_models(tmp_path, _SCHEMA_V2)
        add_email_path = MigrationGenerator(clean_db, migrations_dir).generate(
            models=models_v2,
            name="add_email",
        )
        assert add_email_path is not None
        add_email_source = add_email_path.read_text()
        assert "ops.AddAttribute(" in add_email_source
        assert "ops.AddOwnership(" in add_email_source
        assert "optional=True" in add_email_source
        assert "ops.RunTypeQL" not in add_email_source

        preview = executor.sqlmigrate("0002_add_email")
        assert "smoke152-account owns smoke152-email @card(0..1);" in preview

        results = executor.migrate()
        assert [result.name for result in results] == ["0002_add_email"]
        assert all(result.success for result in results)
        assert _fetch_email_rows(clean_db) == []

        # Step 4: explicit data migration script using RunTypeQL. This must run
        # as a write transaction, not a schema transaction.
        seed_typeql = """
match $a isa smoke152-account, has smoke152-account-id "acct-001";
insert $a has smoke152-email "ops@example.com";
""".strip()
        _write_data_migration(migrations_dir, "seed_email", "0002_add_email", seed_typeql)
        data_preview = executor.sqlmigrate("0003_seed_email")
        assert data_preview == seed_typeql

        results = executor.migrate()
        assert [result.name for result in results] == ["0003_seed_email"]
        assert all(result.success for result in results)

        rows = _fetch_email_rows(clean_db)
        assert len(rows) == 1
        assert _field_value(rows[0], "id") == "acct-001"
        assert _field_value(rows[0], "name") == "Northwind Support"
        assert _field_value(rows[0], "email") == "ops@example.com"

        # Step 5: fail path. A bad data migration should fail in Rust execution,
        # remain unapplied, and leave the previously inserted data intact.
        _write_data_migration(
            migrations_dir,
            "bad_data",
            "0003_seed_email",
            "insert $ghost isa smoke152-missing-account;",
        )

        with pytest.raises(MigrationError, match="Migration failed"):
            executor.migrate()

        statuses = dict(executor.showmigrations())
        assert statuses["0001_initial"] is True
        assert statuses["0002_add_email"] is True
        assert statuses["0003_seed_email"] is True
        assert statuses["0004_bad_data"] is False
        assert _fetch_email_rows(clean_db) == rows
    finally:
        sys.path.remove(str(tmp_path))
        ModelRegistry.clear()
        _clear_generated_modules("generated_models")
