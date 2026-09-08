# type-bridge-schema

Canonical schema loading, lossless YAML normalization, resolution, projection,
managed-scope differencing, and compiled schema-authority verification for
TypeBridge. This is a supporting engine crate for tooling and binding authors;
applications normally use generated models through
[`type-bridge`](https://crates.io/crates/type-bridge).

## Dependency

```toml
[dependencies]
type-bridge-schema = "2.1.0"
```

Use the staged APIs documented at
[`type_bridge_schema`](https://docs.rs/type-bridge-schema/2.1.0): load a schema
set from an explicitly bounded source, normalize and resolve it, then project
or compare only the resulting verified schema values. Split YAML remains the
authoring source; emitted authority artifacts are generated outputs.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0 and requires Rust 1.88+. Resolution accepts the frozen TypeDB 3.11.5 and
3.12.1 V2 semantic profiles; database-facing TypeBridge components support
TypeDB 3.11.x–3.12.x.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
