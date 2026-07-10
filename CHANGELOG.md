# Changelog

All notable changes to TypeBridge will be documented in this file.

## [Unreleased]

## [1.5.6] - 2026-07-10

### New Features

- **Expose migration-state schema boundary for external ledgers (#165)** - added a public
  migration-state schema descriptor and filtering helpers so callers can keep
  migration execution metadata in an external state store while leaving target
  application schema management untouched.

- **Pluggable migration state manager (#165)** - `MigrationExecutor` now accepts an
  optional `state_manager` so external orchestrators can inject their own state
  persistence backend while retaining the existing default TypeDB-backed behavior.

### Testing & Tooling

- **Integration coverage for external-state migrations (#165)** - added live tests for
  external ledger execution paths and schema filtering behavior that validate default
  and injected-state modes across TypeDB live integration.

## [1.5.5] - 2026-06-30

### Bug Fixes

- **Strict Boolean wrappers** - `Boolean` now rejects non-`bool` values at
  construction, Pydantic validation, Pydantic serialization, hydration, and
  Rust-backend dynamic value boundaries instead of truthiness-coercing strings
  such as `"False"` into `True`.

### Testing & Tooling

- **Boolean strictness regressions** - added unit coverage for direct Boolean
  construction, Pydantic hooks, CRUD hydration through `wrap_attribute_value`,
  and Rust backend boolean value marshalling.

## [1.5.4] - 2026-06-26

### New Features

- **Generator name-case overrides (#163)** - bindgen now accepts case
  annotations case-insensitively, including `@case(PascalCase)` and
  language-scoped forms such as `@case(Python, SnakeCase)`.
- **PascalCase and TOML bindgen metadata (#163)** - Python model generation now
  maps PascalCase TypeDB names to `TypeNameCase.CLASS_NAME`, and TOML schemas
  can carry `bindgen_case` plus `annotations` metadata for generated bindings.

### Bug Fixes

- **Mixed-case generated names (#163)** - class-name generation now preserves
  mixed-case sub-segments such as `FirstPerson` instead of lowercasing them
  during Python and Rust bindgen output.
- **Explicit generated type names (#163)** - generated attribute configs now
  emit explicit `name="..."` flags when the TypeDB name does not align with
  the default language-specific name inference.

### Testing & Tooling

- **Cargo CI isolation (#163)** - CI now keeps Cargo homes and caches outside
  the workspace, isolates cache keys by lockfile, and pins the stable Rust
  toolchain to reduce release-run cache and registry churn.
- **Cargo lock checksum repair (#163)** - refreshed the affected Rust lockfile
  checksums so locked metadata validation passes cleanly in release checks.

## [1.5.3] - 2026-06-24

### New Features

- **ORM-backed Python data migrations (#158)** - migrations can now use
  `ops.RunPython(...)` to run normal TypeBridge ORM code during migration
  execution, including `User.manager(db).filter(...)`, bulk inserts, updates,
  deletes, and JSON/TOML-backed data loading.
- **Database-backed migration history (#158)** - applied migration state and
  execution attempts are stored in TypeDB through `type_bridge_migration` and
  `type_bridge_migration_run`, including run IDs, checksums, direction, status,
  timestamps, errors, and executor metadata when available.
- **Historical snapshot bindings for generated migrations (#160)** - generated
  migrations now import stable time-state bindings such as
  `migrations.snapshots.v0001.User` instead of the active generated model
  package, keeping old migrations replayable after later bindgen output removes
  or reshapes types.

### Changes

- **Snapshot drift protection (#160)** - existing snapshot packages are
  validated by schema hash, generated file hash manifest, and top-level binding
  exports before reuse, so stale or hand-edited historical bindings fail
  clearly instead of silently changing migration semantics.

### Documentation

- **Migration guide added (#158/#160)** - added a dedicated migration guide that
  explains the generated migration workflow, why snapshots matter, `RunPython`
  data migrations, rollback, sidecar checksum drift, and TypeDB-backed
  migration/run history.

## [1.5.2] - 2026-06-18

### New Features

- **Rust bindgen model renderer (#157)** - Python, TypeScript, and Rust model
  rendering now run through the shared Rust bindgen engine, with Python
  `generate_models` and the Node generator facade delegating to native
  `render_models_json` bindings.
- **Renderer deduplication (#157)** - The old Python Jinja renderer and
  duplicated TypeScript renderer implementation were removed, so future model
  generation changes land in one core implementation with cross-language parity
  coverage.

### Bug Fixes

- **Shared TypeDB runtime parity (#155/#154)** - `type_bridge_server::typedb::TypeDBClient`
  now uses the same gRPC-only connect path, HTTP-probe fallback, and exact
  `server_version` override logic as the ORM `Database.connect` surface. The
  ORM and server both depend on a shared TypeDB runtime crate for driver
  construction, database lifecycle, transaction execution, and JSON result
  conversion.
- **Node gRPC-only connect option parity (#154)** - `@type-bridge/node`
  `ensureDatabase` and `RustDatabase.connect` now accept `serverVersion`, letting
  Node callers skip the HTTP version probe for gRPC-only deployments while still
  validating the exact TypeDB server version.

## [1.5.1] - 2026-06-17

### Bug Fixes

- **gRPC-only TypeDB connect support (#154)** - `Database.connect(..., server_version=...)`
  now accepts an exact server version across Rust, PyO3, and Python connect surfaces,
  letting deployments without a reachable HTTP version endpoint skip the HTTP probe
  while still validating the supported window.
- **HTTP probe fallback to embedded driver negotiation (#154)** - when no
  `server_version` is supplied and the HTTP probe fails, the Rust runtime now falls
  back through gRPC band negotiation, trying band 8 and then band 7 and reporting
  every attempted path if neither driver can connect.
- **Version-gate CI coverage for fallback paths (#154)** - CI now includes positive
  cells that force the HTTP probe onto a bad port and assert the gRPC fallback
  succeeds, with helper updates for both success and rejection expectations.

### New Features

- **Band-aware server database admin API (#155)** - `type_bridge_server::typedb::TypeDBClient`
  now exposes `database_exists`, `create_database`, `delete_database`, and
  `reset_database`, dispatching through the embedded TypeDB driver band selected
  at connection time.
- **Node database lifecycle parity (#155)** - `@type-bridge/node` `RustDatabase`
  now exposes `databaseExists`, `createDatabase`, `deleteDatabase`, and
  `resetDatabase` on the bound database handle, matching the Python/Rust ORM
  lifecycle surface.

## [1.5.0] - 2026-06-15

### Changes

#### Rust-SSOT typed migrations from generated schema diffs (#152)

- **Generated migrations default to typed operations** — `makemigrations` now
  renders schema diffs as operations such as `ops.AddAttribute(...)`,
  `ops.AddEntity(...)`, `ops.AddRelation(...)`, `ops.AddOwnership(...,
  optional=True)`, and role/player changes instead of flattening schema diffs to
  raw TypeQL.
- **Rust remains the migration execution source of truth** — generated migration
  sidecars preserve structured Rust `OperationSpec` values, and the Rust planner
  lowers typed operations to executable TypeQL at planning/execution time.
- **`ops.RunTypeQL` remains supported as an escape hatch** — handwritten data
  migrations and custom TypeQL can still use `ops.RunTypeQL(...)`; Rust planning
  infers schema vs write transaction kind so data scripts execute correctly.
- **Django-style `schema.toml` lifecycle documented** — the generator guide and
  `examples/advanced/toml_migration/` now show the user flow: edit
  `schema.toml`, run bindgen/generator and `makemigrations`, receive
  `0001_initial.py`, migrate, edit the TOML again, and receive a follow-up
  typed migration such as `0002_add_email.py`.
- **Real smoke coverage added** — integration coverage exercises TOML bindgen,
  an initial typed migration, real data insertion, a typed optional-ownership
  delta, an explicit `ops.RunTypeQL` data migration, and a failing migration path
  that remains unapplied while preserving prior data.

#### Rust runtime as the default Python execution backend

- **`Database.connect()` now opens the embedded Rust runtime by default** — the
  Python ORM no longer needs the optional external `typedb-driver` package for
  normal connection, schema, CRUD, query, migration, or database lifecycle
  operations.
- **Database lifecycle operations are Rust-backed** — `database_exists()`,
  `create_database()`, `delete_database()`, and schema export use the same
  embedded Rust runtime path as query execution.
- **Direct Python driver access remains explicit** — `Database.driver` still
  opens the optional external Python `typedb-driver` and keeps its installed
  driver/server compatibility gate for callers that need direct driver APIs.

#### Dual-band TypeDB driver support: servers 3.8–3.11 served by one artifact (#145)

- **Both TypeDB Rust driver lines now embedded** — the wheel ships a vendored band-7 fork
  (3.8.1) alongside the upstream band-8 driver (3.11.5). The runtime dispatches to the
  correct driver at `Database.connect()` time based on the server's protocol band; no
  user-side configuration is required.
- **TypeDB 3.11 newly supported** — band-8 (3.11.x) servers are now served in addition
  to the existing band-7 window.
- **Existing 3.8/3.10 deployments keep working without change** — the band-7 embedded
  driver is a drop-in for the previous release line; upgrading type-bridge does not require
  a server upgrade.
- **One release artifact covers TypeDB 3.8.0 through 3.11.x** — the per-band install
  guidance is retired. Install `type-bridge` and connect to any server in the supported
  window.
- **CI proven on all three server lines** — `test-integration`, `node-integration`, and
  `cross-language-parity` now fan across 3.8.3, 3.10.4, and 3.11.5 (parity ×2). The
  `version-gate-cells` job retains only the true rejection cells (NEG-window, NEG-driver).

#### Cross-language CI and release coverage (#110)

- **Node binding built, tested, and released** — the `@type-bridge/node` binding is
  now built and gated in CI (native module, TypeScript surface, unit, `.d.ts`
  baseline, and package smoke), runnable locally via `scripts/check.sh node`, and
  packaged as a tarball attached to each GitHub release (registry publish staged
  behind an `NPM_TOKEN`).
- **Cross-language parity enforced** — the Python/Node parity suite runs against live
  TypeDB in CI under a strict mode that fails (rather than silently skips) when the
  binding is unbuilt, so a semantic divergence between the two bindings reds CI; the
  `migration` integration group is also covered.
- **Release hardening** — third-party GitHub Actions are pinned to commit SHAs, and a
  crates.io publish that fails for any reason other than an already-published version
  now fails the release instead of being swallowed.
- **Node binding integration gated in CI** — a `node-integration` job runs the binding's
  own live-TypeDB suites (`test:integration`, which chains `test:typed-integration`)
  against a TypeDB service, mirroring the Python integration job. Previously only the
  offline Node gates and the Python-side parity suite ran; the binding's own integration
  assertions now red CI on a regression.
- **Consolidated test/check scripts** — `test.sh` is now the single full-test entrypoint
  (Rust + Python + Node, unit + integration) with `--no-integration`, `--proxy`, and
  `--no-isolated` flags; it manages a TypeDB by default. The `test-integration.sh`,
  `test-integration-dind.sh`, and `test-proxy-integration.sh` scripts (and the
  Docker-in-Docker image stack) are retired into those flags, and `scripts/check.sh`
  is the single canonical check (the redundant root `check.sh` is removed).

#### Generator parsing routed through the Rust core (#125)

- **`lark` removed** — the generator no longer depends on `lark`. TQL schema
  parsing runs entirely through the Rust `type_bridge_core` core, which is the
  single source of the parsed schema. The `typeql.lark` grammar and the
  lark-vs-Rust differential test have been deleted, and `lark` is dropped from
  the runtime dependencies.
- **Structured function return types** — `FunctionSpec.return_type` is now a
  structured `ReturnTypeSpec` (a `is_stream` flag plus an ordered list of
  `ReturnTypeItem(name, optional)`) instead of a formatted string. Optionality
  is carried as a boolean, so the intermediate representation is
  language-neutral and additive for future bindings. Generated output is
  unchanged — the renderer reconstructs any `?` tokens at its own emit site.

### New Features

#### Plays-side role cardinality (#130)

- **Cross-language plays-cardinality authoring** — Rust `SchemaInfo`, Python
  `Role(..., plays_cardinality=Card(...))`, TypeScript
  `role(..., { playsCardinality: Card(...) })`, and TOML `plays = [{ card =
  "..." }]` now emit `plays relation:role @card(...)` through the shared Rust
  schema emitter/parser path.
- **Relates/plays separation** — relates-side `cardinality` still constrains
  players per relation instance, while plays-side cardinality constrains how
  many relation instances a single player may participate in for that role. The
  two constraints may coexist on the same role.
- **Generated surface round-trip** — parsed TypeQL with plays-side `@card`
  renders back to Python and TypeScript role declarations, preserving
  authoring/render parity.

#### TOML schema DSL (#138)

- **TOML schema authoring** — schemas may now be authored in TOML as an
  additive alternative to TypeQL. A new Rust/PyO3 `toml_to_typeql` transpiles a
  TOML document to canonical TypeQL, which feeds the single existing parse path,
  so the intermediate representation, renderers, and generated packages are
  unchanged. Equivalent TOML and `.tql` schemas produce byte-for-byte identical
  packages. `.tql` authoring remains fully supported.
- **Full schema surface** — the DSL covers attributes (value types, `sub`,
  `abstract`, `@regex`/`@values`/`@range`), entities (`owns` with
  `@key`/`@unique`/`@card`, `plays` with optional per-plays `@card`, `sub`,
  `abstract`), relations (`roles` with per-role `@card` and `as` super-role
  overrides, `owns`, relation-level `plays`, `sub`, `abstract`), functions
  (signature plus verbatim body), and structs.
- **Generator routing** — `generate_models` transpiles a `.toml` source by file
  suffix, or for raw text via an explicit `format="toml"` argument.
- **Field-level diagnostics** — a semantic validation pass rejects malformed TOML
  with a clear `ValueError` naming the offending field or type (unknown value
  type, `value`/`sub` conflict, dangling `sub` parent, missing role player, empty
  struct, malformed function body) before any TypeQL is emitted.

#### Node.js typed model layer (#124)

- **Branded attributes** — `class Name extends attr.String("name") {}` defines a
  hard-typed attribute whose class is nominally distinct from other attributes
  (`Name` is not interchangeable with `Email` or raw `string`). One factory per
  TypeDB value type; the mandatory name is both the schema attribute name and the
  compile-time brand.
- **Typed `Entity` / `Relation` classes** — `class Person extends Entity("person",
  { name: field(Name, Key), age: field(Age).optional() }) {}` yields typed
  construction and field reads, mirroring the Python class surface. Relations
  support multi-player `role(...)` declarations.
- **Descriptor emission** — typed models emit registration descriptors
  byte-identical to the Python facade (verified against the shared parity
  fixture), so the same model definition drives the existing dynamic managers.

## [1.4.5] - 2026-05-21

### Bug Fixes

##### Python ORM
- **Relation role-player hydration** — relation role players fetched as IID-only placeholders now hydrate correctly without requiring their own nested role players (#126).
- **Optional relation roles** — `Relation.manager(...).all()` / fetch queries now include relations whose optional roles have no players (#127).
- **Optional role validation** — omitted optional roles normalize to `None` instead of leaking class-level `RoleRef` defaults, while required roles are still rejected when missing.
- **Role-player deduplication** — relation role players are deduplicated by IID when available, preserving distinct attribute-less relation players.
- **TypeDB driver compatibility** — added `create_driver_options()` helper and exported it from `type_bridge` to support driver option construction across supported TypeDB driver APIs.

##### Rust Core / Server
- **Raw TypeQL endpoint** — restored `POST /query/raw` on the Rust server, parsing TypeQL text into clauses before running the existing query pipeline.
- **Server configuration** — TypeDB connection settings can now be overridden with `TYPEDB_ADDRESS`, `TYPEDB_DATABASE`, `TYPEDB_USERNAME`, and `TYPEDB_PASSWORD`.
- **Clippy cleanup** — updated ORM role collection code to satisfy current Rust lints.

### Compatibility

- **Pinned TypeDB runtime** — Python dependency now pins `typedb-driver==3.10.0`.
- **Python version bounds** — package metadata now declares `>=3.13,<3.15`.
- **Integration environment** — Docker Compose defaults now use TypeDB 3.10.4 and expose a configurable `TYPEDB_PORT`.

### Testing & Tooling

- **Pinned Docker-in-Docker integration runner** — added `Dockerfile.integration`, `docker-compose.dind.yml`, and `test-integration-dind.sh` for reproducible Python 3.13.5 + TypeDB 3.10.4 integration tests.
- Added regression coverage for relation role-player hydration, optional relation fetches, raw proxy queries, and server environment overrides.

## [1.4.4] - 2026-04-09

### New Features

#### Cross-type attribute lookup (PR #117)

`Entity.has` / `Relation.has` / `<Concrete>.has` — find all instances that own a
given attribute, optionally filtered by value or expression.

##### Python ORM — `type_bridge.crud.has_lookup`
- **Cross-type form** — `Entity.has(db, Name)` returns mixed concrete subtypes across all entities owning `Name`.
- **Concrete-class narrowing** — `HLPerson.has(db, Name)` narrows to that type and its TypeDB subtypes via `isa` polymorphism. Abstract base subclasses get subtype matching for free.
- **Role-player hydration** — relation results always include hydrated role players. Entity lookups remain single-query; relations are re-fetched per result through `concrete_class.manager(connection).get(_iid=iid)`.
- **`Attribute.get_owners()`** — static discovery of attribute owners without a database connection, backed by a reverse `_attribute_owners` index in `ModelRegistry`.

## [1.4.3] - 2026-04-05

### Bug Fixes

##### Rust Server — `type-bridge-server`
- **fix**: `extract_crud_info()` now reports the write-target type for Match + Insert/Put queries (#121)

##### Rust Core — `type-bridge-core-lib`
- **refactor**: use idiomatic match guards in validation.rs

## [1.4.2] - 2026-03-04

##### Rust Server — `type-bridge-server`
- **`CrudInfo`** on `RequestContext` — operation, type_name, type_kind, attribute_names, iid
- **`CrudInterceptor` trait** with `on_crud_request` / `on_crud_response` and `should_intercept`
- **`CrudInterceptorAdapter`** bridges `CrudInterceptor` into the existing `Interceptor` chain

## [1.4.1] - 2026-03-03

### New Features

#### Lifecycle Hook System (PR #118, closes #116)

Three-layer hook system for reacting to CRUD lifecycle events (audit logging, validation, cache invalidation, async notifications).

##### Python ORM — `type_bridge.crud.hooks`
- **`CrudEvent` enum** — `PRE_INSERT`, `POST_INSERT`, `PRE_UPDATE`, `POST_UPDATE`, `PRE_DELETE`, `POST_DELETE`, `PRE_PUT`, `POST_PUT`
- **`LifecycleHook` protocol** — implement only the methods you need (`pre_insert`, `post_delete`, etc.)
- **`HookCancelled` exception** — raise in a pre-hook to abort the operation
- **Per-manager registration** — `manager.add_hook(hook)` / `manager.remove_hook(hook)`, chainable
- **`should_run(event, sender)`** filtering by event type or model class
- Pre-hooks run in registration order; post-hooks run in reverse order (middleware unwinding)
- Zero overhead when no hooks are registered

##### Rust ORM — `type-bridge-orm`
- **`LifecycleHook` trait** with `HookContext`, `PreHookResult` (Continue/Reject), and `HookRunner`
- Integrated into `EntityManager` and `RelationManager` via `add_hook()`
- Same semantics as the Python layer (registration-order pre-hooks, reverse-order post-hooks)

#### Put (Upsert) Clause — `type-bridge-core-lib`
- **Added `Clause::Put(Vec<Statement>)` variant** for idempotent insert (upsert) operations
  - Parser: `parse_put_clause` with keyword lookahead in both `parse_patterns` and `parse_statements`
  - Compiler: Generates `put\n<statements>;` TypeQL output
  - Validation: Reuses Insert validation rules (`Clause::Insert | Clause::Put`)
  - Full roundtrip support (parse → AST → compile → parse)

### Refactoring

#### Server API Simplification — `type-bridge-server`
- **Removed standalone CRUD module** (`/entities/*`, `/relations/*` endpoints, `CrudQueryBuilder`, raw query support) — superseded by the interceptor-based design
- **Removed `crud_builder` benchmark** and `criterion` dev-dependency
- **Server now has 4 endpoints**: `POST /query`, `POST /query/validate`, `GET /health`, `GET /schema`

## [1.4.0] - 2026-02-20

### Highlights

- 5 new Rust crates: `core-lib`, `orm`, `orm-derive`, `server`, and `python` (PyO3 bindings)
- Up to **40x faster validation** and **2.5x faster query compilation** via the Rust backend
- Async Rust ORM with derive macros, chainable queries, and batch operations
- Query-intercepting proxy server with REST endpoints
- MkDocs Material documentation site

### New Features

#### Rust Core — `type-bridge-core-lib` (PRs #95, #101-#108)
- **TypeQL schema parser** with inheritance resolution and PyO3 bindings
- **TypeQL query parser** with bidirectional AST roundtrip
- **Schema-aware query validation** with statement and pattern validators
- **Rust-backed value coercion** and `format_value`
- **Custom validation rules** with portable JSON DSL
- **Wired Rust core into Python** compiler and validation pipeline

#### Async Rust ORM — `type-bridge-orm` (PR #114)
- **Async ORM** with entity CRUD and mock-testable session layer
- **Derive macros** — `#[derive(TypeBridgeEntity)]`, `#[derive(TypeBridgeRelation)]`, `#[derive(TypeBridgeAttribute)]`
- **Chainable query builders** with expression filtering and aggregation
- **Schema management** — registration, generation, diff, and sync
- **Abstract types** with inheritance and code generation
- **Batch operations** — `insert_many`, `delete_many`, `update_many`
- **`FieldRef<A>`** for type-safe query field references
- **`include_schema!`** proc-macro for compile-time TQL codegen
- **Schema introspection** from live TypeDB database
- **Group-by queries** with `GroupByResult`
- **Role player field access** for relation query filtering
- **Expression helpers** — `in_range()`, `startswith()`, `endswith()`
- **Connection pooling** with `Database::into_shared()`
- **Serde support** on all ORM model types
- **Structured tracing** spans on all public methods

#### Query Intercept Proxy — `type-bridge-server` (PR #109)
- **REST CRUD endpoints** with schema-aware query building
- **`CrudQueryBuilder` PyO3 class** for TypeQL generation from Python
- **Extensible library/framework** — pluggable `QueryExecutor`, `Interceptor`, `SchemaSource`
- **207 tests** with 100% MC/DC coverage

### Performance: Python vs Rust

#### Validation

| Operation | Python | Rust | Speedup |
|-----------|--------|------|---------|
| Single type name | 1.89 us | 354.75 ns | **5.3x** |
| Long name (100+ chars) | 24.90 us | 620.52 ns | **40.1x** |
| Batch 1,000 names | 5.32 ms | 266.80 us | **19.9x** |
| Batch 5,000 names | 28.02 ms | 1.33 ms | **21.1x** |

#### Query Compilation (via serde bridge)

| Operation | Python | Rust | Speedup |
|-----------|--------|------|---------|
| Standalone update | 93.28 us | 37.82 us | **2.5x** |
| Large batch (200 clauses) | 2.26 ms | 1.07 ms | **2.1x** |
| Heavy insert (100x6) | 1.71 ms | 896.19 us | **1.9x** |

### Documentation & CI

- **MkDocs + Material** documentation site with auto-generated API reference (PR #98)
- **Rust crate CI** with multi-platform wheel builds — Linux (x86_64, aarch64), macOS (x86_64, aarch64), Windows (x86_64) (PR #95)
- **Comprehensive benchmark suite** with TOML storage, comparison reports, and markdown generation (PR #103)
- **Codecov integration** for Rust coverage tracking (PR #109)

### Bug Fixes

- Resolve Rust 1.93.0 clippy lint errors
- Pin Python 3.13 for Rust CI jobs and fix coverage script
- Add version specifiers to inter-crate path dependencies for crates.io publishing
- Make release workflow idempotent (skip already-published packages)

## [1.3.0] - 2026-02-09

### New Features

#### API DTO Generator (PRs #96, #97, #99, #100)
- **Pydantic DTO generation from TypeQL schemas** — Generate typed Create/Out/Patch DTOs directly from parsed schemas
- **Composite DTOs** — `CompositeEntityConfig` for flat/merged DTOs across entity types with name clash validation
- **Per-variant field requiredness overrides** — `FieldOverride` and `EntityFieldOverride` to control field requiredness per DTO variant (Create, Out, Patch)
- **`skip_variants`** — Skip generating specific variant classes while keeping the type Literal enum
- **`extra_fields_out`** — Per-variant override for extra fields on Out DTOs
- **`strict_out_models`** — Flag for required fields in Out DTOs
- **Reusable Jinja macros** — Refactored templates into `_dto_macros.jinja`
- **CLI support** — `python -m type_bridge.generator` now supports DTO generation

#### Date Arithmetic and Query-Level Arithmetic Expressions
- **Date arithmetic** — `__add__`, `__radd__`, `__sub__` on `Date` for Duration arithmetic, matching DateTime/DateTimeTZ
- **ArithmeticExpr** — New expression class with helpers (`add`, `sub`, `mul`, `div`, `mod`, `pow_`) for TypeQL infix operators
- **ArithmeticValue AST node** and compiler support

### Improvements

- **TypeDB 3.8.0 stable** — Bumped from 3.8.0-rc0 to 3.8.0 release
- **TypeDB driver 3.8.0** — Upgraded `typedb-driver` dependency to 3.8.0
- **Expression module cleanup** — Improved handling across boolean, comparison, and string expression modules
- **Field module cleanup** — Enhanced field handling in base and role modules

### Housekeeping

- Applied ruff formatting to all test files
- Added `.coverage` to `.gitignore` (was accidentally tracked)

## [1.2.7] - 2026-02-03

### New Features

#### TypeDB 3.8.0 Support (PR #93)
- Full compatibility with TypeDB 3.8.0
- Updated driver integration

### Performance Improvements

#### Batch Operations Optimization
- **Batch delete operations** with disjunctive IID matching
- **Batch entity inserts/puts** into single query
- **Optimized IID retrieval** with single-query insert+fetch

### Bug Fixes

- **HydrationError handling** - Raise HydrationError instead of silently swallowing exceptions
- **Variable collision prevention** - Use double underscore separator to prevent variable collisions
- **TypeDB 3.x relation syntax** - Use correct relation insert syntax with `links` keyword
- **Multi-variable let fix** - Remove incorrect parentheses in multi-variable let assignment
- **Polymorphic attributes** - Restore polymorphic attribute fetching with nested wildcard

### Refactoring

- Consolidate duplicated code and fix Python 3.13 warnings
- Unify entity identification logic across managers
- Extract shared attribute-to-AST logic into TypeDBType
- Remove `.c` accessor for direct field access

### Code Quality

- Remove all `type: ignore` comments for full type safety

### Testing

- Add recursive relation tests
- Add validation performance benchmarks

## [1.2.6] - 2026-02-01

### New Features

#### Polymorphic Role Player Type Resolution (PR #92)
- **Role players now resolve to their concrete Python types**
  - When querying relations with abstract role types, role players are resolved to concrete types (e.g., `Person` instead of `Profile`)
  - Uses TypeQL `label()` function to fetch type labels and resolve to correct Python class
  - Location: `type_bridge/crud/relation/manager.py`

#### Relations as Role Players
- **Fixed hydration for relations serving as role players**
  - Relations can now properly participate as role players in other relations
  - Comprehensive test suites added for this pattern
  - Location: `type_bridge/crud/relation/manager.py`

### Improvements

#### Modular Helper Methods (PR #92)
- **Extracted reusable helper methods for DRY refactoring**
  - `_resolve_entity_class_from_label()` - Resolves concrete types using TypeQL `label()` function
  - `_hydrate_entity_from_data()` - Helper for role player instantiation
  - Eliminates duplicate code in `get()` and `get_by_iid()` methods

#### Generator Enhancements (PR #92)
- **Improved code generation for relations**
  - Role cardinality annotations now properly rendered
  - Docstrings and type annotations rendering improvements
  - Location: `type_bridge/generator/`

### Bug Fixes

#### Polymorphic Role Validation (PR #92)
- **Fixed polymorphic role validation and role player IIDs**
  - Correct handling of abstract types in role player resolution
  - Fixed role player IID extraction in polymorphic queries

#### Test Fixes (PR #92)
- Fixed `manager.count()` → `manager.filter().count()` in tests
- Renamed attribute types to avoid naming collisions

### Testing

- **19 new integration tests** (`tests/integration/queries/test_social_network_queries.py`)
  - Deep inheritance (3-level: Entity → Profile → Person/Organization)
  - Self-referential symmetric relations (Friendship)
  - Geographic hierarchy chains (Person → City → Country)
  - Polymorphic role players with concrete type verification
  - Multi-value attributes with `@card(0..*)`
  - Chained relations and complex filter combinations

### Key Files Modified

- `type_bridge/crud/relation/manager.py` - Polymorphic role player resolution
- `type_bridge/generator/render/` - Role cardinality and annotations rendering
- `tests/integration/queries/test_social_network_queries.py` - New comprehensive tests

## [1.2.5] - 2026-01-31

### New Features

#### IID-preferring Role Player Matching (PR #91)
- **RelationManager now prefers IID for role player matching**
  - Uses IID for precise matching when available
  - Falls back to key attribute matching when IID not set
  - Raises clear `ValueError` when neither IID nor key attributes available
  - Location: `type_bridge/crud/relation/manager.py`

**Affected Methods:** `insert()`, `put()`, `update()`, `delete()` and `*_many` variants

### Documentation

- Updated role player matching docs in `docs/SKILL.md` and `docs/api/crud.md`

## [1.2.4] - 2026-01-30

### New Features

#### TypeDB 3.8.0 Built-in Functions (PR #88)
- **Added support for TypeDB 3.8.0 built-in functions**
  - Identity functions: `iid()`, `label()`
  - Math functions: `abs_()`, `ceil()`, `floor()`, `round_()`
  - Collection functions: `len_()`, `max_()`, `min_()`
  - Location: `type_bridge/expressions/builtins.py`

**Usage Example:**
```python
from type_bridge.expressions import iid, label, abs_, ceil, floor

# Get entity IID
query = Person.manager(db).filter(iid() == "0x1a2b3c").execute()

# Use math functions
query = Person.manager(db).filter(abs_(Age.value) > 18).execute()
```

#### Unicode XID Identifier Validation (PR #88)
- **Added Unicode identifier validation for TypeDB 3.8.0 compatibility**
  - Validates identifiers follow Unicode XID_Start/XID_Continue rules
  - Ensures TypeQL identifiers are compatible with TypeDB 3.8.0
  - Clear error messages for invalid identifiers
  - Location: `type_bridge/validation.py`

### Bug Fixes

#### Value Extraction Fix (PR #88)
- **Fixed value extraction for `_Value` concepts**
  - Changed from `.as_value()` to `.get()` for proper value extraction
  - Fixes issues with function return values and aggregations
  - Location: `type_bridge/session.py`

#### Driver Initialization Warning Fix (PR #88)
- **Fixed "Failed to initialize logging" warning**
  - Suppresses fd 2 (stderr) during TypeDB driver initialization
  - Eliminates spurious warning messages on startup
  - Location: `type_bridge/typedb_driver.py`

### Documentation

- **Added AI Assistant Skill Documentation** (`docs/SKILL.md`)
  - Guidelines for using TypeBridge with AI code assistants

### Testing

- 32 new unit tests for builtin expressions
- 7 new integration tests for `iid()` and `label()` functions
- All existing tests pass with TypeDB 3.8.0-rc0
- Fixed hardcoded port 1729 in tests to use TEST_DB_ADDRESS

### CI/CD

- **Updated CI to TypeDB 3.8.0-rc0**
  - All tests now run against TypeDB 3.8.0-rc0

### Key Files Modified

- `type_bridge/expressions/builtins.py` - New built-in function expressions
- `type_bridge/session.py` - Value extraction fix
- `type_bridge/typedb_driver.py` - Driver initialization fix
- `type_bridge/validation.py` - Unicode identifier validation
- `docs/SKILL.md` - New AI assistant documentation

## [1.2.3] - 2025-12-28

### New Features

#### Enhanced FunctionQuery System (PR #84)
- **Complete TypeQL query generation for function calls**
  - FunctionQuery generates full `match let` queries with variable binding
  - Support for scalar returns, stream returns, and composite tuples
  - Query methods: `to_call()`, `to_match_let()`, `to_fetch()`, `to_query()`
  - Pagination support with limit/offset/sort
  - Location: `type_bridge/expressions/functions.py`

**Usage Example:**
```python
from myschema.functions import count_artifacts, get_neighbor_ids

# Simple count
fn = count_artifacts()
query = fn.to_query()
# → match let $integer = count-artifacts(); fetch { "integer": $integer };

# Stream with pagination
fn = get_neighbor_ids(target_id="abc-123")
query = fn.to_query(limit=10, offset=5)
```

#### Driver Injection Support (PR #86)
- **Share TypeDB driver instances across Database objects**
  - Optional `driver` parameter in `Database.__init__()`
  - Ownership tracking with `_owns_driver` flag for lifecycle control
  - Enables connection pooling and framework integration
  - Location: `type_bridge/session.py`

**Benefits:**
- Resource efficiency (share one TCP connection)
- Application-level connection pooling strategies
- Centralized driver lifecycle management
- Backwards compatible (default behavior unchanged)

**Usage Example:**
```python
driver = TypeDB.driver("localhost:1729", Credentials("admin", "password"))

db1 = Database(database="project_a", driver=driver)  # Shares driver
db2 = Database(database="project_b", driver=driver)  # Shares driver

db1.close()  # Just clears reference
db2.close()  # Just clears reference
driver.close()  # Actually closes connection
```

#### @independent Annotation Support (PR #83)
- **Standalone attributes without entity/relation owners**
  - Add `independent = True` ClassVar to attribute classes
  - Generates `@independent` annotation in TypeQL schema
  - Enables standalone attribute insertion and queries
  - Location: `type_bridge/attribute/base.py`

**Usage Example:**
```python
class Language(String):
    """Can exist without an owner."""
    independent = True

# Generated TypeQL: attribute Language @independent, value string;
```

#### @range Validation and Schema Generation (PR #82)
- **Runtime validation with database enforcement**
  - `Integer` and `Double` validate `range_constraint` ClassVar at initialization
  - Schema generation includes `@range(min..max)` annotations
  - Also generates `@regex` and `@values` annotations from ClassVars
  - Two-layer validation: Python-side (fail-fast) + TypeDB-side (enforcement)
  - Location: `type_bridge/attribute/integer.py`, `type_bridge/attribute/double.py`

**Usage Example:**
```python
from typing import ClassVar

class Age(Integer):
    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")

Age(200)  # Raises: ValueError: Age value 200 is above maximum 150

# Generated schema: attribute Age, value integer @range(0..150);
```

#### Batch IID Filtering (PR #81)
- **Django-style `iid__in` lookup for efficient batch queries**
  - Filter entities/relations by multiple IIDs in single query
  - Role player filtering: `employee__iid__in=[...]`
  - Uses flat BooleanExpr to avoid stack overflow with many IIDs
  - Location: `type_bridge/expressions/iid.py`, `type_bridge/crud/entity/manager.py`

**Usage Example:**
```python
# Fetch multiple entities by IID
persons = Person.manager(db).filter(iid__in=["0x1a2b3c", "0x4d5e6f"]).execute()

# Filter relations by role player IIDs
employments = Employment.manager(db).filter(employee__iid__in=["0x1a2b3c"]).execute()
```

**Performance:** O(1) query vs O(N) `get_by_iid()` calls.

#### TypeQL Annotation Validation (PR #86)
- **Comprehensive validation for schema annotations**
  - `@card`: Validates min ≤ max, non-negative values, detects comma syntax
  - `@regex`: Validates patterns compile as valid regex
  - `@values`: Validates at least one value, detects duplicates
  - `@range`: Validates proper `..` syntax, rejects comma/single-value
  - Clear, actionable error messages
  - Location: `type_bridge/generator/parser.py`

### Bug Fixes

#### Code Generator Fixes (PR #86)
- **Fixed relation inheritance to include keys, uniques, and cardinalities**
  - Child relations now inherit `@key`, `@unique`, and `@card` from parent
  - Ensures complete type definitions in generated code
  - Location: `type_bridge/generator/render/relations.py`

- **Fixed optional relation attributes in generated models**
  - Correct type hints with `| None` for optional fields
  - Proper default values (e.g., `timestamp: Timestamp | None = None`)
  - Respects cardinality: `@card(1)` = required, no annotation = optional
  - Location: `type_bridge/generator/render/relations.py`

- **Fixed Python literal rendering for @range annotations**
  - Range values output as numeric literals: `(1, None)` not `("1", null)`
  - Fixed type hint from `str | None` to `int | float | None`
  - Location: `type_bridge/generator/render/attributes.py`

#### Query Generation Fixes (PR #84)
- **Fixed composite variable lists in FunctionQuery**
  - Removed incorrect parentheses around variable lists
  - Correct: `match let $a, $b in func()`
  - Incorrect (previous): `match let ($a, $b) in func()`
  - Location: `type_bridge/expressions/functions.py`

#### Type Checking Fixes
- **Proper type annotations for ty type checker**
  - Use `type[Attribute]` instead of generic type
  - Use `type[Entity] | type[Relation]` for model params
  - Core library passes ty with zero warnings
  - Location: `type_bridge/crud/relation/lookup.py`, `type_bridge/schema/introspection.py`

- **Added None checks for optional attributes**
  - `assert position is not None` guards before accessing `.value`
  - Fixes pyright errors in tests
  - Location: `tests/integration/crud/test_iid_feature.py`

### Development & Tooling

#### Project Restructuring
- **Moved Python package to repository root**
  - Transitioned from monorepo (`packages/python/`) to root-level package
  - TypeScript split into separate [type-bridge-ts](https://github.com/ds1sqe/type-bridge-ts) repository
  - Simplified CI/CD configuration

#### Pre-commit Hooks
- **Comprehensive code quality automation**
  - ruff linting and formatting
  - pyright type checking
  - ty type checker for enhanced type safety
  - Location: `.pre-commit-config.yaml`

#### Type Checker Configuration
- **Configured ty rules for metaclass compatibility**
  - Overrides for tests/examples to handle metaclass-generated code
  - Rules: unknown-argument, unresolved-attribute, call-non-callable
  - Location: `pyproject.toml` [tool.ty.overrides]

### Documentation

- **FunctionQuery documentation and integration tests**
  - Comprehensive examples of query generation patterns
  - All function patterns tested (scalar, stream, composite)

- **Database driver injection guide**
  - Connection pooling examples
  - Framework integration patterns

- **Generator relation cardinality documentation**
  - Clarified `@card` behavior on relation attributes
  - Required vs optional attribute patterns

### Testing

- **1096 unit tests passing** (100% pass rate)
- **All integration tests passing**
- **0 errors with pyright and ty type checkers**

### Key Files Modified

- `type_bridge/expressions/functions.py` - Enhanced FunctionQuery
- `type_bridge/session.py` - Driver injection support
- `type_bridge/generator/parser.py` - Annotation validation
- `type_bridge/generator/render/relations.py` - Inheritance and optional attributes
- `type_bridge/generator/render/attributes.py` - Python literal rendering
- `type_bridge/crud/relation/lookup.py` - Type annotations
- `type_bridge/schema/introspection.py` - Type annotations
- `type_bridge/attribute/base.py` - @independent support
- `type_bridge/attribute/integer.py` - @range validation
- `type_bridge/attribute/double.py` - @range validation
- `.pre-commit-config.yaml` - Pre-commit hooks configuration
- `pyproject.toml` - ty type checker configuration

## [1.2.2] - 2025-12-25

### Bug Fixes

#### RelationManager IID Correlation Fix (Issue #78)
- **Fixed incorrect IID assignment in RelationManager.all()**
  - Problem: All relations were getting the same IID values for their role players instead of unique IIDs for each relation
  - Root cause: `_populate_iids` method didn't properly correlate query results to the correct relation instances
  - Solution: Capture key attribute values as variables in the query and build a lookup map for correlating results back to the correct relations
  - Impact: Critical for code relying on role player `_iid` values for deduplication or correlation
  - Example: Multiple friendships now correctly show unique IIDs for each person instead of all showing the same IIDs
  - Location: `type_bridge/session.py`, `type_bridge/crud/relation/manager.py`

### Testing
- Added 133 lines of regression tests for Issue #78
- Verifies multiple relations each get their own unique IIDs

### Key Files Modified
- `type_bridge/session.py` - Added value extraction for attribute concepts in `_extract_concept_row`
- `type_bridge/crud/relation/manager.py` - Fixed IID correlation logic (198 additions, 134 deletions)
- `tests/integration/crud/test_iid_feature.py` - Added regression tests for Issue #78

## [1.2.1] - 2025-12-25

### Bug Fixes

#### Stack Overflow Fix for __in Lookup (Issue #76)
- **Fixed TypeDB stack overflow when using `__in` lookup with many values (75+)**
  - Root cause: Deeply nested binary OR expressions `((((a or b) or c) or d)...)` caused TypeDB query planner to stack overflow
  - Solution: Use flat BooleanExpr structure for OR operations
  - Example: `manager.filter(name__in=["Alice", "Bob", ...150 names])` now works reliably
  - Location: `type_bridge/crud/entity/manager.py`, `type_bridge/crud/relation/lookup.py`, `type_bridge/expressions/boolean.py`
- **Added automatic flattening of BooleanExpr operations**
  - `BooleanExpr.and_()` and `BooleanExpr.or_()` now automatically flatten operands of the same operation type
  - Prevents deeply nested expression trees
  - Improves query compilation performance

### Testing
- Added 249 new tests for BooleanExpr flattening and __in lookup edge cases
- All tests passing

### Key Files Modified
- `type_bridge/crud/entity/manager.py` - Flat OR structure for __in lookups
- `type_bridge/crud/relation/lookup.py` - Flat OR structure for relation lookups
- `type_bridge/expressions/boolean.py` - Automatic flattening in and_() and or_()
- `tests/unit/crud/test_lookup_parser.py` - Entity lookup tests (37 tests)
- `tests/unit/crud/test_role_lookup_parser.py` - Relation lookup tests (31 tests)
- `tests/unit/expressions/test_boolean_expr.py` - BooleanExpr flattening tests (181 tests)

## [1.2.0] - 2025-12-22

### New Features

#### TypeDB 3.0 Structs Support (PR #71)
- **Parse and generate Python code for TypeDB struct types**
  - Structs are value types composed of named fields
  - Full code generator support for struct definitions
  - Location: `type_bridge/generator/`

#### Additional Annotations Support (PR #71)
- **`@range` annotation rendering** in code generator
  - Support for `@range(min..max)` constraints on attributes
  - Renders as validation metadata in generated code

#### Role Player IIDs (PR #70)
- **Populate IIDs on role player entities** when fetching relations
  - Role players now have their `_iid` field populated automatically
  - Enables direct IID-based lookups on role players

#### Generator CLI Enhancement (PR #70)
- **New `--schema-path` CLI option** for code generator
  - Specify custom path for schema file in generated output

#### Batch Delete Error Handling
- **`strict` parameter for `delete_many`** - Optional error handling for batch deletes
  - When `strict=True`, raises error if any entity not found
  - When `strict=False` (default), silently skips missing entities

### Performance Improvements

#### Batch Query Optimizations (PR #75)
- **50-100x improvement for bulk operations**
  - Batch `update_with` operations to reduce round-trips
  - Batch `_populate_iids` for efficient IID resolution
  - Significant performance gains for large datasets

#### N+1 Query Fix (PR #73)
- **Fix N+1 query problem in entity IID/type resolution**
  - Previously: 1 query per entity for IID resolution
  - Now: Single batched query for all entities
  - Major performance improvement for `all()` and `filter().execute()`

#### Batched CRUD Operations
- **Batched `update_many` and `delete_many`** operations
  - Efficient batch processing instead of per-entity queries
  - Reduced database round-trips

### Bug Fixes

- **None check in `delete_many`** - Add None check for entity in batched delete query to prevent errors when entity list contains None values

### Key Files Modified

- `type_bridge/generator/parser.py` - Struct parsing support
- `type_bridge/generator/render/` - Struct code generation
- `type_bridge/generator/__main__.py` - `--schema-path` CLI option
- `type_bridge/crud/entity/manager.py` - Batch operations, strict parameter
- `type_bridge/crud/entity/query.py` - Batch IID population
- `type_bridge/crud/relation/manager.py` - Role player IID population

## [1.1.0] - 2025-12-19

### New Features

#### Polymorphic Entity Instantiation
- **Querying a supertype now returns proper subtype instances** (Issue #65)
  - `Artifact.manager(db).all()` returns a mix of `UserStory`, `DesignAspect`, etc.
  - Each entity has the correct Python type with all subtype-specific attributes populated
  - IIDs and type information correctly extracted for each subtype

**Usage Example:**
```python
# Define type hierarchy
class Artifact(Entity):
    flags = TypeFlags(abstract=True)
    name: Name = Flag(Key)

class UserStory(Artifact):
    story_points: StoryPoints

class DesignAspect(Artifact):
    priority: Priority

# Query supertype - get proper subtypes back
artifacts = Artifact.manager(db).all()
for artifact in artifacts:
    print(type(artifact))  # UserStory, DesignAspect, etc.
    print(artifact.name)   # All have base attributes
    if isinstance(artifact, UserStory):
        print(artifact.story_points)  # Subtype-specific attributes
```

### Refactoring

#### Jinja2 Templates for Code Generation
- **Code generator now uses Jinja2 templates** instead of string concatenation
  - 6 new templates: attributes, entities, relations, functions, package_init, registry
  - Significant code reduction in render modules:
    - `registry.py`: 578 → 156 lines (-422)
    - `package.py`: 116 → 61 lines (-55)
    - `entities.py`: 234 → 198 lines (-36)

#### Typer CLI
- **Generator and migration CLIs migrated to Typer**
  - Improved help messages and command structure
  - Better argument parsing and validation

### Dependencies

- Added `jinja2>=3.1.0` - Template engine for code generation
- Added `typer>=0.15.0` - CLI framework for generator and migration tools

### Key Files Modified

- `type_bridge/crud/entity/manager.py` - Polymorphic instantiation in `get()`
- `type_bridge/crud/entity/query.py` - Polymorphic instantiation in `execute()`
- `type_bridge/crud/utils.py` - `resolve_entity_class()` utility
- `type_bridge/generator/__main__.py` - Typer CLI
- `type_bridge/generator/render/` - Jinja2 template integration
- `type_bridge/generator/templates/` - 6 new Jinja2 templates
- `type_bridge/migration/__main__.py` - Typer CLI with subcommands

## [1.0.1] - 2025-12-19

### New Features

#### TypeDB IID Support
- **Expose TypeDB Internal ID (IID) on entity and relation instances**
  - `_iid` field automatically populated when entities/relations are fetched from database
  - New `get_by_iid(iid: str)` method on EntityManager and RelationManager
  - IID populated from `get()`, `filter().execute()`, and `all()` operations

**Usage Example:**
```python
# Insert entity
person = Person(name=Name("Alice"), age=Age(30))
Person.manager(db).insert(person)

# Fetch - IID is automatically populated
fetched = Person.manager(db).get(name="Alice")
print(fetched[0]._iid)  # '0x1e00000000000000000000'

# Direct IID lookup
person = Person.manager(db).get_by_iid("0x1e00000000000000000000")
```

### Bug Fixes

- **Fixed relation IID assignment order** - Set `_iid` after role player assignments to prevent Pydantic revalidation from resetting the value

### Key Files Modified

- `type_bridge/session.py` - IID extraction functions
- `type_bridge/crud/entity/manager.py` - `get_by_iid()` method
- `type_bridge/crud/entity/query.py` - IID extraction from results
- `type_bridge/crud/relation/manager.py` - `get_by_iid()` method
- `type_bridge/crud/relation/query.py` - IID extraction from results

## [1.0.0] - 2025-12-15

### New Features

#### Django-style Migration System
- **Complete migration framework for TypeDB schema evolution**
  - Auto-generate migrations from Python model changes
  - Apply and rollback migrations with transaction safety
  - Track migration state in TypeDB database
  - CLI commands: `type-bridge makemigrations`, `type-bridge migrate`
  - Location: `type_bridge/migration/` (2872 lines)

- **Migration Operations**
  - `AddAttribute` - Add new attribute types
  - `AddEntity` - Add new entity types
  - `AddRelation` - Add new relation types with roles
  - `AddOwnership` - Add attribute ownership to types
  - `AddRolePlayer` - Add role players to relations
  - `RemoveAttribute`, `RemoveEntity`, `RemoveRelation` - Remove types
  - `RunTypeQL` - Execute raw TypeQL for custom migrations
  - All operations support forward and rollback
  - Location: `type_bridge/migration/operations.py`

- **Migration Generator**
  - Auto-detect schema changes by comparing Python models to database
  - Generate migration files with operations
  - Support for incremental migrations (detect only changes)
  - Location: `type_bridge/migration/generator.py`

- **Migration Executor**
  - Apply pending migrations in order
  - Rollback migrations in reverse order
  - Dry-run mode for previewing changes
  - `sqlmigrate` for viewing generated TypeQL
  - Location: `type_bridge/migration/executor.py`

- **Migration State Tracking**
  - Track applied migrations in TypeDB
  - Location: `type_bridge/migration/state.py`

- **Model Registry**
  - Register models for migration tracking
  - Auto-discover models from modules
  - Location: `type_bridge/migration/registry.py`

#### Schema Introspection
- **Query existing schema from TypeDB database**
  - Introspect entities, relations, attributes, and ownerships
  - TypeDB 3.x compatible queries
  - Model-aware introspection for efficient checking
  - Location: `type_bridge/schema/introspection.py` (527 lines)

#### Breaking Change Detection
- **Analyze schema changes for safety**
  - Detect breaking vs safe changes
  - Categories: BREAKING, SAFE, WARNING
  - Check role player narrowing, type removal, etc.
  - Location: `type_bridge/schema/breaking.py` (411 lines)

#### Enhanced Schema Diff
- **Improved schema comparison**
  - Compare by TypeDB type name instead of Python object identity
  - Detect modified entities, relations, and role players
  - Track attribute and ownership changes
  - Location: `type_bridge/schema/info.py`, `type_bridge/schema/diff.py`

### Bug Fixes

#### TypeDB 3.x Compatibility
- **Fixed schema comparison using Python object identity instead of type name**
  - `SchemaInfo.compare()` now correctly matches entities/relations by type name

- **Fixed migration executor to execute operations separately**
  - TypeDB 3.x doesn't allow multiple `define` blocks in single query

- **Fixed schema introspection to query ownership from schema definition**
  - Uses `match {type_name} owns $a;` for schema queries

- **Fixed migration state tracking for TypeDB 3.x @key semantics**
  - Uses composite key (`migration_id`) instead of dual `@key` attributes

### Testing

- **All 391 integration tests passing**
- Added comprehensive migration test suite
- Added role player diff tests
- Added schema introspection tests

### Key Files Added

- `type_bridge/migration/__init__.py` - Migration module exports
- `type_bridge/migration/__main__.py` - CLI commands
- `type_bridge/migration/base.py` - Migration base class
- `type_bridge/migration/operations.py` - Migration operations
- `type_bridge/migration/generator.py` - Auto-generation
- `type_bridge/migration/executor.py` - Apply/rollback
- `type_bridge/migration/loader.py` - Load migration files
- `type_bridge/migration/state.py` - State tracking
- `type_bridge/migration/registry.py` - Model registry
- `type_bridge/schema/introspection.py` - Schema introspection
- `type_bridge/schema/breaking.py` - Breaking change detection
- `tests/integration/migration/` - Migration integration tests
- `tests/integration/schema/test_role_player_diff.py` - Role player tests

### Usage Example

```python
from type_bridge.migration import MigrationGenerator, MigrationExecutor
from type_bridge import Database

db = Database("localhost:1729", "mydb")

# Generate migration from models
generator = MigrationGenerator(db, "./migrations")
generator.generate(models=[Person, Company], name="initial")

# Apply migrations
executor = MigrationExecutor(db, "./migrations")
results = executor.migrate()

# Rollback
executor.rollback()
```

### CLI Usage

```bash
# Generate migrations
type-bridge makemigrations ./migrations --models myapp.models

# Apply migrations
type-bridge migrate ./migrations

# Show migration status
type-bridge showmigrations ./migrations

# Preview TypeQL
type-bridge sqlmigrate ./migrations 0001_initial
```

## [0.9.4] - 2025-12-12

### New Features

#### Debug Logging Support (Issue #43)
- **Added comprehensive logging throughout TypeBridge using Python's standard `logging` module**
  - See generated TQL queries during CRUD operations
  - Hierarchical logger structure for fine-grained control
  - Documentation: `docs/api/logging.md`

### Bug Fixes

#### Type Safety Improvements
- Fixed pyright errors for `TransactionType.name` access in session.py
  - Added `_tx_type_name()` helper function for type-safe transaction type logging

### Documentation

- Added `docs/api/logging.md` - Comprehensive logging configuration guide
- Updated `docs/DEVELOPMENT.md` with logging section

### Key Files Modified

- `type_bridge/session.py` - Fixed type errors, added logging
- `type_bridge/query.py` - Added logging
- `type_bridge/validation.py` - Added logging
- `type_bridge/schema/manager.py` - Added logging
- `type_bridge/schema/migration.py` - Added logging
- `type_bridge/crud/entity/manager.py` - Added logging
- `type_bridge/crud/entity/query.py` - Added logging
- `type_bridge/crud/entity/group_by.py` - Added logging
- `type_bridge/crud/relation/manager.py` - Added logging
- `type_bridge/crud/relation/query.py` - Added logging
- `type_bridge/crud/relation/group_by.py` - Added logging
- `type_bridge/generator/__main__.py` - Added logging
- `type_bridge/generator/render/attributes.py` - Added logging
- `type_bridge/generator/render/entities.py` - Added logging
- `type_bridge/generator/render/relations.py` - Added logging
- `type_bridge/models/entity.py` - Added logging
- `type_bridge/models/relation.py` - Added logging
- `docs/api/logging.md` - New documentation
- `docs/DEVELOPMENT.md` - Added logging section

## [0.9.3] - 2025-12-12

### Documentation

#### @key Attribute Requirement for update() (Issue #45)
- **Added prominent documentation explaining that `update()` requires @key attributes**
  - New section "Important: @key Attributes Required for update()" in CRUD docs
  - Clear examples showing proper `@key` attribute definition
  - Error scenarios documented: no `@key` defined, `@key` value is `None`
  - Guidance for UUID/ID fields as `@key` attributes
  - Cross-reference to Exception Handling section
  - Location: `docs/api/crud.md` (lines 369-413)

### Key Files Modified

- `docs/api/crud.md` - Added @key requirement documentation in Update Operations section

## [0.9.2] - 2025-12-12

### New Features

#### KeyAttributeError Exception (Issue #44)
- **Added `KeyAttributeError` exception for @key validation failures**
  - Raised when @key attribute is None during update/delete
  - Raised when no @key attributes are defined on entity
  - Structured attributes: `entity_type`, `operation`, `field_name`, `all_fields`
  - Helpful error messages with hints for fixing issues
  - Inherits from `ValueError` for backward compatibility
  - Location: `type_bridge/crud/exceptions.py`

### Testing

- Added 7 unit tests for `KeyAttributeError`
- **813 unit tests** passing

### Key Files Added/Modified

- `type_bridge/crud/exceptions.py` - Added `KeyAttributeError` class
- `type_bridge/crud/__init__.py` - Export `KeyAttributeError`
- `type_bridge/__init__.py` - Export `KeyAttributeError` in public API
- `type_bridge/crud/entity/manager.py` - Use `KeyAttributeError`
- `type_bridge/crud/entity/query.py` - Use `KeyAttributeError`
- `tests/unit/exceptions/test_exceptions.py` - Added KeyAttributeError tests

### Usage Example

```python
from type_bridge import KeyAttributeError

try:
    manager.update(entity_with_none_key)
except KeyAttributeError as e:
    print(f"Entity: {e.entity_type}")
    print(f"Operation: {e.operation}")
    print(f"Field: {e.field_name}")
```

## [0.9.1] - 2025-12-11

### New Features

#### Enhanced Code Generator (PR #53)

- **Registry Module** - Generates `registry.py` with schema metadata as Python dictionaries
  - Entity/relation attributes, roles, and inheritance info
  - JSON Schema fragments for validation
  - Convenience lookup functions
  - Schema hash for change detection
  - Location: `type_bridge/generator/render/registry.py`

- **Enhanced Function Generation** - Generic `FunctionCallExpr[T]` with precise return type hints
  - Support for all TypeDB function variations: stream, scalar, tuple, optional returns
  - Added `bool` to type mapping
  - Improved docstrings with return type documentation
  - Location: `type_bridge/generator/render/functions.py`

- **TypeQL Parser Enhancements**
  - `@independent` attribute flag support
  - `@range(min..max)` constraint parsing (integers, floats, dates, datetimes, open-ended)
  - `@card` on `plays` declarations with inheritance
  - `@card` on `relates` declarations
  - `//` C-style comments alongside `#` comments
  - Comment annotations parsing (`@prefix`, `@tags`, etc.)
  - Location: `type_bridge/generator/parser.py`, `type_bridge/generator/typeql.lark`

### Testing

- Added comprehensive generator unit tests for annotations, functions, and registry
- **1,268 total tests** passing

### Key Files Added/Modified

- `type_bridge/generator/render/registry.py` - Registry module generation (new)
- `type_bridge/generator/annotations.py` - Annotation parsing (new)
- `type_bridge/generator/render/functions.py` - Enhanced function generation
- `type_bridge/generator/parser.py` - Parser enhancements
- `type_bridge/generator/typeql.lark` - Grammar updates
- `type_bridge/generator/models.py` - New annotation models
- `type_bridge/expressions/functions.py` - Generic FunctionCallExpr
- `docs/api/generator.md` - Updated documentation

### Contributors

- @CaliLuke - Generator enhancements

## [0.9.0] - 2025-12-11

### New Features

#### Type-Safe Role Player Expressions (PR #42)
- **Type-safe role player field expressions**
  - Access role player attributes with type safety: `Employment.employee.age.gte(Age(30))`
  - `RoleRef` class for class-level role access
  - `RolePlayerFieldRef` for type-safe attribute access
  - `RolePlayerNumericFieldRef` for numeric comparison methods
  - `RolePlayerStringFieldRef` for string-specific methods (contains, like, regex)
  - Location: `type_bridge/fields/role.py`

- **Django-style role-player lookup filters**
  - Filter by role player attributes: `filter(employee__age__gt=30)`
  - Support for comparison operators: `__eq`, `__gt`, `__lt`, `__gte`, `__lte`
  - Support for string operators: `__contains`, `__like`, `__regex`
  - Location: `type_bridge/crud/relation/lookup.py`

- **Added `order_by()` method to EntityQuery and RelationQuery**
  - Sort results by entity/relation attributes or role player attributes
  - Supports ascending (default) and descending (`-field`) order
  - Role-player sorting: `order_by('employee__age', '-salary')`
  - Location: `type_bridge/crud/entity/query.py`, `type_bridge/crud/relation/query.py`

- **Added public `Relation.get_roles()` method**
  - Clean API for accessing relation roles without internal `_roles` access
  - Returns: `dict[str, Role]` mapping role names to Role instances
  - Location: `type_bridge/models/relation.py`

### Bug Fixes

- **Fixed `RelationQuery.filter()` to support Django-style kwargs**
  - Chained `.filter()` calls now properly support `**filters` parameter
  - Location: `type_bridge/crud/relation/query.py`

### Testing

- Added `TestCombinedWithPagination` integration tests
- Added `TestRoleMultiAttributeAccess` unit tests
- All type checks pass without suppressions (uses `isinstance` for type narrowing)
- **806 unit tests** + **377 integration tests** = **1,183 total tests**

### Key Files Added/Modified

- `type_bridge/fields/role.py` - RoleRef and RolePlayerFieldRef classes
- `type_bridge/crud/relation/lookup.py` - Django-style lookup parser
- `type_bridge/crud/relation/query.py` - RelationQuery with filter and order_by
- `type_bridge/crud/entity/query.py` - EntityQuery with order_by
- `type_bridge/models/relation.py` - Added get_roles() method
- `type_bridge/generator/render/relations.py` - Updated generated code patterns
- `tests/unit/fields/test_role_ref.py` - RoleRef unit tests
- `tests/integration/queries/test_role_field_expressions.py` - Integration tests

### Usage Examples

```python
from type_bridge import Relation, Role, Entity, TypeFlags, String, Integer

class Age(Integer):
    pass

class Name(String):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None

class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)

# Type-safe role player expression
results = Employment.manager(db).filter(
    Employment.employee.age.gte(Age(30))
).execute()

# Django-style lookup
results = Employment.manager(db).filter(employee__age__gt=30).execute()

# Combined with sorting and pagination
results = (
    Employment.manager(db)
    .filter(Employment.employee.age.gte(Age(25)), salary__gte=80000)
    .order_by("employee__age", "-salary")
    .limit(10)
    .execute()
)
```

## [0.8.1] - 2025-12-11

### New Features

#### TypeDB Function Support in Code Generator
- **Parse TypeQL `fun` declarations with parameters and return types**
  - Generate Python function wrappers that return `FunctionCallExpr` objects
  - Auto-generate `functions.py` module when functions are defined in schema
  - Support for all TypeDB data types (string, integer, date, datetime, double, boolean, decimal, duration)
  - Location: `type_bridge/generator/render/functions.py`

- **Added `FunctionCallExpr` class for building function calls in queries**
  - New expression type for calling TypeDB functions
  - Location: `type_bridge/expressions/functions.py`

#### Lark-based Parser
- **Migrated from regex-based parser to robust Lark grammar**
  - Added formal grammar file (`typeql.lark`) for better maintainability
  - Improved handling of edge cases (whitespace, optional clauses)
  - Better support for TypeDB 3.0 features
  - Location: `type_bridge/generator/typeql.lark`, `type_bridge/generator/parser.py`

### Bug Fixes

#### Parser Grammar Fixes
- **Fixed parsing of `sub` and `@abstract` in entity/relation bodies**
  - Now correctly handles patterns like `entity page @abstract, sub content,`
  - Supports `sub` appearing anywhere in the type definition, not just at the start
- **Fixed parsing of `@card` annotations on `plays` and `relates` statements**
  - Now correctly parses `plays posting:post @card(1)` and `relates subject @card(1)`

### Testing

- Added 136 new unit tests for function parsing and rendering
- All 808 tests passing (520 unit + 288 integration)

### Key Files Added/Modified

- `type_bridge/generator/typeql.lark` - Formal TypeQL grammar (75 lines)
- `type_bridge/generator/parser.py` - Rewritten with Lark integration
- `type_bridge/generator/models.py` - Added `FunctionSpec`, `ParameterSpec`
- `type_bridge/generator/render/functions.py` - Function code generation
- `type_bridge/expressions/functions.py` - `FunctionCallExpr` class
- `type_bridge/expressions/__init__.py` - Export `FunctionCallExpr`
- `type_bridge/generator/__init__.py` - Expose function parsing API
- `tests/unit/generator/test_functions.py` - Function unit tests

## [0.8.0] - 2025-12-11

### New Features

#### Code Generator (TypeQL → Python)
- **Added `type_bridge.generator` module for generating Python models from TypeQL schema files**
  - Eliminates manual synchronization between `.tql` schemas and Python code
  - Write schema once in TypeQL, generate type-safe Python models automatically
  - Location: `type_bridge/generator/`

- **CLI interface for code generation**
  - Usage: `python -m type_bridge.generator schema.tql -o ./models/`
  - Options: `--version`, `--no-copy-schema`, `--implicit-keys`
  - Location: `type_bridge/generator/__main__.py`

- **Programmatic API**
  - `generate_models(schema, output_dir)` - Main generation function
  - `parse_tql_schema(schema_content)` - Parse TypeQL to intermediate representation
  - Location: `type_bridge/generator/__init__.py`

- **Full TypeDB 3.x schema support**
  - All value types: string, integer, double, decimal, boolean, date, datetime, datetime-tz, duration
  - Entity and relation inheritance (`sub` declarations)
  - Abstract types (`@abstract`)
  - Attribute constraints: `@key`, `@unique`, `@card`, `@regex`, `@values`
  - Role definitions and `plays` declarations
  - Comment annotations for customization (`# @prefix:`, `# @internal`)

- **Generated package structure**
  ```
  output_dir/
  ├── __init__.py      # Package exports, SCHEMA_VERSION, schema_text()
  ├── attributes.py    # Attribute class definitions
  ├── entities.py      # Entity class definitions
  ├── relations.py     # Relation class definitions
  └── schema.tql       # Copy of original schema
  ```

### Bug Fixes

#### Optional Attribute Update Fix
- **Fixed silent update failures for optional attributes** (Resolves [#47](https://github.com/ds1sqe/type-bridge/issues/47))
  - Previously: Updating an entity with an optional attribute set to `None` could silently fail
  - Now: Properly handles `None` values in update operations
  - Location: `type_bridge/crud/entity/manager.py`

### Testing

- **Added comprehensive generator test suite**
  - 62 unit tests covering parser, naming utilities, and renderers
  - 18 integration tests verifying generated code imports and executes correctly
  - Test fixtures with complex TypeQL schemas
  - Location: `tests/unit/generator/`, `tests/integration/generator/`

- **Expanded lookup filter tests**
  - Additional test coverage for lookup filter parsing and execution
  - Location: `tests/unit/crud/test_lookup_parser.py`, `tests/integration/queries/test_lookup_filters.py`

### Documentation

- **Added generator documentation**
  - Complete API reference and usage guide
  - CLI reference and examples
  - Supported TypeQL features and cardinality mapping
  - Best practices for generated code management
  - Location: `docs/api/generator.md`

- **Updated project documentation**
  - Added generator to project structure in CLAUDE.md
  - Updated README.md with code generator feature

### Key Files Added/Modified

- `type_bridge/generator/__init__.py` - Public API: `generate_models()`, `parse_tql_schema()`
- `type_bridge/generator/__main__.py` - CLI interface
- `type_bridge/generator/models.py` - `ParsedSchema`, `AttributeSpec`, `EntitySpec`, `RelationSpec`
- `type_bridge/generator/parser.py` - TypeQL parser with inheritance resolution
- `type_bridge/generator/naming.py` - kebab-case → PascalCase/snake_case utilities
- `type_bridge/generator/render/` - Code generation renderers (attributes, entities, relations, package)
- `type_bridge/crud/entity/manager.py` - Optional attribute update fix
- `docs/api/generator.md` - Generator documentation

### Usage Examples

#### CLI Usage
```bash
# Generate models from a schema file
python -m type_bridge.generator schema.tql -o ./myapp/models/

# With options
python -m type_bridge.generator schema.tql \
    --output ./myapp/models/ \
    --version 2.0.0 \
    --implicit-keys id
```

#### Programmatic Usage
```python
from type_bridge.generator import generate_models

# From a file path
generate_models("schema.tql", "./myapp/models/")

# From schema text
schema = """
define
entity person, owns name @key;
attribute name, value string;
"""
generate_models(schema, "./myapp/models/")
```

#### Using Generated Models
```python
from myapp.models import attributes, entities, relations
from myapp.models import SCHEMA_VERSION, schema_text

# Access generated classes
person = entities.Person(name=attributes.Name("Alice"))

# Get schema version
print(SCHEMA_VERSION)  # "1.0.0"
```

## [0.7.2] - 2025-12-10

### Breaking Changes

#### New Exceptions for Delete Operations
- **Added `EntityNotFoundError`** - Raised when deleting an entity that doesn't exist
  - Now raised for both keyed and keyless entities when no match is found
  - Previously: keyed entities silently succeeded, keyless raised `ValueError`
  - Subclass of `LookupError`

- **Added `RelationNotFoundError`** - Raised when deleting a relation that doesn't exist
  - Raised when relation with given role players is not found
  - Previously: silently succeeded
  - Subclass of `LookupError`

- **Added `NotUniqueError`** - Raised when keyless entity matches multiple records
  - Replaces `ValueError` for multiple match scenarios
  - Subclass of `ValueError`
  - Suggestion to use `filter().delete()` for bulk deletion

### Migration Guide

```python
# Handling non-existent entity deletion (NEW in v0.7.2)
from type_bridge import EntityNotFoundError, RelationNotFoundError, NotUniqueError

# Entity with @key that doesn't exist
try:
    manager.delete(nonexistent_entity)
except EntityNotFoundError:
    print("Entity was already deleted or never existed")

# Relation that doesn't exist
try:
    relation_manager.delete(nonexistent_relation)
except RelationNotFoundError:
    print("Relation was already deleted or never existed")

# Entity without @key matching multiple records
try:
    manager.delete(keyless_entity)
except NotUniqueError:
    print("Multiple entities matched - use filter().delete() for bulk deletion")

# Bulk delete with __in (migrating from v0.7.0)
# OLD: manager.delete_many(name__in=["Alice", "Bob"])
# NEW:
count = manager.filter(name__in=["Alice", "Bob"]).delete()
```

### Key Files Modified

- `type_bridge/crud/exceptions.py` - **NEW** - Exception classes
- `type_bridge/crud/__init__.py` - Export exceptions
- `type_bridge/crud/entity/manager.py` - Add existence check before delete
- `type_bridge/crud/relation/manager.py` - Add existence check before delete
- `type_bridge/__init__.py` - Export exceptions

## [0.7.1] - 2025-12-09

### Breaking Changes

#### Delete API Refactored to Instance-Based Pattern
- **Changed `EntityManager.delete()` signature**
  - Old: `delete(**filters) -> int` (filter-based, returns count)
  - New: `delete(entity: E) -> E` (instance-based, returns deleted entity)
  - Uses `@key` attributes to identify entity (same pattern as `update()`)
  - Related: [Issue #37](https://github.com/ds1sqe/type-bridge/issues/37)

- **Changed `EntityManager.delete_many()` signature**
  - Old: `delete_many(**filters) -> int` (filter-based with `__in` support)
  - New: `delete_many(entities: list[E]) -> list[E]` (instance list, returns list)

- **Changed `RelationManager.delete()` and `delete_many()` similarly**
  - Uses role players' `@key` attributes to identify the relation
  - Each role player is matched by their `@key` attribute

### New Features

#### Instance Delete Methods
- **Added `Entity.delete(connection)` instance method**
  - Delete entity directly: `alice.delete(db)`
  - Returns self for chaining
  - Location: `type_bridge/models/entity.py`

- **Added `Relation.delete(connection)` instance method**
  - Delete relation directly: `employment.delete(db)`
  - Returns self for chaining
  - Location: `type_bridge/models/relation.py`

#### Fallback for Entities Without @key
- **Entities without `@key` can still be deleted** if they match exactly 1 record
  - Matches by ALL non-None attributes
  - Raises `ValueError` if 0 or >1 matches found
  - Provides safer delete behavior than filter-based approach

### Migration Guide

```python
# OLD (v0.7.0): Filter-based deletion
deleted_count = manager.delete(name="Alice")  # Returns int

# NEW (v0.7.1): Instance-based deletion
alice = manager.get(name="Alice")[0]
deleted = manager.delete(alice)  # Returns Alice instance

# OR use instance method
alice.delete(db)  # Returns alice

# For filter-based deletion, use filter().delete()
count = manager.filter(name__in=["Alice", "Bob"]).delete()  # Still returns int
count = manager.filter(Age.gt(Age(65))).delete()  # Expression filters
```

### Key Files Modified

- `type_bridge/crud/entity/manager.py` - Refactored `delete()`, `delete_many()`
- `type_bridge/crud/relation/manager.py` - Refactored `delete()`, `delete_many()`
- `type_bridge/models/entity.py` - Added `delete()` instance method
- `type_bridge/models/relation.py` - Added `delete()` instance method

## [0.7.0] - 2025-12-08

### 🚀 New Features

#### TransactionContext for Shared Operations
- **Added `TransactionContext` class for sharing transactions across operations**
  - Multiple managers can share a single transaction
  - Auto-commit on context exit, rollback on exception
  - Location: `type_bridge/session.py`

```python
with db.transaction(TransactionType.WRITE) as tx:
    person_mgr = Person.manager(tx)     # reuses tx
    artifact_mgr = Artifact.manager(tx)  # same tx
    # ... operations commit together
```

#### Unified Connection Type
- **Added `Connection` type alias for flexible connection handling**
  - `Connection = Database | Transaction | TransactionContext`
  - All managers accept any Connection type
  - `ConnectionExecutor` handles transaction reuse internally
  - Location: `type_bridge/session.py`

#### Entity Dict Helpers
- **Added `Entity.to_dict()` for serialization**
  - Unwraps Attribute instances to `.value`
  - Supports `include`, `exclude`, `by_alias`, `exclude_unset` options
- **Added `Entity.from_dict()` for deserialization**
  - Optional `field_mapping` for external key names
  - `strict=False` mode to ignore unknown fields
  - Location: `type_bridge/models/entity.py`

```python
person.to_dict()  # {'name': 'Alice', 'age': 30}
Person.from_dict(payload, field_mapping={"display-id": "display_id"})
```

#### Django-style Lookup Filters
- **Added lookup suffix operators to `filter()`**
  - `__contains`, `__startswith`, `__endswith`, `__regex` for strings
  - `__gt`, `__gte`, `__lt`, `__lte` for comparisons
  - `__in` for disjunction (multiple values)
  - `__isnull` for null checks
  - Location: `type_bridge/crud/entity/manager.py`

```python
person_manager.filter(name__startswith="Al", age__gt=30).execute()
person_manager.filter(status__in=["active", "pending"]).execute()
```

#### Bulk Operations
- **Added `EntityManager.update_many()`** - Update multiple entities in one transaction
- **Added `EntityManager.delete_many()`** - Bulk delete with `__in` filter support

### 📚 Documentation

- **Comprehensive documentation update**
  - Fixed broken example paths in README.md
  - Added Connection types documentation to docs/api/crud.md
  - Added TransactionContext usage examples
  - Added lookup filter documentation with TypeQL mappings
  - Added dict helpers documentation to docs/api/entities.md

### 🔧 Maintenance

- Fixed version mismatch between pyproject.toml and __init__.py
- Added `typings/` stubs for `isodate` and `typedb.driver`

### 📦 Key Files Modified

- `type_bridge/session.py` - Added TransactionContext, Connection, ConnectionExecutor
- `type_bridge/models/entity.py` - Added to_dict(), from_dict()
- `type_bridge/crud/entity/manager.py` - Added lookup filters, update_many, delete_many
- `type_bridge/crud/relation/manager.py` - Unified connection handling
- `docs/api/crud.md` - New sections for transactions, lookups, bulk ops
- `docs/api/entities.md` - Dict helpers documentation

## [0.6.4] - 2025-12-04

### 🚀 New Features

#### CRUD PUT Operations (Idempotent Insert)
- **Added `EntityManager.put()` for idempotent entity insertion**
  - Inserts entity only if it doesn't already exist
  - Safe to call multiple times without creating duplicates
  - Uses TypeQL's PUT clause for atomic match-or-insert semantics
  - Location: `type_bridge/crud/entity/manager.py`

- **Added `EntityManager.put_many()` for bulk idempotent insertion**
  - All-or-nothing semantics: entire pattern must match or all is inserted
  - Efficient batch operations for data synchronization

- **Added `RelationManager.put()` for idempotent relation insertion**
  - Same PUT semantics for relations with role players
  - Prevents duplicate relationships
  - Location: `type_bridge/crud/relation/manager.py`

- **Added `RelationManager.put_many()` for bulk relation PUT**
  - Batch idempotent insertion for relations

#### Use Cases
- Data import scripts (safe re-runs)
- Ensuring reference data exists
- Synchronization with external systems
- Idempotent API endpoints

### 📚 Documentation

- **Updated `docs/api/crud.md`** with PUT operations section
  - Comparison table: INSERT vs PUT behavior
  - All-or-nothing semantics explanation
  - Usage examples for entities and relations

- **Added `examples/basic/crud_08_put.py`** tutorial
  - Demonstrates PUT vs INSERT differences
  - Shows idempotent behavior patterns

### 🧪 Testing

- **Added entity PUT integration tests** (`tests/integration/crud/entities/test_put.py`)
  - Single put, bulk put_many
  - Idempotency verification
  - All-or-nothing behavior

- **Added relation PUT integration tests** (`tests/integration/crud/relations/test_put.py`)
  - Relation put operations
  - Role player handling

### 📦 Key Files Modified

- `type_bridge/crud/entity/manager.py` - Added `put()`, `put_many()`
- `type_bridge/crud/relation/manager.py` - Added `put()`, `put_many()`
- `docs/api/crud.md` - PUT documentation
- `examples/basic/crud_08_put.py` - New tutorial
- `tests/integration/crud/entities/test_put.py` - Entity PUT tests
- `tests/integration/crud/relations/test_put.py` - Relation PUT tests
- `README.md` - Updated features

### 💡 Usage Examples

```python
# Single PUT (idempotent insert)
alice = Person(name=Name("Alice"), age=Age(30))
person_manager.put(alice)
person_manager.put(alice)  # No duplicate created

# Bulk PUT
persons = [Person(name=Name("Bob")), Person(name=Name("Carol"))]
person_manager.put_many(persons)
person_manager.put_many(persons)  # No duplicates

# Relation PUT
employment = Employment(employee=alice, employer=techcorp)
employment_manager.put(employment)
```

## [0.6.3] - 2025-12-04

### 🚀 New Features

#### Multi-Player Roles
- **Added `Role.multi()` for roles playable by multiple entity types**
  - Syntax: `origin: Role[Document | Email] = Role.multi("origin", Document, Email)`
  - Eliminates need for artificial supertype hierarchies
  - Generates multiple `plays` declarations in TypeQL schema
  - Location: `type_bridge/models/role.py`
- **Full CRUD support for multi-role relations**
  - Filter by specific player type: `manager.get(origin=doc)`
  - Chainable operations work with multi-roles
  - Batch insert with mixed player types supported
- **Runtime validation**: TypeError if wrong player type assigned
- **Pydantic integration**: Union types provide IDE/type-checker support

#### TypeDB 3.7 Compatibility
- **Verified compatibility with TypeDB 3.7.0-rc0**
- Updated documentation to reflect tested TypeDB version

### 🐛 Bug Fixes

#### Inherited Attribute Filter Bug
- **Fixed filters on inherited attributes being silently ignored**
  - Root cause: `get_owned_attributes()` was used instead of `get_all_attributes()` in filter operations
  - Affected methods: `EntityManager.delete()`, `EntityQuery.delete()`, `QueryBuilder.match_entity()`
  - Example: `Dog.manager(db).get(name=LivingName("Buddy"))` now works when `name` is inherited from parent `Living` class
  - Location: `type_bridge/crud/entity/query.py:215`, `type_bridge/crud/entity/manager.py:219`, `type_bridge/query.py:193`
- **Impact**: Dictionary-based filters (`get()`, `delete()`) now correctly handle inherited attributes in subtype queries

### 🧪 Testing

#### Multi-Role Tests
- **Multi-role integration tests** (`tests/integration/crud/relations/test_multi_role.py`) - 37 tests
  - Insert relations with different role player types
  - Filter by multi-role players
  - Delete filtered by multi-role
  - Chainable `update_with` operations
  - Multi-role with 3+ entity types
- **Multi-role unit tests** (`tests/unit/core/test_multi_role_players.py`)
  - Role.multi() API validation
  - Type safety and runtime validation

#### Inherited Attribute Filter Tests
- **Created unit tests** (`tests/unit/core/test_inherited_attribute_filter.py`) with 9 tests:
  - `get_owned_attributes()` vs `get_all_attributes()` behavior verification
  - `QueryBuilder.match_entity()` with inherited/owned/mixed attribute filters
  - Deep inheritance chain (grandparent → parent → child) attribute access
- **Created integration tests** (`tests/integration/crud/test_inherited_attribute_filter.py`) with 5 tests:
  - `get()` with inherited key attribute
  - `delete()` with inherited attribute filter
  - Combined inherited + owned attribute filters

### 📦 Key Files Modified

- `type_bridge/models/role.py` - Added `Role.multi()` and multi-player support
- `type_bridge/schema/info.py` - Generate multiple `plays` declarations
- `type_bridge/crud/relation/manager.py` - Multi-role query handling
- `type_bridge/crud/relation/query.py` - Multi-role filtering support
- `docs/api/relations.md` - Added "Multi-player Roles" documentation
- `type_bridge/crud/entity/query.py` - Fixed `delete()` to use `get_all_attributes()`
- `type_bridge/crud/entity/manager.py` - Fixed `delete()` to use `get_all_attributes()`
- `type_bridge/query.py` - Fixed `match_entity()` to use `get_all_attributes()`
- `tests/integration/crud/relations/test_multi_role.py` - New multi-role test suite (37 tests)
- `tests/unit/core/test_multi_role_players.py` - New multi-role unit tests

## [0.6.0] - 2025-11-24

### 🚀 New Features

#### Chainable Delete and Update Operations
- **Added `EntityQuery.delete()` for chainable deletion**
  - Delete entities after complex filtering: `manager.filter(Age.gt(Age(65))).delete()`
  - Builds TypeQL delete query from both dict-based and expression-based filters
  - Single atomic transaction with automatic rollback on error
  - Returns count of deleted entities (0 if no matches)
  - Location: `type_bridge/crud.py:626-676`

- **Added `EntityQuery.update_with(func)` for functional bulk updates**
  - Update multiple entities using lambda or named functions
  - Example: `manager.filter(Age.gt(Age(30))).update_with(lambda p: setattr(p, 'age', Age(p.age.value + 1)))`
  - Fetches matching entities, applies function, updates all in single transaction
  - Returns list of updated entities (empty list if no matches)
  - Error handling: Stops immediately and raises error if function fails on any entity
  - All updates in single atomic transaction (all-or-nothing)
  - Location: `type_bridge/crud.py:678-730`

- **Helper methods added**
  - `_build_update_query(entity)`: Builds TypeQL update query for single entity
  - `_is_multi_value_attribute(flags)`: Checks attribute cardinality
  - Reuses existing EntityManager logic for consistency

#### Benefits
1. **Chainable API**: Natural method chaining for complex operations
2. **Type-safe**: Full integration with expression-based filtering
3. **Atomic transactions**: All operations are all-or-nothing
4. **Functional updates**: Clean lambda/function-based bulk updates
5. **Consistent API**: Works seamlessly with existing filter() method

#### AttributeFlags Configuration for Attribute Type Names
- **Added `AttributeFlags.name` field**
  - Explicitly override attribute type name: `flags = AttributeFlags(name="person_name")`
  - Use case: Interop with existing TypeDB schemas, legacy naming conventions
  - Location: `type_bridge/attribute/flags.py:229`

- **Added `AttributeFlags.case` field**
  - Apply case formatting to attribute type names: `flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)`
  - Supports: CLASS_NAME (default), LOWERCASE, SNAKE_CASE, KEBAB_CASE
  - Use case: Consistent naming conventions across large schemas
  - Location: `type_bridge/attribute/flags.py:230`

- **Updated `Attribute.__init_subclass__` to use AttributeFlags**
  - Priority: flags.name > attr_name > flags.case > class.case > default CLASS_NAME
  - Respects both explicit name and case formatting
  - Location: `type_bridge/attribute/base.py:108-126`

#### Benefits
1. **Flexible naming**: Support for legacy schemas and naming conventions
2. **Consistent API**: Mirrors TypeFlags.name pattern for entities/relations
3. **Migration friendly**: Easier interop with existing TypeDB databases
4. **Developer choice**: Explicit name or automatic case formatting

### 🏗️ Refactoring

#### Modularized CRUD Operations
- **Refactored monolithic `crud.py` (3008 lines) into modular structure**
  - Split into 11 focused modules under `crud/` directory
  - Entity operations: `crud/entity/` with manager, query, and group_by modules
  - Relation operations: `crud/relation/` with manager, query, and group_by modules
  - Shared utilities: `crud/utils.py` for `format_value` and `is_multi_value_attribute`
  - Base definitions: `crud/base.py` for type variables
- **Benefits**:
  - Eliminated code duplication (shared utilities now have single implementations)
  - Improved maintainability (files are now 200-800 lines each)
  - Better code organization and discoverability
  - Preserved backward compatibility (all imports still work)
- **Impact**: No breaking changes, all existing code continues to work

#### Modularized Models
- **Previously refactored `models.py` into modular structure**
  - Split into `models/` directory with base, entity, relation, role, and utils modules
  - Improved separation of concerns and maintainability

### 🐛 Bug Fixes

#### String Attribute Escaping in Multi-Value Attributes
- **Fixed proper escaping of special characters in multi-value string attributes**
  - Backslashes are now properly escaped: `\` → `\\`
  - Double quotes are properly escaped: `"` → `\"`
  - Escape order matters: backslashes first, then quotes
  - Location: `type_bridge/query.py`, `type_bridge/crud.py`, `type_bridge/models/base.py`
- **Impact**: Multi-value string attributes with quotes or backslashes now work correctly in insert/update operations
- **Examples**:
  - Quotes: `Tag('skill "Python"')` → TypeQL: `has Tag "skill \"Python\""`
  - Backslashes: `Path("C:\\Users\\Alice")` → TypeQL: `has Path "C:\\Users\\Alice"`
  - Mixed: `Description(r'Path: "C:\Program Files"')` → TypeQL: `has Description "Path: \"C:\\Program Files\""`

### 📚 Documentation

#### API Documentation Updated
- **Updated `docs/api/crud.md`** with comprehensive new sections:
  - Chainable Delete section with examples and behavior explanation
  - Bulk Update with Function section demonstrating lambda and named function usage
  - Updated EntityQuery method signatures
  - Added "New in v0.6.0" markers for discoverability
  - Error handling and empty results behavior documented

- **Updated `docs/api/attributes.md`** with new section:
  - "Configuring Attribute Type Names" section with comprehensive examples
  - Documents AttributeFlags.name for explicit type name overrides
  - Documents AttributeFlags.case for automatic case formatting
  - Shows priority order and all configuration options
  - Use cases and best practices

- **Updated `docs/INTERNALS.md`**:
  - Updated AttributeFlags dataclass documentation
  - Added name and case fields with usage examples
  - Updated cardinality field names (card_min, card_max)

#### README Updated
- **Added "Chainable Operations" to features list**
  - Highlights filter, delete, and bulk update capabilities
  - Location: `README.md`

#### New Example Created
- **Created `examples/advanced/crud_07_chainable_operations.py`**
  - Comprehensive demonstration of chainable delete and update
  - Shows lambda functions, named functions, and complex multi-attribute updates
  - Demonstrates atomic transaction behavior with rollback examples
  - Interactive tutorial format with step-by-step explanations

### 🧪 Testing

#### Integration Tests Added
- **Created `tests/integration/crud/entities/test_chainable.py`** with 9 comprehensive tests:
  1. `test_chainable_delete_with_expression_filter` - Basic delete with expressions
  2. `test_chainable_delete_with_multiple_filters` - Multiple filter combinations
  3. `test_chainable_delete_returns_zero_for_no_matches` - Empty results handling
  4. `test_chainable_delete_with_range_filter` - Range queries (gte/lt)
  5. `test_update_with_lambda_increments_age` - Lambda function updates
  6. `test_update_with_function_modifies_status` - Named function updates
  7. `test_update_with_returns_empty_list_for_no_matches` - Empty results handling
  8. `test_update_with_complex_function_multiple_attributes` - Multi-attribute updates
  9. `test_update_with_atomic_transaction` - Transaction rollback verification

#### Test Results
- **All 9 tests passing** ✅
- Tests verify:
  - Correct entity deletion with expression filters
  - Accurate counts returned
  - Proper transaction boundaries (atomic behavior)
  - Error propagation and rollback
  - Empty result handling
  - Multi-attribute updates
  - Lambda and function-based updates

#### Escaping Test Coverage Added
- **Created comprehensive string escaping test suite**
  - 7 unit tests for multi-value string escaping patterns
  - 9 integration tests verifying end-to-end escaping behavior
  - Location: `tests/unit/attributes/test_multivalue_escaping.py`, `tests/integration/crud/attributes/test_multivalue_escaping.py`
- **Test coverage includes**:
  - Quotes in strings: `'skill "Python"'`
  - Backslashes in paths: `C:\Users\Alice`
  - Mixed escaping: `"C:\Program Files\App"`
  - Empty strings and special characters
  - Unicode characters (café, 日本語, emoji😀)
  - Single quotes (not escaped in TypeQL)
  - Relations with multi-value escaping
  - Batch operations: `insert_many()`, `update_with()`

### 📦 Key Files Modified

- `type_bridge/crud.py` - Added delete() and update_with() to EntityQuery class
- `docs/api/crud.md` - Updated API documentation with new methods
- `README.md` - Added chainable operations to features list
- `examples/advanced/crud_07_chainable_operations.py` - New comprehensive example
- `tests/integration/crud/entities/test_chainable.py` - New integration test suite
- `tests/unit/attributes/test_multivalue_escaping.py` - New unit test suite (7 tests)
- `tests/integration/crud/attributes/test_multivalue_escaping.py` - New integration test suite (9 tests)

### 💡 Usage Examples

#### Chainable Delete
```python
# Delete all persons over 65
count = Person.manager(db).filter(Age.gt(Age(65))).delete()

# Delete with multiple filters
count = manager.filter(
    Age.lt(Age(18)),
    Status.eq(Status("inactive"))
).delete()
```

#### Chainable Update with Lambda
```python
# Increment age for all persons over 30
updated = manager.filter(Age.gt(Age(30))).update_with(
    lambda person: setattr(person, 'age', Age(person.age.value + 1))
)
```

#### Chainable Update with Function
```python
def promote(person):
    person.status = Status("senior")
    person.salary = Salary(int(person.salary.value * 1.1))

promoted = manager.filter(Age.gte(Age(35))).update_with(promote)
```

## [0.5.1] - 2025-11-20

### 🐛 Bug Fixes

#### Integer Key Query Bug
- **Fixed entities with Integer-type keys failing to query by key value**
  - Root cause: Attribute instances not being unwrapped before TypeQL value formatting
  - `EntityId(123)` was formatted as `"123"` (string) instead of `123` (integer)
  - Generated incorrect TypeQL: `has EntityId "123"` causing type mismatch
  - Fix: Added `.value` extraction in `_format_value()` before type checking
  - Location: `type_bridge/query.py:252-256`, `type_bridge/crud.py:419-423`, `type_bridge/crud.py:1145-1149`
- **Impact**: All non-string attribute types (Integer, Double, Decimal, Boolean, Date, DateTime, DateTimeTZ, Duration) now work correctly as entity keys and in query filters
- **Silent failure fixed**: Entities would insert successfully but couldn't be queried, now both work correctly

### 🧪 Testing

#### Regression Tests Added
- **Created comprehensive Integer key test suite**
  - 5 new integration tests specifically for Integer key bug regression
  - Tests cover: basic insert/query, comparison with String keys, various integer values, chainable queries, all()/count() methods
  - Location: `tests/integration/crud/entities/test_integer_key_bug.py`
- **Test Results**: 422/422 tests passing (284 unit + 138 integration) ✅
  - All existing tests still passing
  - All new regression tests passing
  - Zero test failures or regressions

#### Why String Keys Worked
- String attributes need quotes in TypeQL anyway
- Bug accidentally produced correct output: `"KEY-123"`
- Integer, Boolean, Double, etc. require unquoted values in TypeQL
- These would fail with incorrect quoting: `"123"`, `"true"`, `"3.14"`

### 📦 Key Files Modified

- `type_bridge/query.py` - Fixed `_format_value()` function
- `type_bridge/crud.py` - Fixed `EntityManager._format_value()` and `RelationManager._format_value()`
- `tests/integration/crud/entities/test_integer_key_bug.py` - New regression test suite (5 tests)

## [0.5.0] - 2025-11-20

### 🚀 New Features

#### Concise Attribute Type-Based Expression API
- **New streamlined query API using attribute class methods**
  - Old: `Person.age.gt(Age(30))` → New: `Age.gt(Age(30))` ✨
  - Shorter, more readable syntax with better type checking support
  - Type checkers now correctly validate all expression methods
  - Location: `type_bridge/attribute/base.py`, `type_bridge/attribute/string.py`

#### Class Methods Added to Attribute Base Class
- **Comparison methods**: `gt()`, `lt()`, `gte()`, `lte()`, `eq()`, `neq()`
  - Example: `Age.gt(Age(30))`, `Salary.gte(Salary(80000))`
- **Aggregation methods**: `sum()`, `avg()`, `max()`, `min()`, `median()`, `std()`
  - Example: `Salary.avg()`, `Age.sum()`
- **String-specific methods** (on `String` class): `contains()`, `like()`, `regex()`
  - Example: `Email.contains(Email("@company.com"))`, `Name.like(Name("^A.*"))`

#### Runtime Validation
- **Automatic attribute ownership validation in filter() methods**
  - Validates that entity owns the attribute type being queried
  - Raises `ValueError` with helpful message if validation fails
  - Example: `person_manager.filter(Salary.gt(Salary(50000)))` validates Person owns Salary
  - Location: `type_bridge/crud.py`, `type_bridge/expressions/base.py`

### 🔄 API Changes

#### Expression Classes Refactored
- **Changed from field-based to attribute type-based**
  - `ComparisonExpr`, `StringExpr`, `AggregateExpr` now use `attr_type` instead of `field`
  - Simpler internal structure with 1-to-1 mapping (attribute type uniquely identifies field)
  - Location: `type_bridge/expressions/comparison.py`, `type_bridge/expressions/string.py`, `type_bridge/expressions/aggregate.py`

#### Backwards Compatibility
- **Old field-based API still works**
  - `Person.age.gt(Age(30))` continues to work alongside new API
  - FieldRef classes now delegate to attribute class methods internally
  - Gradual migration path for existing code
  - Location: `type_bridge/fields.py`

### 🔧 Type Safety Improvements

#### Keyword-Only Arguments Enforced
- **Changed `@dataclass_transform(kw_only_default=False)` → `True`**
  - All Entity/Relation constructors now require keyword arguments for clarity and safety
  - Improves code readability and prevents positional argument order errors
  - Example: `Person(name=Name("Alice"), age=Age(30))` ✅
  - Positional args now rejected by type checkers: `Person(Name("Alice"), Age(30))` ❌
  - Location: `type_bridge/models/base.py:20`, `type_bridge/models/entity.py:81`, `type_bridge/models/relation.py:30`

#### Optional Field Defaults Required
- **Added explicit `= None` defaults for all optional fields**
  - Pattern: `age: Age | None = None` (previously `age: Age | None`)
  - Makes field optionality explicit in code for better clarity
  - Improves IDE autocomplete and type checking accuracy
  - Required by `kw_only_default=True` to distinguish optional from required fields
  - Applied throughout codebase: examples, tests, integration tests

#### Pyright Type Checking Configuration
- **Added `pyrightconfig.json`** for project-wide type checking
  - Excludes validation tests from type checking (`tests/unit/type-check-except/`)
  - Tests intentionally checking Pydantic validation failures now properly excluded
  - Core library achieves **0 errors, 0 warnings, 0 informations** ✨
  - Proper separation of type-safe code vs runtime validation tests
  - Location: `pyrightconfig.json` (new file)

#### Validation Tests Reorganized
- **Moved intentional validation tests to `tests/unit/type-check-except/`**
  - Tests using raw values to verify Pydantic validation now excluded from type checking
  - Original tests fixed to use properly wrapped attribute types
  - Clean separation: type-safe tests vs validation behavior tests
  - Moved files: `test_pydantic.py`, `test_basic.py`, `test_update_api.py`, `test_cardinality.py`, `test_list_default.py`

#### Benefits
1. **Clearer code**: Keyword arguments make field names explicit at call sites
2. **Better IDE support**: Explicit `= None` improves autocomplete for optional fields
3. **100% type safety**: Pyright validates correctly with zero false positives
4. **Maintainability**: Adding new fields doesn't break existing constructor calls
5. **Error prevention**: Type checker catches argument order mistakes at development time

### 📚 Documentation

#### Automatic Conversions Documented
- **`avg()` → `mean` in TypeQL**
  - TypeDB 3.x uses `mean` instead of `avg`
  - User calls `Age.avg()`, generates `mean($age)` in TypeQL
  - Result key converted back to `avg_age` for consistency
  - Clearly documented in docstrings and implementation comments
- **`regex()` → `like` in TypeQL**
  - TypeQL uses `like` for regex pattern matching
  - `regex()` provided as user-friendly alias
  - Both methods generate identical TypeQL output
  - Documented in method docstrings and code comments

#### TypeQL Compliance Verification
- **All expressions verified against TypeDB 3.x specification**
  - Comparison operators: `>`, `<`, `>=`, `<=`, `==`, `!=` ✓
  - String operations: `contains`, `like` ✓
  - Aggregations: `sum`, `mean`, `max`, `min`, `median`, `std`, `count` ✓
  - Boolean logic: `;` (AND), `or`, `not` ✓
  - Created `tmp/typeql_verification.md` and `tmp/automatic_conversions.md`

#### Examples Updated
- **Updated query_expressions.py to use new API**
  - All field-based expressions converted to attribute type-based
  - Added notes about API improvements and type safety
  - Location: `examples/advanced/query_expressions.py`

### 🧪 Testing

#### Test Results
- **417/417 tests passing** (100% pass rate) ✅
- **Unit tests**: 284/284 passing (0.3s)
- **Integration tests**: 133/133 passing
- **Type checking**: 0 errors, 0 warnings, 0 informations ✅
- All type errors eliminated (250 errors → 0 errors)

#### Tests Updated
- **Updated field reference tests for new API**
  - Tests now check `attr_type` instead of `field` attributes
  - All expression creation and TypeQL generation tests passing
  - Location: `tests/unit/expressions/test_field_refs.py`

#### Test Organization
- **Integration tests reorganized into subdirectories**
  - `tests/integration/queries/test_expressions.py` - Query expression integration tests
  - `tests/integration/crud/relations/test_abstract_roles.py` - Abstract role type tests
  - Better organization: crud/, queries/, schema/ subdirectories
  - Improved test discoverability and maintenance

### 🔧 Type Safety

#### Type Checking Improvements
- **Eliminated all expression-related type errors**
  - Before: 26 errors (`.gt()`, `.avg()` "not a known attribute" errors)
  - After: 0 errors ✅
  - Type checkers now fully understand class method API
  - Pyright passes with 0 errors, 0 warnings, 0 informations

#### Benefits
1. **Type-safe**: Full type checker support with zero errors
2. **Concise**: Shorter syntax (`Age.gt()` vs `Person.age.gt()`)
3. **Validated**: Runtime checks prevent invalid queries
4. **Compatible**: Old API still works for gradual migration
5. **Documented**: All automatic conversions clearly explained

### 📦 Key Files Modified

- `type_bridge/attribute/base.py` - Added class methods for comparisons and aggregations
- `type_bridge/attribute/string.py` - Added string-specific class methods
- `type_bridge/expressions/comparison.py` - Changed to `attr_type`-based
- `type_bridge/expressions/string.py` - Changed to `attr_type`-based
- `type_bridge/expressions/aggregate.py` - Changed to `attr_type`-based
- `type_bridge/expressions/base.py` - Added `get_attribute_types()` method
- `type_bridge/expressions/boolean.py` - Added recursive attribute type collection
- `type_bridge/fields.py` - Updated to delegate to attribute class methods
- `type_bridge/crud.py` - Added validation in filter() methods
- `examples/advanced/query_expressions.py` - Updated to use new API

## [0.4.4] - 2025-11-19

### 🐛 Bug Fixes

- **Fixed inherited attributes not included in insert/get operations**
  - Entity and Relation insert queries now include all inherited attributes
  - Fetch operations properly extract inherited attribute values
  - Added `get_all_attributes()` method to collect attributes from entire class hierarchy
  - Location: `type_bridge/models/base.py`, `type_bridge/models/entity.py`, `type_bridge/crud.py`

### 🔄 API Changes

- **Removed deprecated `EntityFlags` and `RelationFlags` aliases**
  - Use `TypeFlags` for both entities and relations
  - All example files updated to use `TypeFlags`
  - Documentation updated to reflect unified API

### 📚 Documentation

- **Updated CLAUDE.md**: Replaced all EntityFlags/RelationFlags references with TypeFlags
- **Updated examples**: All 17 example files now use the unified TypeFlags API

## [0.4.0] - 2025-11-15

### 🚀 New Features

#### Docker Integration for Testing
- **Automated Docker management for integration tests**
  - Added `docker-compose.yml` with TypeDB 3.5.5 server configuration
  - Created `test-integration.sh` script for automated Docker lifecycle management
  - Docker containers start/stop automatically with test fixtures
  - Location: `docker-compose.yml`, `test-integration.sh`, `tests/integration/conftest.py`
- **Optional Docker usage**: Set `USE_DOCKER=false` to use existing TypeDB server
- **Port configuration**: TypeDB server on port 1729

#### Schema Validation
- **Duplicate attribute type detection**
  - Prevents using the same attribute type for multiple fields in an entity/relation
  - Validates during schema generation to catch design errors early
  - Raises `SchemaValidationError` with detailed field information
  - Location: `type_bridge/schema/info.py`, `type_bridge/schema/exceptions.py`
- **Why it matters**: TypeDB stores ownership by attribute type, not by field name
  - Using `created: TimeStamp` and `modified: TimeStamp` creates a single ownership
  - This causes cardinality constraint violations at runtime
  - Solution: Use distinct types like `CreatedStamp` and `ModifiedStamp`

### 🧪 Testing

#### Test Infrastructure
- **Improved test organization**: 347 total tests (249 unit + 98 integration)
- **Docker-based integration tests**: Automatic container lifecycle management
- **Added duplicate attribute validation tests**: 6 new tests for schema validation
  - Location: `tests/unit/validation/test_duplicate_attributes.py`

### 📚 Documentation

- **Updated CLAUDE.md**:
  - Added Docker setup instructions for integration tests
  - Documented duplicate attribute type validation rules
  - Added schema validation best practices
  - Included examples of correct vs incorrect attribute usage
- **Updated test execution patterns**: Docker vs manual TypeDB server options

### 🔧 CI/CD

- **Updated GitHub Actions workflow**:
  - Integrated Docker Compose for automated integration testing
  - Added TypeDB 3.5.5 service container configuration
  - Location: `.github/workflows/` (multiple CI updates)

### 📦 Dependencies

- Added `docker-compose` support for development workflow
- No changes to runtime dependencies

### 🐛 Bug Fixes

- **Fixed test fixture ordering**: Improved integration test reliability with Docker
- **Enhanced error messages**: Schema validation errors now include field names

## [0.3.X] - 2025-01-14

### ✅ Full TypeDB 3.x Compatibility

**Major Achievement: 100% Test Pass Rate (341/341 tests)**

### Fixed

#### Query Pagination
- **Fixed TypeQL clause ordering**: offset must come BEFORE limit in TypeDB 3.x
  - Changed `limit X; offset Y;` → `offset Y; limit X;`
  - Location: `type_bridge/query.py:151-154`
- **Added automatic sorting for pagination**: TypeDB 3.x requires sorting for reliable offset results
  - Automatically finds and sorts by key attributes when using limit/offset
  - Falls back to required attributes if no key exists
  - Location: `type_bridge/crud.py:447-468`

#### Schema Conflict Detection
- **Updated to TypeDB 3.x syntax**: Changed from `sub` to `isa` for type queries
  - TypeDB 3.x uses `$e isa person` instead of `$e sub entity`
  - Fixed `has_existing_schema()` to properly detect existing types
  - Fixed `_type_exists()` to use correct TypeQL syntax
  - Location: `type_bridge/schema/manager.py:65-284`
- **Improved conflict detection**: Now properly raises SchemaConflictError when types exist

#### Type Safety
- **Fixed AttributeFlags attribute access**: Changed `cardinality_min` to `card_min`
  - Resolved pyright type checking error
  - Location: `type_bridge/crud.py:460`

### Testing

#### Test Results
- **Unit tests**: 243/243 passing (100%) - ~0.3s runtime
- **Integration tests**: 98/98 passing (100%) - ~18s runtime
- **Total**: 341/341 passing (100%)

#### Test Coverage
- All 9 TypeDB attribute types fully tested (Boolean, Date, DateTime, DateTimeTZ, Decimal, Double, Duration, Integer, String)
- Full CRUD operations for each type (insert, fetch, update, delete)
- Multi-value attribute operations
- Query pagination with limit/offset/sort
- Schema conflict detection and inheritance
- Reserved word validation

#### Code Quality
- ✅ Ruff linting: 0 errors, 0 warnings
- ✅ Ruff formatting: All 112 files properly formatted
- ✅ Pyright type checking: 0 errors, 0 warnings, 0 informations

### Documentation

- Updated README.md with current test counts and features
- Updated CLAUDE.md testing strategy section
- Added TypeDB 3.x compatibility notes
- Documented pagination requirements and automatic sorting

#### Key Files Modified
- `type_bridge/query.py` - Fixed clause ordering in build()
- `type_bridge/crud.py` - Added automatic sorting for pagination, fixed attribute access
- `type_bridge/schema/manager.py` - Updated to TypeDB 3.x `isa` syntax

## [0.2.0] - Previous Release

See git history for earlier changes.
