# type-bridge-schema-migration

Provider-neutral schema migration planning for TypeBridge. The crate verifies
migration histories and manifests, classifies safety, lowers canonical schema
deltas, constructs apply and rollback plans, and coordinates fenced execution
through an injected provider. Most applications should use the `type-bridge`
CLI rather than assemble these contracts directly.

## Dependency

```toml
[dependencies]
type-bridge-schema-migration = "2.1.0"
```

Start with the verified manifest and plan builders in the
[crate API](https://docs.rs/type-bridge-schema-migration/2.1.0). Execute only a
verified plan under the matching managed scope and lowering profile; use
[`type-bridge-schema-migration-typedb`](https://crates.io/crates/type-bridge-schema-migration-typedb)
for the TypeDB-backed provider.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0 and requires Rust 1.88+. The shipped lowering profile is pinned to TypeDB
3.12.1; the wider runtime supports TypeDB 3.11.x–3.12.x where capabilities
allow.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-migration/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
