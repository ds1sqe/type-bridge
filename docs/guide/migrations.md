# V2 migrations

TypeBridge V2 migrations move a declared Split-YAML schema through an ordered,
binding-neutral history. Generated Python, TypeScript, and Rust packages are
projections of the resulting schema; they are not migration authority.

## Configure the workspace

```yaml
format: typebridge.workspace/v1

schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: application

migrations:
  directory: migrations/v2
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
`migration apply`, `migration verify`, and `migration adopt` require both an
exact `typedb-3.12.1/v1` workspace semantic profile and a negotiated TypeDB
3.12.1 server. Generated applications and offline authoring retain the wider
3.11–3.12 support window.

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
`NAME__tbv2_journal`. Reserve that suffix. Back up, restore, clone, and delete
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
database can replay the committed V2 chain without importing the current
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

## Adopt a frozen V1 history

Existing root Python/JSON migration histories are recovery inputs, not active
writers. Quiesce the old migrator and revoke its write authority before the
one-way cutover:

```bash
type-bridge --manifest typebridge.yaml migration adopt \
  --environment production \
  --archive-directory path/to/frozen-history
```

The adopter requires bounded regular files and verifies dependencies, original
source/sidecar checksums, snapshots, metadata, and the applied ledger. It
reconstructs the independently verified head, compares it with the live managed
schema, and publishes an archive-frontier genesis plus zero-operation bridge.
It never executes archived `RunPython` callbacks.

Adoption is idempotent for the same exact authority and rejects drift or a
competing canonical publisher. After it succeeds, continue only with V2
`make/plan/apply/verify` commands.

The retained Python loader, checksum, snapshot metadata, state reader, and
sidecar converter are read-only recovery components. They cannot author a new
root history, update an applied ledger, or write a historical snapshot.

## Historical TOML

TOML desired-schema authoring is not a migration path. Convert an immutable
historical TOML document with `type_bridge_core.toml_to_typeql`, review its
meaning, express the target in Split-YAML, and use the V2 flow above. See
[TOML recovery](toml.md).

## CI acceptance

For every migration change, test:

- offline check/make/plan determinism;
- apply and verify against a clean database;
- interruption/retry and lease behavior;
- destructive approval rejection and acceptance;
- full replay from empty;
- archive adoption followed by a new V2 migration when recovery changes;
- generated package regeneration and application operation parity.

See [schema workflows](schema-workflows.md), [schema commands](schema.md), and
the [Split-YAML reference](split-yaml-v1.md).
