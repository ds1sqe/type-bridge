# Upgrading to 2.0

`type-bridge 2.0.0` completes the Rust single-source-of-truth cutover:
one Rust implementation owns every schema, query, and migration rule,
and Python, TypeScript, and the server consume validated projections of
it. A 1.5.7 application upgrades through the documented adapter path
below **without source changes first**; opting into V2 authoring is a
separate, later step.

## The SemVer exception, disclosed

The scheduled `v2.1.0` removals listed in
[V2 Deprecations](v2-deprecations.md) are an **intentional exception to
ordinary SemVer minor compatibility**: `v2.1.0` removes the V1 surfaces
named there, in a minor release, after the notice period.

- Pin `type-bridge>=2.0,<2.1` if you depend on any deprecated V1
  surface.
- `v2.1.0` cannot ship before the earliest calendar cutoff published in
  the `v2.0.0` release notes; the final `v2.0.x` line is maintained
  through that migration window.
- Every deprecated surface stays fully operational throughout `2.0.x`.

## Step 1 — upgrade without source changes

Update the dependency and run your existing application:

- V1 models, fused `Role[T]`, CRUD managers, `Query`/`QueryBuilder`,
  and the Node typed query surface keep their released behavior. Their
  semantics now execute in Rust; the native module is required
  (`type-bridge-core` ships with the package, and
  `TYPE_BRIDGE_BACKEND` accepts only the Rust engine).
- Rust V1 `MatchRequest` queries lower onto V2 plans through the
  one-way adapter (`adapt_match_request`), whose result parity against
  direct V1 execution is proven by a live conformance corpus.
- Existing legacy migrations keep working: readers, checksum
  verification, and snapshots are unchanged.

## Step 2 — adopt canonical migrations

Import your legacy migration history once through the legacy-frontier
bridge: the applied ledger is reconstructed from your live database and
the frozen legacy manifests, and the canonical V2 chain continues from
that frontier. New migrations are authored with `type-bridge migration
make` and applied with `type-bridge migration apply` against named
workspace environments with symbolic credentials.

## Step 3 — convert schema authoring

Move desired-schema authoring to the split-YAML workspace when ready:

- **From TOML**: the read-only converter renders your `schema.toml`
  into canonical TypeQL (`toml_to_typeql`) and imports it into the
  canonical schema graph (`toml_to_declared`). TOML authoring is
  deprecated; the converter and its frozen parser are permanent.
- **From TypeQL**: `typeql_to_declared` imports an existing `define`
  block into the same canonical graph. TypeQL import is a permanent
  surface.
- **From Python declarations / fused `Role[T]`**: the canonical graph
  generates split-YAML-equivalent projections with explicit
  `relates()`/`plays()`; declarations converted through either importer
  above land in the same one canonical schema.

A V2 workspace is a `typebridge.yaml` manifest, a schema-set manifest
(`format: typebridge.schema-set/v1`) listing fragment documents, and
`format: typebridge.schema/v2` fragments. `type-bridge schema check`
validates it offline; generation produces Python, TypeScript, and Rust
projections from the one canonical schema.

## Step 4 — opt into V2 queries

New code uses prepared V2 plans: reusable, capability-gated,
schema-validated read programs with typed inputs, executed locally or
through the versioned remote envelope with identical semantics. V1
queries continue to work throughout `2.0.x`; the adapter migration can
proceed one query at a time.

## One semantic engine

As of 2.0, exactly one implementation — Rust — resolves schemas, plans
migrations, validates queries, and emits TypeQL on every V2 path. The
Python and TypeScript packages retain their own inheritance-resolution
and emission code **only** where it serves the deprecated V1 facades;
that code is scheduled for removal with those facades in `v2.1.0` and
never executes on a V2 path.
