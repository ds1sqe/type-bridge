# type-bridge-workspace

Validated TypeBridge workspace configuration and orchestration. The crate
binds split-YAML schema sources, managed scope, semantic profile, environments,
migration directories, generated outputs, symbolic secrets, and workspace
locks without performing implicit network I/O. Most applications should use
the `type-bridge` CLI rather than instantiate these services directly.

## Dependency

```toml
[dependencies]
type-bridge-workspace = "2.1.0"
```

Begin with `TypeBridgeConfigSpec` and `TypeBridgeWorkspace` in the
[crate API](https://docs.rs/type-bridge-workspace/2.1.0). Provide explicit
source, secret-reference, and extension services, then keep generated paths
confined to the validated workspace authority.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0 and requires Rust 1.88+. Each workspace selects one frozen
`typedb-3.11.5/v1` or `typedb-3.12.1/v1` semantic profile; database operations
support TypeDB 3.11.x–3.12.x where capabilities permit.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-workspace/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
