"""Live integration tests for snapshot bindings and temporal migrations."""

import importlib
import sys
from pathlib import Path

import pytest

from type_bridge.migration import MigrationExecutor, MigrationGenerator
from type_bridge.migration.loader import MigrationLoadError
from type_bridge.session import Database

_PACKAGE_NAME = "integration_scenario_app"
_MIGRATIONS_DIR_NAME = "integration_scenario_migrations"


def _clear_modules() -> None:
    for module_name in list(sys.modules):
        if module_name == _PACKAGE_NAME or module_name.startswith(f"{_PACKAGE_NAME}."):
            del sys.modules[module_name]
        if module_name == _MIGRATIONS_DIR_NAME or module_name.startswith(
            f"{_MIGRATIONS_DIR_NAME}."
        ):
            del sys.modules[module_name]


@pytest.mark.integration
def test_temporal_data_migration_scenario(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    clean_db: Database,
) -> None:
    _clear_modules()
    monkeypatch.syspath_prepend(str(tmp_path))

    migrations_dir = tmp_path / _MIGRATIONS_DIR_NAME
    migrations_dir.mkdir()

    # Step 1: Pre-state models
    # User owns email (key) and name
    pre_source = """\
from type_bridge import AttributeFlags, Entity, Flag, Key, String, TypeFlags

class UserEmail(String):
    flags = AttributeFlags(name="email")

class UserName(String):
    flags = AttributeFlags(name="name")

class User(Entity):
    flags = TypeFlags(name="user")
    email: UserEmail = Flag(Key)
    name: UserName
"""
    app_dir = tmp_path / _PACKAGE_NAME
    app_dir.mkdir()
    (app_dir / "__init__.py").write_text("")
    (app_dir / "models.py").write_text(pre_source)

    # Generate and apply migration 1 (Pre-state)
    models_v1 = importlib.import_module(f"{_PACKAGE_NAME}.models")
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    initial_path = generator.generate(
        models=[models_v1.User],
        name="initial",
    )
    assert initial_path is not None
    result = executor.migrate()
    assert len(result) == 1
    assert result[0].success

    # Seed pre-state data
    models_v1.User.manager(clean_db).insert(
        models_v1.User(
            email=models_v1.UserEmail("alice@test.com"),
            name=models_v1.UserName("Alice Smith"),
        )
    )

    # Step 2: Transition state (User owns email, name, full_name, user_id)
    transition_source = """\
from type_bridge import AttributeFlags, Entity, Flag, Key, String, TypeFlags

class UserEmail(String):
    flags = AttributeFlags(name="email")

class UserName(String):
    flags = AttributeFlags(name="name")

class UserFullName(String):
    flags = AttributeFlags(name="full_name")

class UserId(String):
    flags = AttributeFlags(name="user_id")

class User(Entity):
    flags = TypeFlags(name="user")
    email: UserEmail = Flag(Key)
    name: UserName | None = None
    full_name: UserFullName | None = None
    user_id: UserId | None = None
"""
    _clear_modules()
    (app_dir / "models.py").write_text(transition_source)
    models_v2 = importlib.import_module(f"{_PACKAGE_NAME}.models")

    # Generate and apply migration 2 (Transition State)
    # The generator will write snapshot v0002 first, then render operations
    generator = MigrationGenerator(clean_db, migrations_dir)
    transition_path = generator.generate(
        models=[models_v2.User],
        name="transition",
    )
    assert transition_path is not None

    # Step 3: Inject custom Python backfill logic in migration 2
    # We want to edit migration 2 to run a Python data migration that reads from v0001.User and writes to v0002.User
    migration_2_content = transition_path.read_text()

    # We want to replace operations = [...] with a RunPython backfill
    snapshot_imports = f"""\
from {_MIGRATIONS_DIR_NAME}.snapshots.v0001 import User as OldUser
from {_MIGRATIONS_DIR_NAME}.snapshots.v0002 import Email, FullName, Name, User as NewUser, UserId
"""

    backfill_code = """
def backfill_data(db):
    for old_user in OldUser.manager(db).filter().execute():
        # Get matching NewUser instance to write the values
        NewUser.manager(db).update(
            NewUser(
                email=Email(str(old_user.email)),
                name=Name(str(old_user.name)),
                full_name=FullName(str(old_user.name)),
                user_id=UserId("ID-" + str(old_user.email)),
            )
        )

    # Delete the name attribute from all user instances so they can be contracted
    db.execute_query(
        "match $u isa user, has name $n; delete $n of $u;",
        transaction_type="write",
    )
"""
    # Find operations block in migration_2_content
    op_start = migration_2_content.find("    operations: ClassVar[list[Operation]] = [")
    op_end = migration_2_content.find("    ]", op_start) + 5
    original_ops_decl = migration_2_content[op_start:op_end]

    # Insert backfill_data function and add ops.RunPython to operations
    modified_content = migration_2_content.replace(
        "class TransitionMigration(Migration):",
        backfill_code + "\nclass TransitionMigration(Migration):\n",
    ).replace(
        original_ops_decl,
        original_ops_decl.replace("    ]", "        ops.RunPython(backfill_data),\n    ]"),
    )

    # Add RunPython import if not present
    modified_content = modified_content.replace(
        "from type_bridge.migration import operations as ops, ref",
        "from type_bridge.migration import operations as ops, ref\n" + snapshot_imports,
    )
    transition_path.write_text(modified_content)

    with pytest.raises(MigrationLoadError, match="sidecar drift"):
        MigrationExecutor(clean_db, migrations_dir).migrate()

    # This hand-authored Python runtime migration cannot use the stale generated
    # sidecar. Deleting it exercises the trusted Python migration path after the
    # drift gate above proves stale sidecars do not execute.
    sidecar_path = transition_path.with_suffix(".json")
    if sidecar_path.exists():
        sidecar_path.unlink()

    # Now apply the transition migration
    executor = MigrationExecutor(clean_db, migrations_dir)
    result = executor.migrate()
    assert len(result) == 1
    assert result[0].success

    # Verify that Alice Smith has user_id="ID-alice@test.com" and full_name="Alice Smith" in the database
    # Let's query using the transition model User
    users = models_v2.User.manager(clean_db).filter(email="alice@test.com").execute()
    assert len(users) == 1
    assert str(users[0].full_name) == "Alice Smith"
    assert str(users[0].user_id) == "ID-alice@test.com"

    # Step 4: Contract state (User owns email, full_name, user_id; name is removed)
    contract_source = """\
from type_bridge import AttributeFlags, Entity, Flag, Key, String, TypeFlags

class UserEmail(String):
    flags = AttributeFlags(name="email")

class UserFullName(String):
    flags = AttributeFlags(name="full_name")

class UserId(String):
    flags = AttributeFlags(name="user_id")

class User(Entity):
    flags = TypeFlags(name="user")
    email: UserEmail = Flag(Key)
    full_name: UserFullName
    user_id: UserId
"""
    _clear_modules()
    (app_dir / "models.py").write_text(contract_source)
    models_v3 = importlib.import_module(f"{_PACKAGE_NAME}.models")

    # Generate and apply migration 3 (Contract State)
    generator = MigrationGenerator(clean_db, migrations_dir)
    contract_path = generator.generate(
        models=[models_v3.User],
        name="contract",
    )
    assert contract_path is not None

    # Apply contract migration
    executor = MigrationExecutor(clean_db, migrations_dir)
    result = executor.migrate()
    assert len(result) == 1
    assert result[0].success

    # Step 5: Verify that querying old field fails
    # Active model User (v3) no longer owns name
    assert not hasattr(models_v3.User, "name")

    # Trying to query using old snapshot v0001 (which expects UserName to be owned by User)
    # should fail at the TypeDB schema level or because it doesn't exist
    from type_bridge.migration.snapshots import get_snapshot_class_map

    class_map_v1 = get_snapshot_class_map(migrations_dir, "v0001")
    OldUser = class_map_v1["user"]

    # This should fail because the live DB schema no longer has "name" owned by "user"
    with pytest.raises(Exception):
        OldUser.manager(clean_db).filter(name="Alice Smith").execute()
