# Upgrading to 2.0

`type-bridge 2.0.0` completes the Rust single-source-of-truth cutover
for the V2 stack: one Rust implementation owns every V2 schema, query,
and migration rule, and Python, TypeScript, and the server consume
validated projections of it. A 1.5.x application upgrades through the
steps below **without source changes first**; opting into V2 authoring
is a separate, later step.

## Compatibility schedule

The named removals in [V2 Deprecations](v2-deprecations.md) all land in the
`v2.1.0` minor release as a deliberate, exactly-enumerated exception to
ordinary major-version scheduling: active TypeDB 3.8/3.10 provider support
(the wheel then embeds only the band-8 and band-9 driver lines), direct TOML
desired-schema authoring, the V1 schema and model facades, and legacy
migration authoring. There is no `v3.0.0` removal plan. Every deprecated
surface stays fully operational throughout 2.0.x — and every surface without
a removal schedule throughout the whole 2.x line —
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
- Rust V1 `MatchRequest` keeps its released public request, result, ordering,
  and diagnostic contract. Internally, a valid request is checked against one
  descriptor-registry snapshot, translated through the same Rust V2 builder
  gate used by ordinary V2 plans, and executed and hydrated against that same
  snapshot. The production parity corpus covers every released operation,
  predicate, output, collection, ordering, inherited-role, and scalar family.
  The direct V1 executor remains only for an exact representation-envelope
  case: a released string or its escaped canonical plan can exceed V2's fixed
  artifact-size ceiling. That fallback is selected after V1 validation and
  cannot be requested by decoded V2 input. No source change is required in
  either case.
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
  TOML authoring itself is deprecated and is removed in `v2.1.0` with the
  rest of the legacy window, so translate
  before upgrading past 2.0.x or pin `type-bridge>=2,<2.1`. Translate
  the reviewed schema
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
methods, status codes, headers, bodies, and error encodings remain unchanged.
That includes `/health.version`, which remains the final released V1 HTTP
identity, `1.5.11`; use the package metadata or `type-bridge-server --version`
when the installed V2 package identity is required.

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
through the versioned remote envelope with identical semantics. Python
authors complete low-level plans through `type_bridge.query_v2`; Node uses
`@type-bridge/node/query-v2`. Both expose `QueryV2Authority`,
`QueryPlanBuilder`, `AuthoredQueryPlan`, and `AuthoredQueryInvocation`.
Their opaque handles delegate every transition, validation, canonical byte,
fingerprint, and capability decision to the same Rust incremental builder;
neither binding assembles plan JSON. V1 queries continue to work throughout
2.x, so applications can migrate one query at a time without changing
unrelated direct-query code.

For model-oriented code, keep `QuerySession` when the application owns a
`Database` or read transaction and wants the released synchronous terminal
types. Use `RemoteQuerySession` only when the application owns a remote
executor exchange. Its immutable composition grammar is the same, but
`one()`, `rows()`, `page_by()` / `pageBy()`, `count_by()` / `countBy()`, and
`exists_by()` / `existsBy()` are explicitly awaitable. Constructing variables,
predicates, and queries performs no I/O. One awaited terminal performs exactly
one caller-supplied request/response exchange and no retry or client-side
hydration query. See [Immutable Typed Queries](typed-queries.md#remote-model-queries)
for exact Python and Node imports, limits, and examples.

Adopt V2 query-by-query:

1. Preserve the existing V1 query and its compatibility tests.
2. Create one `QueryV2Authority` from the canonical declared-schema snapshot
   for the intended scope and exact `typedb-<version>/v1` profile.
3. Choose either the complete low-level builder or the model-oriented
   `QuerySession` / `RemoteQuerySession`; do not translate plans through
   hand-written JSON.
4. Compare result shape, ordering, diagnostics, and concrete-subtype
   materialization before switching the call site.
5. Remove only that migrated call site's V1 construction. No V1 facade has a
   scheduled removal, so unrelated queries may remain unchanged.

The binding execution smokes now author their plans at runtime. This retires
only the former embedded `PLAN_B64` constants in the Python and Node V2 binding
smokes. Canonical codec goldens, fingerprint vectors, hostile remote-envelope
fixtures, V1 wire fixtures, MatchRequest corpora, and released compatibility
fixtures remain authoritative and are not replaced by facade-generated bytes.

For remote execution, the exact capability advertisement is an explicit trust
input: it carries the executor epoch and reply-signing identity that preparation
pins. Fetching `/v2/capabilities` over unauthenticated HTTP is discovery, not a
trust bootstrap, because an intermediary could substitute its own key. Obtain
the advertisement over authenticated TLS with the intended server identity or
provision/pin its exact bytes or fingerprint out of band. A standalone server
rotates the epoch and signing identity on restart, so clients must authenticate
and accept the new advertisement rather than silently treating it as the old
executor.

The caller owns capability discovery, advertisement authentication,
credentials, TLS, HTTP or other transport, application headers, response
status handling, and retry policy. TypeBridge accepts the authenticated
advertisement and an exchange callback; it does not construct an HTTP client
or application context. Remote limits and the absolute deadline are pinned
before the exchange. Preparation failure performs zero exchanges, and a
transport failure is neither decoded nor retried by TypeBridge.

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
The deprecated V1 facades keep their released behavior and public contracts.
Some retain their existing Python or TypeScript inheritance-resolution and
emission code, including the Python V1 query-compiler fallback described in
Step 1. Rust `MatchRequest` execution may instead delegate representable
requests to the parity-proven V2 compatibility program and retain the direct
executor for its explicit legacy-required dispositions. That internal choice
does not schedule the V1 facade for removal. A retained implementation is
removed only together with its facade under the conditions in
[V2 Deprecations](v2-deprecations.md).
