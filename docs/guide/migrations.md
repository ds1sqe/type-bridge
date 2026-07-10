# Migrations

TypeBridge migrations are ordered Python files that apply schema and data
changes to a TypeDB database. They are intended for production workflows where
schema history must be replayable, data backfills must use the ORM, and applied
state must have one durable source of truth. Standalone TypeBridge stores that
state in TypeDB by default; embedding orchestrators can inject an externally
owned ledger instead.

## Workflow

For a generated model package, the normal loop is:

```bash
python -m type_bridge.generator schema.toml -o generated_models

python -m type_bridge.migration makemigrations \
  --models generated_models \
  --migrations-dir migrations \
  --name initial \
  --database app

python -m type_bridge.migration migrate \
  --migrations-dir migrations \
  --database app
```

Then edit the schema, regenerate models, make the next migration, and apply it:

```bash
python -m type_bridge.generator schema.toml -o generated_models

python -m type_bridge.migration makemigrations \
  --models generated_models \
  --migrations-dir migrations \
  --name add_full_name \
  --database app

python -m type_bridge.migration migrate \
  --migrations-dir migrations \
  --database app
```

`makemigrations` compares the current model package with the live TypeDB schema.
Apply each generated migration before generating the next one so the live schema
is the previous state for the next diff.

## Migration Files

Migration files live in a migrations directory and use the
`NNNN_name.py` naming pattern:

```text
migrations/
|-- 0001_initial.py
|-- 0001_initial.json
|-- 0002_add_full_name.py
|-- 0002_add_full_name.json
`-- snapshots/
    |-- __init__.py
    |-- v0001/
    `-- v0002/
```

A generated migration uses typed operations:

```python
from typing import ClassVar

from migrations.snapshots.v0001 import Name, User
from type_bridge.migration import Migration
from type_bridge.migration import operations as ops
from type_bridge.migration.operations import Operation


class InitialMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = [
        ops.AddAttribute(Name),
        ops.AddEntity(User),
        ops.AddOwnership(User, Name, key=True),
    ]
```

The `.json` sidecar beside a generated migration stores the typed Rust
`OperationSpec` used for fast portable schema execution. The `.py` file remains
readable and is also the checksum source. If the `.py` file and sidecar drift
apart, migration loading fails before any schema or data change runs.

For hand-authored Python data migrations, prefer an empty migration:

```bash
python -m type_bridge.migration makemigrations \
  --empty \
  --migrations-dir migrations \
  --name load_users \
  --database app
```

## Why Snapshots Matter

The active model package is the current application API. It is not migration
history.

That distinction matters when a migration is replayed from an empty database.
Suppose migration `0001` creates `User.name`, migration `0002` adds
`User.full_name`, and migration `0003` removes `User.name`. After `0003`, the
active generated package no longer has the old `User.name` field. If `0001` or
`0002` imported from the active package, a fresh replay would import the wrong
schema shape, or fail because a removed type or field is gone.

Snapshots solve that by making the import path carry schema time:

```python
from migrations.snapshots.v0001 import User as OldUser
from migrations.snapshots.v0002 import FullName, Name, User
```

`migrations.snapshots.v0001.User` is the `User` binding for the schema state
after `0001` was generated. `migrations.snapshots.v0002.User` is the `User`
binding for the schema state after `0002`. Both can be imported in one
migration, with aliases when symbols collide.

This gives migrations two important properties:

- Fresh replay works after active bindgen output changes.
- Data migrations can use the ORM against the schema state that exists at that
  point in the migration plan.

A snapshot does not bypass TypeDB schema rules. The imported class determines
how TypeBridge compiles ORM operations, but the live database must still have
the matching schema at that point. For example, use `OldUser` before removing
the old ownership, and use the newer `User` after adding the new ownership.

## Schema And Data In One Plan

Use generated schema operations for portable schema changes, and `RunPython`
for data movement that needs application logic or external files.

For an expand/backfill/contract rename, use separate migration states:

1. Expand the schema so both old and new fields can exist.
2. Backfill data with `RunPython`.
3. Contract the schema by removing the old ownership or type.

Example backfill:

```python
from typing import ClassVar

from migrations.snapshots.v0001 import User as OldUser
from migrations.snapshots.v0002 import FullName, Name, User
from type_bridge.migration import Migration
from type_bridge.migration import operations as ops
from type_bridge.migration.operations import Operation


def forwards(db: object) -> None:
    for old_user in OldUser.manager(db).filter().execute():
        User.manager(db).update(
            User(
                name=Name(str(old_user.name)),
                full_name=FullName(str(old_user.name)),
            )
        )


class BackfillFullNameMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ("migrations", "0002_expand_user"),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(
            forwards,
            description="copy user name to full_name",
        ),
    ]
```

The `db` argument is the same database connection used by the migration
executor. Normal manager APIs work:

```python
users = User.manager(db).filter(full_name__startswith="A").execute()
count = User.manager(db).filter().count()
User.manager(db).insert_many(users_to_create)
User.manager(db).update(user)
User.manager(db).filter(email="old@example.com").delete()
```

## Loading JSON Or TOML During Migration

`RunPython` is useful for programmatic imports, fixture loading, and large data
creation. Declare files in `resources` so the executor validates that they exist
before running the migration body.

```python
from pathlib import Path
from typing import ClassVar

import json
import tomllib

from migrations.snapshots.v0001 import Team, TeamSlug, User, UserEmail
from type_bridge.migration import Migration
from type_bridge.migration import operations as ops
from type_bridge.migration.operations import Operation


def forwards(db: object) -> None:
    migration_dir = Path(__file__).parent
    teams = tomllib.loads((migration_dir / "data" / "teams.toml").read_text())["teams"]
    users = json.loads((migration_dir / "data" / "users.json").read_text())

    Team.manager(db).insert_many([
        Team(slug=TeamSlug(row["slug"]))
        for row in teams
    ])
    User.manager(db).insert_many([
        User(email=UserEmail(row["email"]))
        for row in users
    ])


class LoadUsersMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ("migrations", "0001_initial"),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunPython(
            forwards,
            description="load users and teams",
            resources=["data/teams.toml", "data/users.json"],
            import_checks=["json", "tomllib"],
        )
    ]
```

`sqlmigrate` previews Python operations as comments, including resources and
import checks:

```bash
python -m type_bridge.migration sqlmigrate 0002_load_users \
  --migrations-dir migrations \
  --database app
```

## Rollback

Schema operations are reversible only when they can produce rollback TypeQL.
Destructive operations such as removing a type are generally not reversible
because deleted data cannot be reconstructed.

For Python data migrations, pass a reverse callable:

```python
def reverse(db: object) -> None:
    User.manager(db).filter(email__startswith="seed-").delete()


operations = [
    ops.RunPython(
        forwards,
        reverse=reverse,
        description="seed users",
    )
]
```

Rollback to a target migration with:

```bash
python -m type_bridge.migration migrate 0001_initial \
  --migrations-dir migrations \
  --database app
```

If any operation in the rollback path is irreversible, the executor raises an
error before pretending the rollback succeeded.

## Migration State Backends

### Default TypeDB-backed state

By default, `MigrationExecutor` constructs a TypeDB-backed
`MigrationStateManager`. The target database is the source of truth used by
`migrate`, `showmigrations`, and checksum drift checks.

Two internal entity types are created automatically:

- `type_bridge_migration`: one row for each applied migration, including
  `app_label`, `name`, `applied_at`, and `checksum`.
- `type_bridge_migration_run`: one row for each execution attempt, including
  `run_id`, `app_label`, `name`, `checksum`, `direction`, `status`,
  `started_at`, `finished_at`, `error`, and executor metadata when available.

You can read this state through the Python facade:

```python
from type_bridge.migration import MigrationStateManager

state_manager = MigrationStateManager(db)
state = state_manager.load_state()
runs = state_manager.load_runs()

for record in state.applied:
    print(record.app_label, record.name, record.checksum)

for run in runs:
    print(run.run_id, run.name, run.direction, run.status, run.error)
```

Because the state is in TypeDB, a different process can inspect the same
migration history, and TypeDB Studio can view the internal migration entities
after the migration tracking schema has been ensured.

The migration CLI uses this default backend. Its `migrate` and
`showmigrations` commands do not accept an external state store; embedding
applications should use the programmatic executor API described below.

### External ledgers for embedding orchestrators

An orchestrator that already has an authoritative, project-scoped migration
ledger can inject an implementation of the `MigrationStateStore` protocol:

```python
from pathlib import Path

from type_bridge.migration import MigrationExecutor, MigrationStateStore

external_store: MigrationStateStore = project_migration_store
executor = MigrationExecutor(
    db,
    Path("migrations"),
    state_manager=external_store,
)
executor.migrate()
```

With an injected store, the executor reads and writes migration state through
that store and does not define TypeBridge migration-ledger types in the target
database. The target therefore contains only the application ontology and the
schema changes made by the migrations themselves.

The ownership boundary is deliberate: TypeBridge owns migration specs,
planning, lowering, checksums, and execution. The embedding orchestrator owns
applied-state persistence and any external run logging. Keep the default
TypeDB-backed manager for standalone TypeBridge workflows; inject a store only
when another system is the authoritative ledger.

### Excluding infrastructure from schema exports

`MIGRATION_STATE_SCHEMA` is the canonical public description of every schema
object owned by TypeBridge's default ledger. Consumers should use it through
the public helpers instead of copying a list of reserved labels:

```python
from type_bridge.migration import (
    MIGRATION_STATE_SCHEMA,
    is_migration_state_type,
    migration_state_schema,
    without_migration_state_schema,
)

# Filter a complete result from SchemaIntrospector before snapshotting it.
application_schema = without_migration_state_schema(introspected_schema)

# Or classify individual objects in another exporter.
if is_migration_state_type(kind="entity", label=label):
    continue

# Access value types, ownership annotations, and relation roles when needed.
full_state_schema = migration_state_schema()
```

Use `is_migration_state_type` with the object's actual kind (`entity`,
`relation`, `attribute`, or `role`). Exact kind-and-label matching avoids
discarding application types that merely resemble migration infrastructure.
`MIGRATION_STATE_SCHEMA` is the immutable kind-and-label projection used for
classification. Call `migration_state_schema()` for the full Rust `SchemaInfo`
contract, including value types, ownership annotations, and relation roles.

## Sidecars And Hand Editing

Generated migrations have a `.json` sidecar. Do not hand-edit a sidecar-backed
generated migration and expect the sidecar to remain valid. The loader compares
the sidecar checksum with the current `.py` checksum and fails on drift.

Use one of these patterns:

- For pure generated schema changes, leave the `.py` and `.json` pair together.
- For custom data movement, create an empty migration and write `RunPython`.
- For custom TypeQL, use `ops.RunTypeQL` in a hand-authored migration.

This keeps generated schema execution portable while keeping user-authored
Python migrations explicit.

## Commands

```bash
# Generate a migration from models.
python -m type_bridge.migration makemigrations \
  --models generated_models \
  --migrations-dir migrations \
  --name add_email \
  --database app

# Create an empty migration for hand-authored Python or TypeQL.
python -m type_bridge.migration makemigrations \
  --empty \
  --migrations-dir migrations \
  --name load_users \
  --database app

# Apply all pending migrations.
python -m type_bridge.migration migrate \
  --migrations-dir migrations \
  --database app

# Roll back to a target migration.
python -m type_bridge.migration migrate 0001_initial \
  --migrations-dir migrations \
  --database app

# Show applied and pending migrations.
python -m type_bridge.migration showmigrations \
  --migrations-dir migrations \
  --database app

# Preview forward or reverse TypeQL/comments.
python -m type_bridge.migration sqlmigrate 0002_load_users \
  --migrations-dir migrations \
  --database app
```
