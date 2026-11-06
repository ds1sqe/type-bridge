# type-bridge-core-lib

Pure-Rust TypeQL AST, schema parser, query compiler, validation engine, and value coercer for **type-bridge**.

This is a supporting engine crate. Most applications should consume generated
models through [`type-bridge`](https://crates.io/crates/type-bridge) instead.

```toml
[dependencies]
type-bridge-core-lib = "2.3.2"
```

## Modules

| Module | Purpose |
|--------|---------|
| `ast` | Canonical typed query operations |
| `validation` | Identifier validation and a portable JSON validation-rule DSL |
| `compiler` | Compiles canonical typed operations into TypeQL |
| `value_coercion` | Coerces raw values into TypeDB value-types and formats TypeQL literals |
| `reserved_words` | TypeQL reserved-word detection |

## Usage

```rust
use type_bridge_core_lib::validation::ValidationEngine;

let validation = ValidationEngine::new();
assert!(validation.validate_type_name("person", "entity").is_valid);
assert!(!validation.validate_type_name("match", "entity").is_valid);
```

Canonical Split-YAML schema loading and resolution live in
[`type-bridge-schema`](https://crates.io/crates/type-bridge-schema). The hidden
frozen parser/schema modules in this crate are compatibility machinery, not an
application schema-authoring API.

## Feature flags

| Feature | Default | Effect |
|---------|---------|--------|
| `pyo3` | no | Enables `#[derive(FromPyObject)]` on AST types for PyO3 interop |

## Testing

```bash
cargo test -p type-bridge-core-lib
```

## Compatibility

This crate is released in lockstep with TypeBridge 2.3.2 and requires Rust
1.88+. Database-facing TypeBridge components support TypeDB
3.12.x–3.14.x, with 3.13.6 as the V2 semantic baseline.

[API documentation](https://docs.rs/type-bridge-core-lib/2.3.2) ·
[Repository](https://github.com/ds1sqe/type-bridge)

## License

[MIT](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
