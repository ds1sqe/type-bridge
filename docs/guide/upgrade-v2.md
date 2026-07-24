# Upgrading to 2.0

`type-bridge 2.0.0` completes the Rust single-source-of-truth cutover
for the V2 stack: one Rust implementation owns every V2 schema, query,
and migration rule, and Python, TypeScript, and the server consume
validated projections of it. A 1.5.x application upgrades through the
steps below **without source changes first**; opting into V2 authoring
is a separate, later step.

## Compatibility schedule

The named removals in [V2 Deprecations](v2-deprecations.md) follow ordinary
SemVer and are scheduled for `v3.0.0`, not a 2.x minor release. Every
deprecated surface stays fully operational throughout the 2.x release line
unless an individual migration scope explicitly completes the irreversible V2
adoption described below. Adoption closes only that scope's legacy writer
lane; it is not a package-wide early removal. The inventory is exact: a
surface without an explicit removal schedule remains unscheduled.

## Step 1 — upgrade without source changes

Update the dependency and run your existing application:

- V1 models, fused `Role[T]`, CRUD managers, `Query`/`QueryBuilder`,
  and the Node typed query surface keep their released behavior,
  executing through the same released V1 engines. The native module is
  required (`type-bridge-core` is installed as an exact same-version
  dependency, and `TYPE_BRIDGE_BACKEND` accepts only the Rust engine).
- Python V1 `Query` TypeQL compilation routes through the Rust V1
  compiler and retains the released automatic Python-compiler fallback:
  a query the Rust compiler rejects still compiles and executes, with
  the released DEBUG-level fallback diagnostic. Sort, offset, and limit
  emission for the Python facade remains in Python.
- Rust V1 `MatchRequest` remains on its V1 engine. Valid released requests
  retain their public result shapes, ordering, and diagnostics; internally,
  every provider statement has a finite resource ceiling. When the public
  projection is exactly the provider's distinct projection, the public window
  is placed in TypeQL and the finite stream is consumed through its terminal
  frame. Shapes with hidden witness bindings retain the released behavior of
  stopping after the required public prefix. Owned read transactions receive
  bounded explicit cleanup; the remaining upstream early-stream close
  limitation is tracked by
  [#196](https://github.com/ds1sqe/type-bridge/issues/196). An experimental
  adapter onto V2 plans exists but is not wired into any execution path, covers
  only part of the released query algebra, and is **not** part of the upgrade
  contract.
- Existing legacy migrations keep working: the released readers and checksum
  rules remain compatible, and old snapshots remain readable. Newly authored
  snapshots may carry additive declared-descriptor authority; existing
  snapshot bytes are neither rewritten nor required to contain it.

## Step 2 — adopt canonical migrations

Import your legacy migration history once with `type-bridge migration
adopt --environment <env> --legacy-directory <dir>`. First run
`python -m type_bridge.migration.sidecar <dir>`. The frozen trusted Python
reader emits checksum- and digest-bound adoption metadata for every migration;
it also emits an executable JSON sidecar where the released operation graph is
portable. Adoption never re-executes legacy operations, including
`RunPython`.

Adoption treats the supplied history as security authority, so first
materialize its recognized migration sources, sidecars, adoption metadata,
ordinary migration-package resources, `snapshots/` hierarchy, and snapshot
contents as regular local files and real directories, not symbolic links or
special entries. Relevant filenames, JSON, and schema text must be UTF-8.
Migration source decoding retains the released
`Path.read_text()` contract, including the runtime's default encoding and
universal-newline handling; Python module execution additionally retains
Python's BOM and PEP 263 rules. Run the converter under the same intended
runtime and locale as the released history. `snapshot.json` and every file it
binds must be regular, bounded, and hash-matching. Unbound ambient children,
including `__pycache__`, are ignored without being opened, followed, hashed, or
copied into authority. Each inspected directory is limited to 65,536 entries,
each authority artifact to 16 MiB, and the retained history to 256 MiB in
aggregate. If an archival layout uses links or exceeds those ceilings, make a
bounded regular-file copy containing the bound history and run the sidecar
converter against that copy. These restrictions apply to the adoption
checkpoint; they do not narrow the retained released legacy reader. The
`--legacy-directory`
path may itself resolve through a directory link; the no-link requirement
begins with the recognized authority entries below that root.

Treat adoption as a writer cutover, not as an online coexistence protocol.
Quiesce every legacy migrator for the scope and revoke or rotate the credential
that could run it before invoking `migration adopt`. A released 1.5.x process
that already passed its ledger preflight, an old `SchemaManager` or
`SimpleMigrationManager`, a current `RunPython` callback that opens its own
transaction after its immediate preflight, or code that writes TypeDB directly
cannot be retroactively stopped by TypeBridge. Those paths cannot make the
marker a substitute for credential revocation. The checkpoint records an exact
internal cutover row in the legacy ledger in the
same managed schema transaction as the V2 adoption anchor. Current 2.x
TypeBridge-owned legacy writer entry points validate that exact bound pair and
reject before mutation; a coincidental legacy migration name or ID without the
anchor is not a cutover. Verified legacy readers continue to work and filter
only the exact internal row. Offline files can still be edited, but after
cutover they are historical input rather than active authority; returning the
scope to legacy writing is unsupported.

The native adopter validates the complete dependency graph and applied ledger,
then reconstructs the legacy head from its immutable snapshot authority. A
models-free, `RunPython`-only schema-neutral migration may inherit the exact
snapshot shared by its parents. A released sidecar whose operation list is
empty or contains only schema-neutral `copy_attribute` data operations may do
the same; every other head needs an exact bound snapshot.
For a multi-head history, every selected parent authority is independently
verified. Distinct snapshot owners may converge when their schema hash and
exact schema bytes agree; different hashes or bytes fail closed instead of
choosing one branch. The live schema export is used only to prove equality with
that independently reconstructed head. The verified snapshot bytes—not the
live export—are published atomically as the durable
`adopted-genesis.typeql` artifact, and the canonical V2 chain continues from
the checkpointed frontier. Failed cutovers retain that authority and can be
retried safely. New migrations are authored with `type-bridge migration make`
and applied with `type-bridge migration apply` against named workspace
environments with symbolic credentials.

## Step 3 — convert schema authoring

Move desired-schema authoring to the split-YAML workspace when ready:

- **From TOML**: the public read-only converter
  `type_bridge_core.toml_to_typeql` renders your `schema.toml` into
  canonical TypeQL for review; it and its frozen parser are permanent.
  TOML authoring itself is deprecated. Translate the reviewed schema
  into split-YAML fragments by hand following the workspace format
  below. No automated TOML-, TypeQL-, or Python-to-YAML converter is part
  of 2.0.0.
- **From Python declarations / fused `Role[T]`**: run your existing
  declarations unchanged (Step 1) until you author the equivalent
  split-YAML fragments; the generated projections from
  `type-bridge schema generate` then replace hand-written models with
  explicit `relates()`/`plays()`.

A V2 workspace is a `typebridge.yaml` manifest, a schema-set manifest
(`format: typebridge.schema-set/v1`) listing fragment documents, and
`format: typebridge.schema/v2` fragments. `type-bridge schema check`
validates it offline; generation produces Python, TypeScript, and Rust
projections from the one canonical schema. `type-bridge schema
export-declared --output declared-schema.json` emits the deterministic
canonical authority artifact consumed by a V2 server. The versioned
[Split-YAML and Workspace V1 Reference](split-yaml-v1.md) defines every
accepted field and includes the checked executable fixture.

### Opt-in TLS transport

Plaintext remains the default at every layer. A workspace environment enables
TLS with the closed string-Boolean policy below:

```yaml
environments:
  production:
    database: example
    uri: typedb.example.internal:1729
    tls: 'true'
    tls-root-ca: certs/production-root.pem
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
```

Omit `tls-root-ca` to use operating-system trust roots. A custom PEM bundle is
workspace-confined, must be non-empty and no larger than 1 MiB, and is trusted
instead of—not in addition to—the operating-system store. Supplying a root
with omitted or false `tls` is a pre-I/O error; a path never enables TLS
implicitly. HTTP version discovery, gRPC driver-band fallback, database
lifecycle calls, and transactions all retain the one resolved mode and never
retry over plaintext.

For the standalone server, outbound TypeDB transport uses `tls = true` and an
optional `tls-root-ca` in `[typedb]`. Relative paths resolve against the server
configuration file. The configuration itself must be a regular file no larger
than 1 MiB. Optional inbound HTTPS termination is:

```toml
[server.tls]
cert-path = "certs/server-chain.pem"
key-path = "certs/server-key.pem"
```

Both inbound files are required and validated before the listener binds;
omitting the block preserves the released HTTP listener. V1 response shapes,
status codes, and error encodings remain unchanged; the one intentional
byte-level exception is `/health.version`, whose value reflects the current
TypeBridge package identity instead of retaining a 1.5.x version string.

When V2 routes are enabled, a relative `v2.declared_schema_file` also resolves
against the server configuration directory. Every path component and the
final target must be free of symbolic links; the target must be a non-empty
regular file no larger than 16 MiB. The server double-reads and compares the
file while loading configuration, retains those verified bytes as the
immutable authority snapshot, and never reopens the path for requests. Replace
the file and reload the complete server configuration to adopt new authority;
mutating the public path field after loading is rejected.

## Step 4 — opt into V2 queries

New code uses prepared V2 plans: reusable, capability-gated,
schema-validated read programs with typed inputs, executed locally or
through the versioned remote envelope with identical semantics. Plan
authoring is a Rust surface in 2.0.0; the Python and Node bindings
execute prepared plans (canonical plan bytes plus invocation) but do
not yet offer idiomatic plan-builder facades — those ship in a later
`2.0.x` release. V1 queries continue to work throughout 2.x and
migrate one query at a time as the V2 authoring surface reaches your
binding.

For remote execution, the exact capability advertisement is an explicit trust
input: it carries the executor epoch and reply-signing identity that preparation
pins. Fetching `/v2/capabilities` over unauthenticated HTTP is discovery, not a
trust bootstrap, because an intermediary could substitute its own key. Obtain
the advertisement over authenticated TLS with the intended server identity or
provision/pin its exact bytes or fingerprint out of band. A standalone server
rotates the epoch and signing identity on restart, so clients must authenticate
and accept the new advertisement rather than silently treating it as the old
executor.

Prepared execution has two explicit authority modes:

- **Managed** is the default. `type_bridge_core.query_v2_authority(...)`, the
  Node `new QueryV2Authority(...)` constructor, and a standalone server with
  `authority_mode = "managed"` require the exact V2 migration-control schema
  and singleton for the configured scope. Execution is admitted only while
  that singleton is free and has no holder.
- **Query-only** is for a database that has no V2 or legacy migration controls.
  Local callers must bind the authority to the exact database with
  `type_bridge_core.query_v2_query_only_authority(database, ...)` or
  `QueryV2Authority.queryOnly(database, ...)`. That handle cannot prepare a
  remote request. A standalone executor opts in with
  `authority_mode = "query_only"`; it rejects a database if either control
  schema is present.

Both modes require the configured profile to equal the connected server's
exact `typedb-<version>/v1` identity and re-check the live schema before work.
The transaction that actually executes a plan captures its schema export under
a bounded TypeDB schema-exclusion fence. With the 3.12.1 driver this is a
read-only query carried by a `WRITE` transaction, so the credential needs the
corresponding transaction permission and a long-running V2 query can delay a
concurrent `SCHEMA` transaction until it closes. One request is bounded by one
absolute deadline (30 seconds by default, at most five minutes); timeout or
cancellation closes the transaction and never publishes a partial answer.

## One semantic engine on every V2 path

As of 2.0, exactly one implementation — Rust — resolves schemas, plans
migrations, validates queries, and emits TypeQL on every **V2** path.
The deprecated V1 facades keep their released engines: the Python and
TypeScript packages retain their own inheritance-resolution and
emission code where it serves those facades, including the Python
V1 query-compiler fallback described in Step 1. That code never
executes on a V2 path and is removed only together with its facade,
under the conditions in [V2 Deprecations](v2-deprecations.md).
