# type-bridge-migration

The released migration IR, validation, checksum, recovery, planning, and
execution substrate for TypeBridge. It also retains the V1 migration command
surface and archive-recovery contracts. New schema work should use the V2
workspace commands exposed by `type-bridge-cli`.

## Dependency

```toml
[dependencies]
type-bridge-migration = "2.1.0"
```

Use `load_dir_checked` and `validate_graph` before planning or executing any
retained migration history; the [crate API](https://docs.rs/type-bridge-migration/2.1.0)
documents the typed recovery and state-store boundaries. Do not bypass checksum
or recovery gates with raw database queries.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0, requires Rust 1.88+, and uses the shared runtime for supported TypeDB
3.11.x–3.12.x servers.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-migration/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
