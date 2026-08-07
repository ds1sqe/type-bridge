# type-bridge-core-lib

Pure-Rust TypeQL AST, schema parser, query compiler, validation engine, and value coercer for **type-bridge**.

This is a supporting engine crate. Most applications should consume generated
models through [`type-bridge`](https://crates.io/crates/type-bridge) instead.

```toml
[dependencies]
type-bridge-core-lib = "2.1.0"
```

## Modules

| Module | Purpose |
|--------|---------|
| `ast` | TypeQL Abstract Syntax Tree — patterns, statements, clauses, and values |
| `validation` | Schema-aware query validation plus a portable JSON validation-rule DSL |
| `compiler` | Compiles an AST back into a TypeQL query string |
| `query_parser` | Parses a TypeQL query string into the AST (bidirectional round-trip) |
| `value_coercion` | Coerces raw values into TypeDB value-types and formats TypeQL literals |
| `reserved_words` | TypeQL reserved-word detection |

## Usage

```rust
use type_bridge_core_lib::query_parser::parse_typeql_query;
use type_bridge_core_lib::compiler::QueryCompiler;

// Parse a TypeQL query into AST clauses
let clauses = parse_typeql_query(r#"match $p isa person, has name "Alice";"#).unwrap();

// Compile back to TypeQL
let compiler = QueryCompiler::new();
let typeql = compiler.compile(&clauses);
assert_eq!(
    typeql,
    r#"match
$p isa person, has name "Alice";"#,
);
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

This crate is released in lockstep with TypeBridge 2.1.0 and requires Rust
1.88+. Database-facing TypeBridge 2.1 components support TypeDB
3.11.x–3.12.x, with 3.12.1 as the V2 semantic baseline.

[API documentation](https://docs.rs/type-bridge-core-lib/2.1.0) ·
[Repository](https://github.com/ds1sqe/type-bridge)

## License

[MIT](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
