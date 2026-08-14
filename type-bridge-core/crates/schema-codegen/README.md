# type-bridge-schema-codegen

Deterministic Python, TypeScript, Rust, and internal C package emitters over
validated, binding-neutral TypeBridge model projections. It is a supporting
generator crate; application code should run `type-bridge schema generate` and
consume the generated package rather than constructing emitters itself.

## Dependency

```toml
[dependencies]
type-bridge-schema-codegen = "2.1.0"
```

Generator integrations begin with `PythonEmitter`, `TypeScriptEmitter`,
`RustEmitter`, or the internal `CEmitter` from the
[crate API](https://docs.rs/type-bridge-schema-codegen/2.1.0). Only pass
projections produced by the canonical schema engine, and write every file in
the returned `GeneratedPackage` as one atomic generation operation.

The C emitter feature-selects its compatibility surface. Unordered C-v2
packages retain their five generated files byte-for-byte, including ABI 1.3
descriptor and dependency metadata. Ordered C-v3 packages include the additive
ABI 1.4 header and emit policy-aware nominal CRUD and operation-branded batch
wrappers. They also emit header-local exact-model managers: immutable field-token
filters compose into siblings, and database or borrowed read-transaction
terminals provide exhaustive `all`, identity-strict `first`, `count`, and
`exists` without extending the frozen native export or macro inventories.
Manager filters and results retain their own native owners and use the same
generated close/recovery and deep-alias rules as the other nominal C facades.
The C package remains an unpublished, unsupported foundation until the C SDK
plans are complete.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0 and requires Rust 1.88+. Generated runtime packages follow the TypeBridge
2.1 support matrix: TypeDB 3.11.x–3.12.x and a 3.12.1 V2 semantic baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-codegen/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
