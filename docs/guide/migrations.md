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

The Python `MigrationExecutor` integration remains migration-level. If its
result or the subsequent external state write is uncertain, record that
migration attempt as indeterminate and do not retry it automatically. Use the
Rust per-step boundary below when the embedder needs proven checkpoint resume.

#### Crash-safe per-step recovery (Rust embedders)

An external migration-level record is not atomic with TypeDB. TypeDB commits
each lowered step independently, so retrying a whole pending migration after a
timeout can replay steps that already committed. This is unsafe for arbitrary
`RunTypeQL` data writes.

Rust embedders should use the recovery boundary when they need resumable
execution:

1. Call `type_bridge_migration::plan_recovery(...)`. It performs the normal
   graph and checksum checks and returns a `CheckedExecutionPlan` whose entire
   ordered step sequence is available before mutation. Each step has a
   versioned `ExecutionStepId` derived from the artifact checksum, migration
   identity, direction, ordered index, and operation kind.
2. Persist the intended attempt and checked step IDs in the external ledger.
3. Implement `StepRecoveryController`. Its `classify` method must return:
   `Pending` only when non-commit is proven or a named idempotency strategy
   makes replay safe; `Applied` when durable evidence or TypeDB inspection
   proves the step committed; and `Indeterminate` otherwise.
4. Call `execute_recovery_plan(...)`. The executor skips `Applied`, executes
   only explicitly `Pending`, and stops immediately on `Indeterminate`.

The controller receives the target `Database` and complete checked step, so it
can inspect schema operations or run an operation-specific reconciliation
query. `PendingProof::IdempotentReplay` names the caller-owned contract used to
retry an idempotent backfill or data operation. TypeBridge does not assume raw
`RunTypeQL` is idempotent.

The executor emits typed `BeforeCommit`, `Committed`,
`FailedBeforeCommit`, and `UnknownCommitOutcome` events. A commit error is
always returned as an indeterminate outcome because a lost response does not
prove that TypeDB rolled back. Similarly, failure to durably record the
`Committed` event stops execution as indeterminate.

These callbacks provide a recovery protocol, not a distributed transaction.
A process can still disappear after the TypeDB commit and before the external
ledger records `Committed`. On restart, a durable `BeforeCommit` without a
provable outcome must remain indeterminate until an operator or an
operation-specific reconciler supplies evidence. TypeBridge's reserved
migration-state schema is not created or required by this API.

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

## Offline Authoring (No Database Connection)

`author_migration` authors the complete checked artifact set — the reviewable
`.py`, the executable `.json` sidecar, and the immutable `snapshots/vNNNN/`
files — from two serialized `SchemaInfo` dictionaries. It needs no TypeDB
connection, no `Database`, no model registry, and no generated application
package. The live `makemigrations` flow delegates to the same Rust core, so
live and offline authoring produce identical artifacts for identical inputs.

The intended embedder flow pairs offline authoring with an external migration
ledger (see Migration State Backends): a remote orchestrator owns the
authoritative applied-migration state and the application schema, and the
authoring process runs where TypeDB is unreachable.

```python
from type_bridge.migration import author_migration

# 1. Retrieve the authoritative base schema from the remote service as a
#    serialized SchemaInfo (the state-schema boundary from v1.5.6).
base = remote_service.fetch_application_schema()

# 2. Compile the local declaration into a target SchemaInfo.
target = local_schema_info.to_rust_schema_info()

# 3. Author the next migration entirely offline.
authored = author_migration(
    base,
    target,
    app_label="migrations",
    name="0003_add_assignment",
    dependencies=[("migrations", "0002_add_team")],
    snapshot_version="v0003",
    previous_snapshot_version="v0002",
    before_schema=[
        {
            "kind": "run_typeql",
            "forward": "match $x isa legacy-link; delete $x;",
        }
    ],
)

if authored is not None:
    # Inspect in memory: authored.python_source, authored.spec,
    # authored.files. Add embedder bytes before manifest computation, then
    # publish the whole version through one upstream commit point.
    authored.add_extension(
        "my-embedder",
        "companion.json",
        b'{"semantic_revision":3}',
        critical=True,
    )
    review_set = authored.composed_files
    authored.publish_to(migrations_dir)
```

Notes:

- `author_migration` returns `None` when the diff is empty and no explicit
  operations or declared semantic transition were supplied.
- Set `declared_intent` to deterministic caller-owned bytes only when an
  otherwise empty diff represents a real semantic version transition.
  TypeBridge computes the `tb-declared-transition-v1` identity and authors the
  canonical zero-operation Python/JSON/snapshot set. It executes no dummy
  TypeQL. Omit the declaration for a true no-op.
- `before_schema`/`after_schema` place explicit portable operations around
  the generated schema change set (destructive cleanup before removals,
  backfills after additions). `run_typeql` carries forward/reverse TypeQL
  verbatim; `copy_attribute` takes the structured form below and authoring
  synthesizes its backfill TypeQL and renders a faithful
  `ops.CopyAttribute(...)`:

  ```python
  after_schema=[
      {
          "kind": "copy_attribute",
          "owner": "person",
          "source": "legacy-name",
          "dest": "display-name",
          # Optional extra match constraint; the terminating ';' is added.
          "filter": "$x has age $a",
      }
  ]
  ```
- `attribute_renames=[("legacy-name", "display-name")]` declares that an
  attribute present in `base` continues as a new name in `target`. Without
  the directive that diff maps to an independent remove+add, which destroys
  the data; with it, authoring emits a staged, data-preserving expansion
  (define the new attribute, plain ownerships, backfill, annotation
  tightening, old-value cleanup, removal). The old-value cleanup step is
  irreversible. The non-executable `ops.RenameAttribute` placeholder is not
  accepted here — pass the directive instead.
- Pass a fixed `generated_at` to make the output byte-deterministic;
  otherwise the current time is stamped.
- `publish_to` writes canonical and extension files under no-clobber names and
  publishes immutable `NNNN_name.manifest.json` last. The checked loader sees
  either the previous chain or the complete hash-verified version. A one-time
  immutable tree sentinel records the exact legacy prefix.
- Extension paths are rooted below
  `ext/<namespace>/<migration-name>/`; traversal, case collisions, reserved
  names, and hash drift fail closed. Unknown critical namespaces require an
  embedder-aware checked loader.
- `write_to` remains only for explicitly legacy unmanifested output. New
  integrations and `makemigrations` use `publish_to`.
- A schema change the canonical mapper cannot express raises an explicit
  unsupported-change error instead of silently dropping the change.

The same core is available to Rust embedders as
`type_bridge_migration::author::author_migration`, which returns the artifact
files in memory alongside `write_authored_migration` for validated
persistence. New Rust integrations use `MigrationComposer`,
`publish_composed_migration`, and `load_dir_checked_with_extensions`; explicit
`collect_migration_orphans` cleanup holds the exclusive tree lock and never
runs in the background. Concurrent authoring requires a local filesystem with
reliable advisory locks.

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
