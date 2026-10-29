# type-bridge-migration

The released migration IR, validation, checksum, recovery, planning, and
execution substrate for TypeBridge. Schema work uses canonical workspace
commands exposed by `type-bridge-cli`. Archive and sidecar adoption must be
completed with 2.3.x before upgrading to 3.0.

## Dependency

```toml
[dependencies]
type-bridge-migration = "2.3.2"
```

Use canonical workspace migration commands for application histories; the [crate API](https://docs.rs/type-bridge-migration/2.3.2)
documents the typed recovery and state-store boundaries. Do not bypass checksum
or recovery gates with raw database queries.

This crate has no optional features. It is released in lockstep with TypeBridge
2.3.2, requires Rust 1.88+, and uses the shared runtime for supported TypeDB
3.12.x–3.14.x servers.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-migration/2.3.2) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
