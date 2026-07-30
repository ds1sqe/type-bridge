# Development

Contributor and maintainer documentation for the shared Rust engine, language
facades, generated SDKs, and release products.

Start with the repository-root
[`DEVELOPMENT.md`](https://github.com/ds1sqe/type-bridge/blob/master/DEVELOPMENT.md)
for the working agreement and command map.

## Work on the repository

- [Development setup](setup.md) -- dependencies, containers, code quality, and
  the local workflow
- [Testing](testing.md) -- offline, live TypeDB, cross-language, packaging, and
  release-artifact tiers
- [Internals](internals.md) -- Python model metadata and architecture
- [Rust backend](rust-backend.md) -- shared ORM execution path and binding
  boundary
- [TypeDB concepts and compatibility](typedb.md) -- driver bands, supported
  versions, and TypeQL behavior
- [Abstract type concepts](abstract-types.md) -- interface hierarchies and
  polymorphic queries

## Maintainer contracts

These pages preserve detailed acceptance and parity decisions. They are not
the shortest route for application users:

- [Unified typed-query contract](typed-query-contract.md)
- [Generated Rust parity inventory](rust-generated-parity.md)
- [Integration coverage inventory](integration-coverage-inventory.md)
