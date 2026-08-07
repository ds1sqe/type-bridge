# type-bridge-cli

The `type-bridge` command-line interface for split-YAML workspaces, generated
bindings, and canonical schema migrations. Schema checking, generation, and
migration authoring/planning are offline; apply, verify, and adopt connect only
through an explicitly selected workspace environment.

## Install and start

```bash
cargo install type-bridge-cli --version 2.1.0 --locked
type-bridge --help
type-bridge schema check
type-bridge schema generate
```

Run commands from a workspace containing `typebridge.yaml`, or pass
`--manifest <path>`. Keep credentials as symbolic environment references and
enable migration access explicitly in the chosen environment. The same entry
point is available to Rust integrators as `type_bridge_cli::run_cli`.

The crate has no optional features. It is released in lockstep with TypeBridge
2.1.0, requires Rust 1.88+, and supports TypeDB 3.11.x–3.12.x; 3.12.1 is the V2
semantic and migration baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-cli/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
