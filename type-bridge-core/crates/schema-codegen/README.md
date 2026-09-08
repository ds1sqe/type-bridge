# type-bridge-schema-codegen

Deterministic Python, TypeScript, Rust, and internal C package emitters over
validated, binding-neutral TypeBridge model projections. It is a supporting
generator crate; application code should run `type-bridge schema generate` and
consume the generated package rather than constructing emitters itself.

## Dependency

```toml
[dependencies]
type-bridge-schema-codegen = "2.2.0"
```

Generator integrations begin with `PythonEmitter`, `TypeScriptEmitter`,
`RustEmitter`, or the internal `CEmitter` from the
[crate API](https://docs.rs/type-bridge-schema-codegen/2.2.0). Only pass
projections produced by the canonical schema engine, and write every file in
the returned `GeneratedPackage` as one atomic generation operation.

Every generated C package targets ABI 1.6 through one header and exact
runtime dependencies. Schema features select the projection format. Ordered
packages provide nominal CRUD, typed mutation batches, and field-token
manager filters with database and borrowed read-transaction terminals.
Generated handles retain their native owners and share the C boundary's
close, recovery, and alias checks. The package remains unpublished until
its release acceptance checks pass.

This crate has no optional features. It is released in lockstep with TypeBridge
2.2.0 and requires Rust 1.88+. Generated runtime packages follow the TypeBridge
2.1 support matrix: TypeDB 3.11.x–3.12.x and a 3.12.1 V2 semantic baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-codegen/2.2.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
