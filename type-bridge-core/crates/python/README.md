# type-bridge-core (Python bindings)

PyO3 bindings that expose the **type-bridge** Rust core to Python.

## Overview

This crate wraps [`type-bridge-core-lib`](../core/) and exposes it as a native
Python extension module via [PyO3](https://pyo3.rs) and
[maturin](https://www.maturin.rs).

All complex types cross the Python ↔ Rust boundary via
`pythonize` / `depythonize` using **serde-tagged-enum dicts**:

```python
# Pattern dict format (serde tag = "type", content = "data")
pattern = {
    "type": "Entity",
    "data": {
        "variable": "$p",
        "type_name": "person",
        "constraints": [],
        "is_strict": False,
    },
}
```

## Available Python classes

| Class | Wraps |
|-------|-------|
| `ValidationEngine` | `type_bridge_core_lib::validation::ValidationEngine` |
| `QueryCompiler` | `type_bridge_core_lib::compiler::QueryCompiler` |
| `TypeSchema` | `type_bridge_core_lib::schema::TypeSchema` |
| `ValueCoercer` | `type_bridge_core_lib::value_coercion::ValueCoercer` |

**Standalone functions:** `parse_typeql_query()`, `format_value()`, `coerce_value()`

**AST wrappers:** `EntityPattern`, `RelationPattern`, `MatchClause`, `InsertClause`, etc. (mirrors of `type_bridge_core_lib::ast`)

## Building

```bash
cd type-bridge-core

# Development build (editable)
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop

# Release build
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin build --release
```

> **Note:** `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` is required when your
> local Python version exceeds PyO3's maximum supported version (currently 3.13).

## Usage from Python

```python
from type_bridge_core import ValidationEngine, QueryCompiler, TypeSchema

# Parse and compile
compiler = QueryCompiler()
clauses = compiler.parse("match $p isa person, has name 'Alice';")
typeql = compiler.compile(clauses)

# Schema-aware validation
schema = TypeSchema.from_typeql("define entity person, owns name; attribute name, value string;")
engine = ValidationEngine()
result = schema.validate_query(clauses)
print(result)  # {"is_valid": true, "errors": []}
```

## License

MIT
