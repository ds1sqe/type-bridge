# Choose an SDK

Every TypeBridge SDK uses the same Rust-owned schema, query, validation,
migration, and ORM contracts. Choose based on the language that owns your
application boundary, not on different database semantics.

| Surface | Model style | Execution | Best fit |
| --- | --- | --- | --- |
| [Python](../getting-started/quickstart.md) | Generated model/value classes | Embedded native runtime | Python services, scripts, data applications |
| [TypeScript / Node](typescript.md) | Generated branded model/value classes | Embedded N-API runtime | Node services with compile-time model safety |
| [Rust](rust.md) | Generated schema crate | Native async client | Rust services that bind models to a canonical schema |
| [Server](server-container.md) | Canonical request and schema contracts | Remote one-exchange execution | Centralized execution with caller-owned transport |

## Shared behavior

- Split-YAML entity, relation, attribute, role, inheritance, and cardinality
  facts are validated by Rust before generation.
- CRUD, hydration, query compilation, and transactions use the shared ORM.
- Immutable typed queries preserve owner-aware fields and selected model types.
- Compiled schema authority, schema fingerprints, and runtime projections are
  embedded in each generated package and bind its direct and remote queries.
- The separate generated schema-authority artifact exists only for a generic
  server and is never an SDK schema input.

## Deliberate language differences

The generated projections preserve native language conventions. Python uses
keyword-only constructors; TypeScript uses branded value classes and `bigint`;
Rust uses generated create/model types and async operations. These are typed
boundary differences, not alternate query or migration implementations.

The exact shared behavior of immutable queries is documented in
[Immutable typed queries](typed-queries.md). Maintainers should also consult
the normative [unified typed-query contract](../development/typed-query-contract.md).
