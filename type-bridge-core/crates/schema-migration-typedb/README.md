# type-bridge-schema-migration-typedb

The TypeDB-backed provider and fenced execution store for canonical TypeBridge
schema migrations. This leaf crate joins provider-neutral migration plans to
the TypeDB runtime, control schema, journal, live observation, and managed
fence. Most applications should invoke these capabilities through the
`type-bridge` CLI or server.

## Dependency

```toml
[dependencies]
type-bridge-schema-migration-typedb = "2.1.0"
```

Start with `TypeDbMigrationRunner` or `TypeDbMigrationProvider` from the
[crate API](https://docs.rs/type-bridge-schema-migration-typedb/2.1.0). Run only
verified plans and retain the active managed fence for the full mutation; never
write the control schema or journal ad hoc.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0, requires Rust 1.88+, and executes migrations only against exactly TypeDB
3.12.1. The wider TypeDB driver-band support belongs to the runtime and server
crates, not this migration provider.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-migration-typedb/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
