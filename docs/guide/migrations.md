# Migrations

TypeBridge migrations move a declared Split-YAML schema through an ordered,
binding-neutral history. Generated Python, TypeScript, and Rust packages are
projections of the resulting schema; they are not migration authority.

## Configure the workspace

History defaults to `migrations/` beside the workspace manifest. Set
`migrations.directory` explicitly to use another confined directory.

```yaml
format: typebridge.workspace/v1

schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: application

migrations:
  directory: migrations
  app-label: application
  destructive: require-approval

bindings:
  python:
    output: generated/python/app_models

environments:
  development:
    database: application
    uri: localhost:1729
    tls: 'false'
    migrate: 'true'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
```

`migration make` and `migration plan` are offline. Connected commands resolve
one named environment and enforce its `migrate` policy. TypeDB-backed
`migration apply`, `migration verify`, `migration rollback --execute`,
and `migration upgrade-profile` run only on these
profile/server pairs: workspace semantic profile `typedb-3.12.1/v1` with a
negotiated TypeDB 3.12.3+ server, or `typedb-3.13.6/v1` with TypeDB 3.13.6+ or
3.14.0+ and newer lines. Here, `+` admits later patches on the same minor
line. Newer lines reuse the newest measured profile and protocol assumptions
and warn once per connection that they are
"not tested or supported by this TypeBridge release". No compatibility is
guaranteed. Older profiles still reject other server lines. General version
admission requires >=3.12.0; supported lines remain the measured 3.12–3.14.
Offline authoring remains available for supported semantic profiles.

## Author and apply a change

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
type-bridge --manifest typebridge.yaml migration make --name add-person
type-bridge --manifest typebridge.yaml migration plan
type-bridge --manifest typebridge.yaml migration apply --environment development
type-bridge --manifest typebridge.yaml migration verify --environment development
```

Review the committed migration and preview before applying. When the workspace
requires approval for destructive changes, approve the exact compound migration
identity:

```bash
type-bridge --manifest typebridge.yaml migration apply \
  --environment production \
  --approve application/0002_remove-old-field
```

Generation is deliberately separate from migration application and never
contacts TypeDB. It emits every configured binding from one captured workspace,
and each package privately embeds the verified authority needed by its managers
and query sessions. Ordinary applications need no standalone authority JSON;
generated packages remain projections rather than migration or authoring
authority.

## State authority

For managed database `NAME`, TypeBridge owns a companion journal database named
`NAME__tb_journal`. Reserve that suffix. Back up, restore, clone, and delete
the managed database and journal together.

The journal is bootstrapped only when its exact owner, control schema, and
managed scope can be established. A partial/foreign journal, wrong owner,
missing manifest, checksum drift, or schema-fingerprint mismatch fails closed.
`migration verify` is read-only and never creates a database.

The committed history, declared-schema fingerprint, journal head, and live
managed schema form one state triad. Apply advances them under a lease and
execution journal so interruption can be diagnosed and retried without
silently skipping a step.

## Safety and replay

Each migration has a stable identity, exact parents, checksum, normalized
operations, safety classification, and resulting declared schema. A clean
database can replay the committed migration chain without importing the current
application package.

Use expand/backfill/contract as separate migrations when data must move:

1. expand the schema so old and new facts coexist;
2. backfill through a reviewed binding-neutral operation or application job
   bound to the intermediate generated projection;
3. verify the backfill;
4. contract the old schema in a separately approved migration.

Do not place target-language callbacks or model declarations in new migration
authority. Application data jobs use generated bindings for the exact
intermediate schema and are coordinated explicitly around migration apply.

## Upgrade an archived history

Archive and sidecar adoption are removed in 3.0. Adopt frozen Python/JSON
histories with TypeBridge 2.3.x before upgrading. Existing canonical adopted
histories continue through `migration make`, `plan`, `apply`, and `verify`;
their permanent cutover binding remains checked against the database.

## Move a history to a new TypeDB line

A migration history is bound to the semantic profile it was authored on. A
`typedb-3.13.6/v1` history moves between TypeDB 3.13.6 and 3.14.0 without a
bridge, since both lines share that profile. To continue a `typedb-3.12.1/v1`
history on TypeDB 3.13.6 or 3.14.0, record one profile
bridge per history:

1. Quiesce writers and move the managed database and its migration journal
   (`<database>__tb_journal`) to the 3.13.6 server with TypeDB's own
   `database export` and `database import`.
2. Set the workspace profile in `typebridge.yaml`:

   ```yaml
   compatibility:
     semantic-profile: typedb-3.13.6/v1
   ```

3. Point the environment at the new server and run, once per environment:

   ```bash
   type-bridge --manifest typebridge.yaml migration upgrade-profile \
     --environment production --to typedb-3.13.6/v1
   ```

4. Commit the new bridge manifest and continue with `migration make` and
   `migration apply` as usual.

Until the bridge exists, `make`, `plan`, `apply`, and `verify` refuse the
older history with `migration_history_profile_upgrade_required` and print the
command above. `--to` must equal the workspace profile, be a supported
migration profile, and be newer than the history's profile; anything else is
refused before credentials are read or files are written.

The command writes a zero-operation bridge manifest named like `migration make`
names migrations (`0004_profile_upgrade_typedb_3_13_6.tbmigration.json`;
`--name` replaces the descriptive part). Then it connects and records the
bridge in the ledger. Recording requires the ledger to stand at the history
head and the live schema to equal that head's schema; out-of-band changes are
refused with `migration_upgrade_profile_live_drift`. A second run reports the
bridge as already recorded. Once the committed bridge exists, other moved
environments can also catch up with an ordinary `migration apply`.

New environments on the newer server need no export. `migration apply` into
an empty database on 3.13.6 replays the whole bridged history: the 3.12.1
migrations before the bridge are lowered and verified under their own
profile, the bridge switches the recorded state to `typedb-3.13.6/v1`, and the
later migrations apply as usual. Every step is checked as in any other apply,
and the ledger records each migration. Older-profile migrations replay only as
ancestors of a bridge to the server's profile; without such a bridge they are
refused.

The bridge is one-way. A history crosses at most one bridge, and rollback
refuses the bridge and every migration before it
(`migration_rollback_profile_bridge`); returning to 3.12 needs a database
restored on the old server.

## Historical TOML

TOML conversion is removed in 3.0. Convert historical TOML with TypeBridge
2.3.x before upgrading, review its meaning, then express the target in
Split-YAML and use the canonical workspace migration flow above.

## CI acceptance

For every migration change, test:

- offline check/make/plan determinism;
- apply and verify against a clean database;
- interruption/retry and lease behavior;
- destructive approval rejection and acceptance;
- full replay from empty;
- existing adopted histories retain their cutover binding during migration;
- generated package regeneration and application operation parity.

See [schema workflows](schema-workflows.md), [schema commands](schema.md), and
the [Split-YAML reference](split-yaml-v1.md).

## Declarative data backfills

Use the canonical workspace migration engine for data backfills. The legacy
Rust migration planner and executor are removed in 3.0.

Canonical copy-attribute authoring uses format 2 and an explicit TypeQL mode:

```yaml
format: typebridge.migration-backfill-intent/v2
copy-attribute:
  mode: isa
  owner-kind: entity
  owner: person
  source: legacy-name
  destination: display-name
  partition-key: person-id
  batch-rows: 128
  reverse: remove-equal-copied-destination
```

Use `mode: isa!` to match only the exact concrete attribute types. `mode: isa`
includes attribute subtypes and supports abstract source labels. Reverse
execution retains the forward plan's mode and removes only destination
ownerships whose values equal their source values.

Format-1 backfill records keep their original bytes, fingerprints, and historical
`isa` behavior for applied-history validation, recovery, and rollback. New
YAML intents and manifests must use format 2. An unapplied format-1 backfill can
execute only when it matches an existing persisted open plan from an interrupted
execution; otherwise regenerate it with an explicit mode. The refusal is
`migration_backfill_historical_execution_requires_open_plan` and occurs before
saving a new plan or mutating provider data.

## Atomic data steps

Use a data step for a canonical query plan, raw TypeQL, or an SDK callback.
Each forward or reverse program runs in one engine-owned managed write
transaction. The engine commits its completion marker in that same transaction;
a retry uses that marker to repair the journal without running committed work
again. Reversing the step deletes its marker atomically, so a later apply can
run forward again. One step is one transaction: split large derivations into
several steps to stay within TypeDB transaction limits. There is no batching API.

Put an intent beside the migration history, for example `seed.data.yaml`:

```yaml
format: typebridge.migration-data-intent/v1
id: seed-person
forward:
  kind: typeql
  text: 'insert $p isa person, has name "Ada";'
reverse:
  kind: typeql
  text: 'match $p isa person, has name "Ada"; delete $p;'
```

Review the reverse against your application data: this example assumes that
name identifies the seeded person. Omit `reverse` when no safe inverse exists.
The preview then marks the step `IRREVERSIBLE`, and rollback refuses it before
writes.

Author with `type-bridge migration make --name seed --data-step seed.data.yaml`.
Repeat `--data-step` to include several intents in the given order. Data
migration authoring requires the compiled schema sources to equal the committed
history head; publish schema expansion first. Data work is classified as opaque
and uses the existing exact-manifest approval flow.

A plan program uses `kind: plan` and `file: forward.plan.json`. The file must
contain canonical query-plan v3 JSON with no invocation inputs, bound to the
historical schema. Intent and plan paths must be regular files directly inside
the configured migration directory. Forward and reverse may use different
program kinds. The manifest digest covers embedded plan bytes and exact TypeQL
text.

A callback program uses `kind: user` and `step-id: derive-gender`. Its manifest
covers the registration ID and whether a reverse is declared; it does **not**
cover the callback's source code. Keep callback code versioned with the
application. CLI apply cannot load user code and reports
`migration_user_step_unregistered`; execute such migrations through an SDK
catalog with the callback registered.


### Registered user data steps

Register code on the generated catalog **before creating the preview**. A
preview snapshots its registrations. Both directions of a user intent normally
name the same registration ID; the engine selects its forward or reverse
function. A missing callback or reverse is refused before writes. User callbacks
run only on direct connections; proxy execution is refused before writes.

Python callbacks are synchronous. Generated managers use the callback transaction:

```python
from app_models import Gender, Person, open_migration_catalog
from type_bridge.migration import MigrationTransaction


def derive_gender(tx: MigrationTransaction) -> None:
    people = Person.manager(tx)
    for person in people.all():
        # Example legacy encoding: the final digit encodes this stored category.
        person.gender = Gender("male" if int(person.ssn.value[-1]) % 2 else "female")
        people.update(person)


def clear_gender(tx: MigrationTransaction) -> None:
    tx.execute_query("match $p isa person, has gender $g; delete has $g of $p;")


catalog = open_migration_catalog()
catalog.register("derive-gender", derive_gender, clear_gender)
```

Use this reverse only when the migration owns all those gender values. A real
migration must preserve unrelated pre-existing data. Async Python typed-query
terminals can run with `asyncio.run()` inside the synchronous callback.

Node callbacks are async and migration execution is awaited:

```typescript
catalog.register("derive-gender", async tx => {
  const people = Person.manager(tx);
  for (const person of people.all()) {
    people.update(person.iid, Person.create({
      ssn: person.ssn,
      gender: Gender.create(Number(person.ssn.value.slice(-1)) % 2 ? "male" : "female"),
    }));
  }
}, async tx => {
  await tx.executeQuery("match $p isa person, has gender $g; delete has $g of $p;");
});
const report = await plan.execute(database, "deployment");
```

In Rust, a plain `async fn(MigrationTransaction<AppSchema>) -> type_bridge::Result<()>`
works with `catalog.register("derive-gender", forward)` or
`catalog.register_with_reverse("derive-gender", forward, reverse)`. Use
`tx.entities::<Person>()`, `tx.relations::<Relation>()`, `tx.query()` and
`tx.execute_query(text).await` on that transaction.

C uses `type_bridge_migration_catalog_register(catalog, step_id, forward,
reverse, context, &diagnostics)`. Its callback signature is:

```c
type_bridge_status_t forward(type_bridge_write_transaction_t *tx,
    void *context, type_bridge_execution_diagnostics_t **diagnostics);
```

The borrowed write handle supports generated managers and typed queries.
`type_bridge_migration_transaction_execute_query` returns the ordinary raw JSON
result handle. Return an error status, optionally transferring owned execution
diagnostics, to fail the callback. The context must be callable from a worker
thread and remain live until the catalog and all derived plans close. Close
those owners before closing the database; they retain borrowed handle storage.
C callbacks must not unwind. Cancellation invalidates the borrowed transaction;
the execute call waits for the C function to return before releasing its context.

All surfaces share the same engine transaction. The engine commits its data
and completion marker together, or rolls both back on a callback error. A
process failure after commit is recovered from the marker without calling the
callback again. Callbacks cannot commit, roll back or close the transaction;
escaped handles and managers become invalid after return or cancellation.
Keep external side effects out of callbacks: retries cannot roll those back.
One data step is one TypeDB transaction. Split large derivations into several
steps to stay within TypeDB transaction limits.
