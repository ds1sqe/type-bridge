# type-bridge-core-lib

Pure-Rust TypeQL AST, schema parser, query compiler, validation engine, and value coercer for **type-bridge**.

## Modules

| Module | Purpose |
|--------|---------|
| `ast` | TypeQL Abstract Syntax Tree — patterns, statements, clauses, and values |
| `schema` | Schema representation with entity / relation / attribute types and inheritance resolution |
| `validation` | Schema-aware query validation plus a portable JSON validation-rule DSL |
| `compiler` | Compiles an AST back into a TypeQL query string |
| `query_parser` | Parses a TypeQL query string into the AST (bidirectional round-trip) |
| `value_coercion` | Coerces raw values into TypeDB value-types and formats TypeQL literals |
| `reserved_words` | TypeQL reserved-word detection |
| `parser` | Low-level Winnow grammar consumed by `query_parser` and `schema` |

## Usage

```rust
use type_bridge_core_lib::query_parser::parse_typeql_query;
use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_core_lib::schema::TypeSchema;
use type_bridge_core_lib::validation::ValidationEngine;

// Parse a TypeQL query into AST clauses
let clauses = parse_typeql_query("match $p isa person, has name 'Alice';").unwrap();

// Compile back to TypeQL
let compiler = QueryCompiler::new();
let typeql = compiler.compile(&clauses);
assert_eq!(typeql, "match $p isa person, has name 'Alice';");

// Validate against a schema
let schema = TypeSchema::from_typeql("define entity person, owns name; attribute name, value string;").unwrap();
let engine = ValidationEngine::new();
let result = engine.validate_query(&clauses, &schema);
assert!(result.is_valid);
```

## Feature flags

| Feature | Default | Effect |
|---------|---------|--------|
| `pyo3` | no | Enables `#[derive(FromPyObject)]` on AST types for PyO3 interop |

## Testing

```bash
cargo test -p type-bridge-core-lib
```

## License

MIT
