# type-bridge-schema-codegen

Deterministic Python, TypeScript, and Rust package emitters over validated,
binding-neutral TypeBridge model projections. It is a supporting generator
crate; application code should run `type-bridge schema generate` and consume
the generated package rather than constructing emitters itself.

## Dependency

```toml
[dependencies]
type-bridge-schema-codegen = "2.1.0"
```

Generator integrations begin with `PythonEmitter`, `TypeScriptEmitter`, or
`RustEmitter` from the [crate API](https://docs.rs/type-bridge-schema-codegen/2.1.0).
Only pass projections produced by the canonical schema engine, and write every
file in the returned `GeneratedPackage` as one atomic generation operation.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0 and requires Rust 1.88+. Generated runtime packages follow the TypeBridge
2.1 support matrix: TypeDB 3.11.x–3.12.x and a 3.12.1 V2 semantic baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-codegen/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
