"""Live integration tests for the #158 manager-based run-python user scenario."""

import importlib
import json
import sys
from pathlib import Path
from typing import Any

import pytest

from type_bridge.migration import MigrationExecutor, MigrationGenerator, MigrationStateManager
from type_bridge.migration.executor import MigrationError
from type_bridge.session import Database

_PACKAGE_NAME = "scenario_app"
_MIGRATIONS_DIR_NAME = "scenario_migrations"
_TEAM_COUNT = 10
_USER_COUNT = 1000
_SEEDED_USER_EMAIL = "seeded@scenario-local.test"
_SEEDED_USER_NAME = "Seeded Scenario User"
_LOADED_USER_EMAIL_PREFIX = "user-"
_TEAM_SLUG_PREFIX = "team-"


def _scenario_package_source() -> str:
    return """\
from type_bridge import AttributeFlags, Entity, Flag, Key, Relation, Role, String, TypeFlags


class ScenarioUserEmail(String):
    flags = AttributeFlags(name="scenario158-user-email")


class ScenarioUserName(String):
    flags = AttributeFlags(name="scenario158-user-name")


class ScenarioTeamSlug(String):
    flags = AttributeFlags(name="scenario158-team-slug")


class ScenarioMembershipRole(String):
    flags = AttributeFlags(name="scenario158-membership-role")


class ScenarioUser(Entity):
    flags = TypeFlags(name="scenario158-user")
    email: ScenarioUserEmail = Flag(Key)
    name: ScenarioUserName


class ScenarioTeam(Entity):
    flags = TypeFlags(name="scenario158-team")
    slug: ScenarioTeamSlug = Flag(Key)


class ScenarioMembership(Relation):
    flags = TypeFlags(name="scenario158-membership")
    user: Role[ScenarioUser] = Role("member", ScenarioUser)
    team: Role[ScenarioTeam] = Role("team", ScenarioTeam)
    role: ScenarioMembershipRole
"""


def _build_teams_payload() -> str:
    lines: list[str] = []
    for index in range(_TEAM_COUNT):
        slug = f"{_TEAM_SLUG_PREFIX}{index:02d}"
        name = f"Scenario Team {index:02d}"
        lines.append("[[teams]]")
        lines.append(f'slug = "{slug}"')
        lines.append(f'name = "{name}"')
        lines.append("")
    return "\n".join(lines)


def _build_user_payload() -> list[dict[str, str]]:
    return [
        {
            "email": f"{_LOADED_USER_EMAIL_PREFIX}{index:04d}@scenario-local.test",
            "name": f"Loaded User {index:04d}",
            "team_slug": f"{_TEAM_SLUG_PREFIX}{index % _TEAM_COUNT:02d}",
            "role": "member",
        }
        for index in range(_USER_COUNT)
    ]


def _clear_scenario_modules() -> None:
    for module_name in list(sys.modules):
        if module_name == _PACKAGE_NAME or module_name.startswith(f"{_PACKAGE_NAME}."):
            del sys.modules[module_name]


def _write_scenario_package(tmp_path: Path) -> None:
    package_dir = tmp_path / _PACKAGE_NAME
    package_dir.mkdir(exist_ok=True)
    (package_dir / "__init__.py").write_text("")
    (package_dir / "models.py").write_text(_scenario_package_source())


def _write_data_resources(migrations_dir: Path) -> tuple[Path, Path]:
    resources_dir = migrations_dir / "data"
    resources_dir.mkdir(parents=True, exist_ok=True)

    teams_toml = resources_dir / "teams.toml"
    teams_toml.write_text(_build_teams_payload())

    users_json = resources_dir / "users.json"
    users_json.write_text(json.dumps(_build_user_payload(), indent=2))

    return teams_toml, users_json


def _load_models(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    _clear_scenario_modules()
    monkeypatch.syspath_prepend(str(tmp_path))
    return importlib.import_module(f"{_PACKAGE_NAME}.models")


def _migrations_dir(tmp_path: Path) -> Path:
    return tmp_path / _MIGRATIONS_DIR_NAME


def _ensure_initial_schema(
    clean_db: Database,
    models: Any,
    migrations_dir: Path,
) -> tuple[MigrationExecutor, str]:
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    initial_path = generator.generate(
        models=[models.ScenarioUser, models.ScenarioTeam, models.ScenarioMembership],
        name="initial",
    )
    assert initial_path is not None, "initial migration must be generated"

    result = executor.migrate()
    assert len(result) == 1
    assert result[0].success, f"initial migration failed: {result[0].error}"

    return executor, initial_path.stem


def _seed_user(clean_db: Database, models: Any) -> None:
    models.ScenarioUser.manager(clean_db).insert(
        models.ScenarioUser(
            email=models.ScenarioUserEmail(_SEEDED_USER_EMAIL),
            name=models.ScenarioUserName(_SEEDED_USER_NAME),
        )
    )


def _write_load_users_migration(migrations_dir: Path, dependency_name: str) -> None:
    migration_path = migrations_dir / "0002_load_users.py"

    migration_path.write_text(
        f"""\
from pathlib import Path
from typing import ClassVar

import json
import tomllib

from scenario_app.models import (
    ScenarioMembership,
    ScenarioMembershipRole,
    ScenarioTeam,
    ScenarioTeamSlug,
    ScenarioUser,
    ScenarioUserEmail,
    ScenarioUserName,
)
from type_bridge.migration import Migration
from type_bridge.migration import operations as ops
from type_bridge.migration.operations import Operation

_SEEDED_EMAIL = {_SEEDED_USER_EMAIL!r}
_LOADED_EMAIL_PREFIX = {_LOADED_USER_EMAIL_PREFIX!r}
_TEAM_SLUG_PREFIX = {_TEAM_SLUG_PREFIX!r}


def _load_teams(migration_path: Path) -> list[dict[str, str]]:
    path = migration_path / "data" / "teams.toml"
    return tomllib.loads(path.read_text())["teams"]


def _load_users(migration_path: Path) -> list[dict[str, str]]:
    with (migration_path / "data" / "users.json").open("r", encoding="utf-8") as fp:
        return json.load(fp)


def forwards(db: object) -> None:
    migration_dir = Path(__file__).parent
    teams = _load_teams(migration_dir)
    users = _load_users(migration_dir)

    seeded_users = ScenarioUser.manager(db).filter(email=_SEEDED_EMAIL).execute()
    if len(seeded_users) != 1:
        raise RuntimeError(
            "expected seeded user " + str(_SEEDED_EMAIL) + "; got " + str(len(seeded_users))
        )
    seeded = seeded_users[0]

    # Query while migrating through the public manager API.
    _ = ScenarioUser.manager(db).filter().limit(5).execute()
    _ = ScenarioUser.manager(db).filter().count()

    team_objects = [
        ScenarioTeam(
            slug=ScenarioTeamSlug(team_row["slug"]),
        )
        for team_row in teams
    ]
    ScenarioTeam.manager(db).insert_many(team_objects)
    team_by_slug = dict(
        (team_row["slug"], model)
        for team_row, model in zip(teams, team_objects)
    )

    user_objects = [
        ScenarioUser(
            email=ScenarioUserEmail(user["email"]),
            name=ScenarioUserName(user["name"]),
        )
        for user in users
    ]
    ScenarioUser.manager(db).insert_many(user_objects)

    memberships = [
        ScenarioMembership(
            user=user,
            team=team_by_slug[user_row["team_slug"]],
            role=ScenarioMembershipRole(user_row["role"]),
        )
        for user, user_row in zip(user_objects, users)
    ]
    memberships.append(
        ScenarioMembership(
            user=seeded,
            team=team_by_slug[teams[0]["slug"]],
            role=ScenarioMembershipRole("owner"),
        )
    )
    ScenarioMembership.manager(db).insert_many(memberships)

    _ = ScenarioMembership.manager(db).filter(user=seeded).execute()


def reverse(db: object) -> None:
    seeded = ScenarioUser.manager(db).filter(email=_SEEDED_EMAIL).first()
    if seeded is not None:
        ScenarioMembership.manager(db).filter(user=seeded).delete()

    loaded_users = ScenarioUser.manager(db).filter(email__startswith=_LOADED_EMAIL_PREFIX).execute()
    for loaded_user in loaded_users:
        ScenarioMembership.manager(db).filter(user=loaded_user).delete()
        ScenarioUser.manager(db).filter(email=loaded_user.email.value).delete()

    ScenarioTeam.manager(db).filter(slug__startswith=_TEAM_SLUG_PREFIX).delete()


class LoadUsersMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({migrations_dir.name!r}, {dependency_name!r}),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(
            forwards,
            reverse=reverse,
            description="load scenario users and memberships",
            resources=["data/teams.toml", "data/users.json"],
            import_checks=["json", "tomllib"],
        )
    ]
"""
    )


def _write_failure_migration(migrations_dir: Path, dependency_name: str) -> None:
    migration_path = migrations_dir / "0003_missing_resource.py"
    migration_path.write_text(
        f"""\
from typing import ClassVar

from scenario_app.models import ScenarioUser
from type_bridge.migration import Migration
from type_bridge.migration import operations as ops
from type_bridge.migration.operations import Operation


def forwards(db: object) -> None:
    raise RuntimeError("migration should not execute")


class MissingResourceMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({migrations_dir.name!r}, {dependency_name!r}),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(
            forwards,
            description="missing-resource python migration",
            resources=["data/missing_users.json"],
        )
    ]
"""
    )


def _write_irreversible_migration(migrations_dir: Path, dependency_name: str) -> None:
    migration_path = migrations_dir / "0004_irreversible.py"
    migration_path.write_text(
        f"""\
from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration import operations as ops
from type_bridge.migration.operations import Operation


def forwards(db: object) -> None:
    pass


class IrreversibleMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({migrations_dir.name!r}, {dependency_name!r}),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(
            forwards,
            description="irreversible python migration",
        )
    ]
"""
    )


def _typedb_rows(db: Database, query: str) -> list[dict[str, object]]:
    result = db.execute_query(query, transaction_type="read")
    return list(result) if result else []


def _count_rows(db: Database, query: str) -> int:
    return len(_typedb_rows(db, query))


@pytest.mark.integration
@pytest.mark.order(420)
def test_run_python_user_scenario_full_cycle(
    clean_db, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_scenario_package(tmp_path)
    models = _load_models(monkeypatch, tmp_path)
    migrations_dir = _migrations_dir(tmp_path)

    executor, initial_name = _ensure_initial_schema(clean_db, models, migrations_dir)

    _seed_user(clean_db, models)
    _write_data_resources(migrations_dir)
    _write_load_users_migration(migrations_dir, initial_name)

    # sqlmigrate preview shows RunPython comments and resources.
    preview = executor.sqlmigrate("0002_load_users")
    assert "RunPython: load scenario users and memberships" in preview
    assert "resources: data/teams.toml, data/users.json" in preview
    assert "import checks: json, tomllib" in preview
    assert "RunPython reverse: load scenario users and memberships" in executor.sqlmigrate(
        "0002_load_users",
        reverse=True,
    )

    results = executor.migrate()
    assert len(results) == 1
    assert results[0].name == "0002_load_users"
    assert results[0].success

    # Assertions from the manager API.
    assert len(models.ScenarioUser.manager(clean_db).all()) == 1 + _USER_COUNT
    assert len(models.ScenarioTeam.manager(clean_db).all()) == _TEAM_COUNT
    assert models.ScenarioMembership.manager(clean_db).count() == 1 + _USER_COUNT

    seeded_user = models.ScenarioUser.manager(clean_db).filter(email=_SEEDED_USER_EMAIL).first()
    assert seeded_user is not None
    assert models.ScenarioMembership.manager(clean_db).filter(user=seeded_user).count() == 1

    # Assertions from TypeDB query execution.
    assert (
        _count_rows(
            clean_db,
            """\
match
  $u isa scenario158-user,
    has scenario158-user-email $e;
fetch { "e": $e };
""",
        )
        == 1 + _USER_COUNT
    )
    assert (
        _count_rows(
            clean_db,
            """\
match
  $t isa scenario158-team,
    has scenario158-team-slug $slug;
fetch { "slug": $slug };
""",
        )
        == _TEAM_COUNT
    )
    assert (
        _count_rows(
            clean_db,
            """\
match
  $m isa scenario158-membership,
    has scenario158-membership-role $role;
fetch { "role": $role };
""",
        )
        == 1 + _USER_COUNT
    )

    state = MigrationStateManager(clean_db).load_state()
    assert state.is_applied(_MIGRATIONS_DIR_NAME, "0002_load_users")

    run_rows = MigrationStateManager(clean_db).load_runs()
    run_rows_for_load = [
        row
        for row in run_rows
        if row.app_label == _MIGRATIONS_DIR_NAME
        and row.name == "0002_load_users"
        and row.direction == "apply"
    ]
    succeeded_run_rows = [row for row in run_rows_for_load if row.status == "succeeded"]
    assert succeeded_run_rows
    run_row = succeeded_run_rows[-1]
    assert run_row.name == "0002_load_users"
    assert run_row.app_label == _MIGRATIONS_DIR_NAME
    assert run_row.direction == "apply"
    assert run_row.status == "succeeded"
    assert run_row.run_id
    assert run_row.checksum
    assert run_row.started_at
    assert run_row.finished_at
    assert run_row.error is None

    # Rollback restores the pre-migration state.
    rollback = executor.migrate(target=initial_name)
    assert len(rollback) == 1
    assert rollback[0].success
    assert rollback[0].action == "rolled_back"
    assert (
        not MigrationStateManager(clean_db)
        .load_state()
        .is_applied(
            _MIGRATIONS_DIR_NAME,
            "0002_load_users",
        )
    )

    assert len(models.ScenarioUser.manager(clean_db).all()) == 1
    assert len(models.ScenarioTeam.manager(clean_db).all()) == 0
    assert models.ScenarioMembership.manager(clean_db).count() == 0

    # Re-apply after rollback does not duplicate loaded entities.
    reapply = executor.migrate()
    assert len(reapply) == 1
    assert reapply[0].success
    assert len(models.ScenarioUser.manager(clean_db).all()) == 1 + _USER_COUNT
    assert len(models.ScenarioTeam.manager(clean_db).all()) == _TEAM_COUNT
    assert models.ScenarioMembership.manager(clean_db).count() == 1 + _USER_COUNT


@pytest.mark.integration
@pytest.mark.order(421)
def test_run_python_missing_resource_does_not_mutate(
    clean_db, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_scenario_package(tmp_path)
    models = _load_models(monkeypatch, tmp_path)
    migrations_dir = _migrations_dir(tmp_path)

    executor, initial_name = _ensure_initial_schema(clean_db, models, migrations_dir)
    _seed_user(clean_db, models)

    before_rows = _count_rows(
        clean_db,
        """\
match
  $u isa scenario158-user,
    has scenario158-user-email $e;
fetch { "e": $e };
""",
    )

    _write_failure_migration(migrations_dir, initial_name)

    with pytest.raises(MigrationError, match="missing"):
        executor.migrate()

    state = MigrationStateManager(clean_db).load_state()
    assert not state.is_applied(_MIGRATIONS_DIR_NAME, "0003_missing_resource")
    assert not any(
        row.name == "0003_missing_resource" for row in MigrationStateManager(clean_db).load_runs()
    )
    assert (
        _count_rows(
            clean_db,
            """\
match
  $u isa scenario158-user,
    has scenario158-user-email $e;
fetch { "e": $e };
""",
        )
        == before_rows
    )


@pytest.mark.integration
@pytest.mark.order(422)
def test_sqlmigrate_irreversible_preview(
    clean_db, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _write_scenario_package(tmp_path)
    models = _load_models(monkeypatch, tmp_path)
    migrations_dir = _migrations_dir(tmp_path)

    executor, initial_name = _ensure_initial_schema(clean_db, models, migrations_dir)
    _write_irreversible_migration(migrations_dir, initial_name)

    with pytest.raises(MigrationError, match="not reversible"):
        executor.sqlmigrate("0004_irreversible", reverse=True)
