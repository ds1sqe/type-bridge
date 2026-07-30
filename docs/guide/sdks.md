# Choose an SDK

Every TypeBridge SDK uses the same Rust-owned schema, query, validation,
migration, and ORM contracts. Choose based on the language that owns your
application boundary, not on different database semantics.

| Surface | Model style | Execution | Best fit |
| --- | --- | --- | --- |
| [Python](../getting-started/quickstart.md) | Declarative Pydantic classes or generated models | Embedded native runtime | Python services, scripts, data applications |
| [TypeScript / Node](typescript.md) | Branded classes or generated models | Embedded N-API runtime | Node services with compile-time model safety |
| [Rust](rust.md) | Generated schema crate | Native async client | Rust services that bind models to a canonical schema |
| [Server](server-container.md) | Canonical request and schema contracts | Remote one-exchange execution | Centralized execution with caller-owned transport |

## Shared behavior

- Entity, relation, attribute, role, inheritance, and cardinality descriptors
  are validated by Rust.
- CRUD, hydration, query compilation, and transactions use the shared ORM.
- Immutable typed queries preserve owner-aware fields and selected model types.
- Schema fingerprints and runtime projections bind generated code to its
  declared authority.

## Deliberate language differences

The facades preserve native language conventions. Python uses Pydantic and
keyword-only constructors; TypeScript uses branded value classes and `bigint`;
Rust uses generated create/model types and async operations. These are typed
boundary differences, not alternate query or migration implementations.

The exact shared behavior of immutable queries is documented in
[Immutable typed queries](typed-queries.md). Maintainers should also consult
the normative [unified typed-query contract](../development/typed-query-contract.md).
