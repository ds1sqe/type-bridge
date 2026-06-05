# Integration Coverage Inventory

This inventory supports #124 Phase 6. The goal is to restructure integration
coverage so Rust shared-runtime, Rust ORM binding, Python binding, and
TypeScript/Node binding tests are selected explicitly and fail hard when their
real TypeDB dependency is unavailable.

## Current Phase 6 Status

As of Phase 6.3, Rust shared-runtime and Rust ORM binding integration tests
have been migrated into a selected non-ignored target:

```bash
cargo test -p type-bridge-orm --features integration-tests --test integration -- --nocapture
```

That target currently runs 25 real TypeDB tests with 0 ignored tests. Normal
fast Rust checks still run without TypeDB through `cargo test -p
type-bridge-orm`.

The Node/TypeScript side is still pending Phase 6.4. Rust-side NAPI DB tests
under `type-bridge-core/crates/node/src/tests/` remain ignored until package
level TypeScript/Node integration coverage replaces or narrows them.

## Pre-Migration Baseline

Python has the only mature integration layout today. Its real TypeDB tests are
organized under `tests/integration/` by behavior and selected with pytest's
`integration` marker.

Rust and Node have useful real-DB assertions, but they are not yet real gates:

- Rust shared-runtime DB tests are flat `dynamic_*_integration.rs` files under
  `type-bridge-core/crates/orm/tests/`.
- Rust ORM typed/derive DB tests are in
  `type-bridge-core/crates/orm/tests/integration_tests.rs`.
- Node DB tests are Rust-side NAPI wrapper tests under
  `type-bridge-core/crates/node/src/tests/`.
- All current Rust and Node DB tests are marked `#[ignore]`.
- Current Rust and Node DB setup returns early when TypeDB is unavailable, so
  selected ignored tests can still pass without touching a database.
- Node package-level TypeScript integration tests do not exist yet. The
  existing package smoke and TypeScript wrapper tests are useful, but they are
  not substitutes for real package integration.

Baseline commands run on 2026-06-01:

```bash
cargo test -p type-bridge-orm \
  --test dynamic_entity_crud_integration \
  dynamic_entity_crud_against_typedb \
  -- --ignored --nocapture
```

Result: passed while printing `Skipping Rust dynamic integration test: TypeDB
unavailable (...) Connection refused`.

```bash
cargo test -p type-bridge-node \
  node_entity_crud_against_typedb \
  -- --ignored --nocapture
```

Result: passed while printing `Skipping Node integration test: TypeDB
connection unavailable (...) Connection refused`.

That false-green behavior is the first target for Phase 6.2.

## Python Binding Coverage

Python integration coverage is the model for the new Rust and Node layout.

| Behavior group | Current Python files | Notes |
| --- | --- | --- |
| Attribute CRUD | `tests/integration/crud/attributes/test_*.py` | Covers string, integer, double, boolean, date, datetime, datetime-tz, decimal, duration, multi-value attributes, and escaping. |
| Entity CRUD | `tests/integration/crud/entities/test_*.py` plus `tests/integration/crud/test_typedb_manager.py` | Covers insert, fetch, update, put, delete, bulk operations, chainable operations, integer keys, and manager behavior. |
| Relation CRUD | `tests/integration/crud/relations/test_*.py` plus `tests/integration/crud/test_complex_relations.py` | Covers insert, fetch, filter, update, put, delete, chainable operations, multi-role relations, and abstract roles. |
| Interop/type combinations | `tests/integration/crud/interop/test_*.py` | Covers mixed primitive types, mixed entity shapes, and multi-type queries. |
| Query API | `tests/integration/queries/test_*.py` | Covers match, filters, lookup filters, role-player lookup filters, role multi-lookup filters, expressions, ordering, pagination, and domain-shaped query suites. |
| Schema | `tests/integration/schema/test_*.py` | Covers creation, diff, migration, annotations, attributes, cardinality, conflicts, constraints, inheritance, relations, role-player diffs, and type coverage. |
| Sessions and transactions | `tests/integration/session/test_*.py` | Covers session lifecycle, transaction context commit/rollback, and transaction edge cases. |
| Generator and migration | `tests/integration/generator/test_*.py`, `tests/integration/migration/test_*.py` | Python-only user-space pipeline coverage. |
| Validation | `tests/integration/validation/test_reserved_words_integration.py` | Python public validation behavior. |
| Proxy | `tests/integration/proxy/test_*.py` | Proxy server integration, separate from the binding parity target. |
| Cross-language parity | `tests/integration/parity/test_*.py` | Descriptor fixture tests plus Python-writer/Node-reader real DB parity. Node-writer/Python-reader remains Phase 5.4. |

## Rust Shared-Runtime Coverage

Current shared-runtime DB coverage lives in ignored files. These should migrate
under `type-bridge-core/crates/orm/tests/integration/core/`.

| Current test | Coverage | Migration destination |
| --- | --- | --- |
| `dynamic_entity_crud_against_typedb` | Dynamic entity insert, get, update, get-by-iid, put-many, count, aggregate, group-by aggregate, delete. | `integration/core/entities.rs` |
| `dynamic_entity_all_primitive_attribute_values_against_typedb` | All MVP primitive values through dynamic entity CRUD. | `integration/core/attributes.rs` |
| `dynamic_entity_multi_value_attributes_against_typedb` | Dynamic multi-value attributes. | `integration/core/attributes.rs` |
| `dynamic_relation_crud_against_typedb` | Dynamic relation insert, fetch, update, put, delete, role players. | `integration/core/relations.rs` |
| `dynamic_entity_filters_and_lookup_against_typedb` | Dynamic entity filters and lookup behavior. | `integration/core/filters.rs` |
| `dynamic_relation_filters_and_role_lookup_against_typedb` | Dynamic relation filters and role-player lookup behavior. | `integration/core/filters.rs` |
| `dynamic_entity_transaction_commit_and_rollback_against_typedb` | Dynamic entity transaction commit and rollback behavior. | `integration/core/transactions.rs` |
| `dynamic_entity_filter_then_iid_update_and_delete_against_typedb` | Chainable-equivalent entity mutation semantics via filter, IID update, and delete. | `integration/core/mutations.rs` |
| `dynamic_relation_filter_then_iid_update_and_delete_against_typedb` | Chainable-equivalent relation mutation semantics via filter, IID update, and delete. | `integration/core/mutations.rs` |
| `dynamic_relation_single_role_accepts_multiple_player_types_against_typedb` | Same role accepting multiple concrete player types. | `integration/core/relations.rs` |
| `dynamic_relation_abstract_role_resolves_concrete_players_against_typedb` | Abstract role-player definitions, concrete subtype insertion/filtering, and hydration. | `integration/core/relations.rs` |

Coverage gaps compared with Python:

- Dynamic runtime does not yet have Python-style edge-case breadth for all
  entity and relation delete/update/put variants.
- Dynamic runtime has only focused filter coverage, not the full lookup,
  expression, order, and pagination breadth of Python queries.
- Dynamic runtime has no grouped schema/migration suite; schema behavior is
  mostly Rust unit coverage or Rust ORM typed coverage.
- Dynamic runtime lacks a non-ignored selected integration target that fails on
  missing TypeDB.

## Rust ORM Binding Coverage

Current Rust ORM typed/derive DB coverage lives in
`type-bridge-core/crates/orm/tests/integration_tests.rs`. It uses
`include_schema!`, typed managers, and derive-style models, but all tests are
ignored and setup silently skips when TypeDB is unavailable.

| Current test | Coverage | Migration destination |
| --- | --- | --- |
| `full_entity_lifecycle` | Typed entity schema sync, insert, all, count, delete, verify deleted. | `integration/rust_binding/entities.rs` |
| `batch_insert_and_delete` | Typed entity insert-many and delete-many. | `integration/rust_binding/entities.rs` |
| `query_builder_with_filters` | Typed query builder filter execution. | `integration/rust_binding/queries.rs` |
| `schema_introspection` | Schema sync and live schema introspection. | `integration/rust_binding/schema.rs` |
| `full_relation_lifecycle` | Typed relation insert with role players and attributes. | `integration/rust_binding/relations.rs` |
| `entity_update_lifecycle` | Typed entity update lifecycle. | `integration/rust_binding/entities.rs` |
| `entity_put_creates_and_updates` | Typed entity put create/update behavior. | `integration/rust_binding/entities.rs` |
| `query_builder_with_sort_and_limit` | Typed query ordering and limit behavior. | `integration/rust_binding/queries.rs` |
| `transaction_context_batch_commit` | Typed transaction context batch commit behavior. | `integration/rust_binding/transactions.rs` |

Coverage gaps compared with Python:

- Typed Rust binding coverage is much smaller than Python's entity, relation,
  query, schema, and transaction suites.
- Multi-value, abstract-role, multi-role, and same-role multi-player behavior
  currently live mainly in dynamic runtime tests, not typed Rust binding tests.
- There is no selected non-ignored Rust ORM integration target.

## TypeScript/Node Binding Coverage

Current Node coverage has three layers:

- Rust-side NAPI wrapper unit tests in `type-bridge-core/crates/node/src/lib.rs`
  and `src/tests/`.
- Rust-side NAPI DB tests in `type-bridge-core/crates/node/src/tests/*_integration.rs`.
- Package smoke and scope checks under `type-bridge-core/crates/node/tests/`.

The Rust-side NAPI DB tests are useful native-boundary probes, but they are not
package-level TypeScript integration tests.

| Current test | Coverage | Migration destination |
| --- | --- | --- |
| `node_entity_crud_against_typedb` | Entity CRUD through NAPI JSON manager wrapper. | `tests/integration/crud/entities.test.ts` |
| `node_entity_all_primitive_attribute_values_against_typedb` | All MVP primitive values through NAPI JSON manager wrapper. | `tests/integration/crud/attributes.test.ts` |
| `node_entity_multi_value_attributes_against_typedb` | Multi-value attributes through NAPI JSON manager wrapper. | `tests/integration/crud/attributes.test.ts` |
| `node_relation_crud_against_typedb` | Relation CRUD and role players through NAPI JSON manager wrapper. | `tests/integration/crud/relations.test.ts` |
| `node_entity_filters_and_lookup_against_typedb` | Entity filters and lookups through NAPI JSON manager wrapper. | `tests/integration/queries/filters.test.ts` |
| `node_relation_filters_and_role_lookup_against_typedb` | Relation filters and role-player lookup through NAPI JSON manager wrapper. | `tests/integration/queries/role-players.test.ts` |
| `node_entity_transaction_commit_and_rollback_against_typedb` | Transaction commit and rollback through NAPI wrapper. | `tests/integration/transactions/transactions.test.ts` |
| `node_entity_filter_then_iid_update_and_delete_against_typedb` | Chainable-equivalent entity mutation semantics through NAPI wrapper. | `tests/integration/crud/entities.test.ts` |
| `node_relation_filter_then_iid_update_and_delete_against_typedb` | Chainable-equivalent relation mutation semantics through NAPI wrapper. | `tests/integration/crud/relations.test.ts` |
| `node_relation_single_role_accepts_multiple_player_types_against_typedb` | Same role accepting multiple concrete player types. | `tests/integration/crud/relations.test.ts` |
| `node_relation_abstract_role_resolves_concrete_players_against_typedb` | Abstract role-player definitions and concrete hydration. | `tests/integration/crud/relations.test.ts` |

Coverage that is useful but not a real DB substitute:

- `tests/package-smoke.cjs` verifies that the package entrypoint can load a
  built native module and register descriptors. It does not execute real DB
  CRUD.
- `tests/scope-probe.cjs` verifies package boundary rules. It does not execute
  real DB CRUD.
- The generated `target/ts-tests/tests/crud_contract.js` exercises
  TypeScript wrapper marshalling against fake managers. It is useful
  facade-contract coverage, not integration coverage.

Coverage gaps compared with Python:

- There is no source-controlled TypeScript/Node integration suite.
- Node package-level tests do not yet exercise real TypeDB entity CRUD,
  relation CRUD, attributes, filters, transactions, multi-role relations, or
  abstract-role behavior through the public package entrypoint.
- Current Node DB tests are all `#[ignore]`d Rust tests and silently skip on
  missing TypeDB.

## Cross-Language Parity Coverage

Current parity coverage lives under `tests/integration/parity/`.

| Current test | Coverage | Status |
| --- | --- | --- |
| `test_phase5_fixture_contract_loads_and_validates_without_typedb` | Shared fixture metadata and expected canonical JSON load. | TypeDB-free smoke. |
| `test_canonical_json_output_is_stable` | Canonical JSON ordering/stability. | TypeDB-free smoke. |
| `test_python_descriptor_snapshot_matches_fixture_without_typedb` | Python public model descriptors to fixture descriptor snapshot. | TypeDB-free descriptor coverage. |
| `test_node_descriptor_snapshot_matches_fixture_without_typedb` | Node public descriptor registry to fixture descriptor snapshot. | TypeDB-free descriptor coverage. |
| `test_python_and_node_descriptor_snapshots_match_without_typedb` | Python and Node normalized descriptors match. | TypeDB-free descriptor parity. |
| `test_python_writer_node_reader_matches_canonical_fixture` | Python writes through public managers under Rust backend, Node reads through package dynamic managers, canonical output matches fixture. | Real DB cross-language integration. |

Remaining parity gaps:

- Node writer to Python reader remains Phase 5.4.
- Bidirectional update/put/delete/transaction parity remains Phase 5.5.
- CI/local all-parity entrypoint remains Phase 5.6 and should align with the
  Phase 6 integration entrypoints.

## Migration Rules

- Remove `#[ignore]` from migrated Rust and Node DB tests.
- Do not keep `Option` setup plus early `return` for selected integration
  tests. Missing TypeDB must be a test failure.
- Keep fast unit commands available without TypeDB by using explicit
  integration targets, required features, or package scripts.
- Prefer package-level Node integration over Rust-side NAPI JSON integration.
- Keep Rust-side NAPI DB tests only where they prove native-boundary behavior
  that a TypeScript package test cannot cover.
- Do not duplicate the existing Python manager integration corpus.
- Label smoke, fake-manager, descriptor-only, and package-load tests as
  non-substitutes for real DB integration.
