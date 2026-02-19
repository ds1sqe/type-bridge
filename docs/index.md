# TypeBridge

A modern, Pythonic ORM for [TypeDB](https://github.com/typedb/typedb) with an Attribute-based API that aligns with TypeDB's type system.

## Highlights

- **True TypeDB Semantics** -- Attributes are independent types that entities and relations own
- **Complete Type Support** -- All 10 TypeDB value types: String, Integer, Double, Decimal, Boolean, Date, DateTime, DateTimeTZ, Duration
- **Flag System** -- Clean API for `@key`, `@unique`, and `@card` annotations
- **Pydantic Integration** -- Built on Pydantic v2 for automatic validation, serialization, and type safety
- **Declarative Models** -- Define entities and relations using Python classes
- **Code Generator** -- Generate Python models from TypeQL schema files (`.tql`)
- **CRUD Operations** -- Full CRUD with fetching API, chainable operations, and Django-style lookups
- **Rust Core** -- Optional Rust-backed validation (~8.6x speedup) and query compilation

## Quick Example

```python
from type_bridge import Entity, String, Integer, TypeFlags, Flag, Key

class Name(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None
```

## Next Steps

- [Installation](getting-started/installation.md) -- Get TypeBridge set up
- [Quick Start](getting-started/quickstart.md) -- Define models, insert data, run queries
- [User Guide](guide/index.md) -- Comprehensive API documentation
- [API Reference](reference/index.md) -- Auto-generated from source docstrings
