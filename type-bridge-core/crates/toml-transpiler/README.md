# type-bridge-toml-transpiler

A small, pure-Rust converter from the retained TypeBridge TOML schema DSL to a
canonical TypeQL `define` block. New TypeBridge workspaces author split YAML;
this crate exists for callers that still need the released TOML conversion
boundary.

## Dependency and usage

```toml
[dependencies]
type-bridge-toml-transpiler = "2.1.0"
```

```rust
let toml_source = r#"
[attributes.name]
value = "string"

[entities.person]
owns = ["name"]
"#;

let typeql = type_bridge_toml_transpiler::toml_to_typeql(toml_source)?;
assert_eq!(
    typeql,
    "define\nattribute name, value string;\nentity person, owns name;\n",
);
# Ok::<(), type_bridge_toml_transpiler::TranspileError>(())
```

The converter validates the complete TOML document before returning TypeQL and
performs no filesystem or database I/O. It has no optional features.

The crate is released in lockstep with TypeBridge 2.1.0 and requires Rust
1.88+. Database-facing TypeBridge 2.1 components support TypeDB
3.11.x–3.12.x, with 3.12.1 as the V2 semantic baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-toml-transpiler/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
