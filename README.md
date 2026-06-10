# TypeBridge

[![CI](https://github.com/ds1sqe/type-bridge/actions/workflows/ci.yml/badge.svg)](https://github.com/ds1sqe/type-bridge/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/type-bridge.svg)](https://pypi.org/project/type-bridge/)
[![Downloads](https://img.shields.io/pypi/dm/type-bridge.svg)](https://pypi.org/project/type-bridge/)
[![Python 3.13+](https://img.shields.io/badge/python-3.13+-blue.svg)](https://www.python.org/downloads/)
[![TypeDB 3.x](https://img.shields.io/badge/TypeDB-3.x-orange.svg)](https://typedb.com/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)
[![Ruff](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/astral-sh/ruff/main/assets/badge/v2.json)](https://github.com/astral-sh/ruff)

A modern, Pythonic ORM for [TypeDB](https://github.com/typedb/typedb) with an Attribute-based API that aligns with TypeDB's type system.

## Features

- **True TypeDB Semantics**: Attributes are independent types that entities and relations own
- **Complete Type Support**: All TypeDB value types - String, Integer, Double, Decimal, Boolean, Date, DateTime, DateTimeTZ, Duration
- **Flag System**: Clean API for `@key`, `@unique`, and `@card` annotations
- **Flexible Cardinality**: Express any cardinality constraint with `Card(min, max)`
- **Pydantic Integration**: Built on Pydantic v2 for automatic validation, serialization, and type safety
- **Type-Safe**: Full Python type hints and IDE autocomplete support
- **Declarative Models**: Define entities and relations using Python classes
- **Automatic Schema Generation**: Generate TypeQL schemas from your Python models
- **Code Generator**: Generate Python models from TypeQL schema files (`.tql`)
- **Schema Conflict Detection**: Automatic detection of breaking schema changes to prevent data loss
- **Data Validation**: Automatic type checking and coercion via Pydantic, including keyword validation
- **JSON Support**: Seamless JSON serialization/deserialization
- **CRUD Operations**: Full CRUD with fetching API (get, filter, all, update) for entities and relations
- **Lifecycle Hooks**: Pre/post-operation hooks for audit logging, validation, cache invalidation, and async notifications
- **Chainable Operations**: Filter, delete, and bulk update with method chaining and lambda functions
- **Query Builder**: Pythonic interface for building TypeQL queries
- **Multi-player Roles**: A single role can accept multiple entity types via `Role.multi(...)`
- **Transaction Context**: Share transactions across multiple operations with `TransactionContext`
- **Django-style Lookups**: Filter with `__contains`, `__gt`, `__in`, `__isnull` and more
- **Dict Helpers**: `to_dict()` and `from_dict()` for easy serialization and API integration
- **Bulk Operations**: `update_many()` and `delete_many()` for efficient batch processing

## Installation

```bash
# Install from PyPI
pip install type-bridge

# Or with uv
uv add type-bridge
```

**From source:**

```bash
git clone https://github.com/ds1sqe/type-bridge.git
cd type_bridge
uv sync
```

## Quick Start

### 1. Define Attribute Types

TypeBridge supports all TypeDB value types:

```python
from type_bridge import String, Integer, Double, Decimal, Boolean, Date, DateTime, DateTimeTZ, Duration

class Name(String):
    pass

class Age(Integer):
    pass

class Balance(Decimal):  # High-precision fixed-point numbers
    pass

class BirthDate(Date):  # Date-only values
    pass

class UpdatedAt(DateTimeTZ):  # Timezone-aware datetime
    pass
```

**Configuring Attribute Type Names:**

```python
from type_bridge import AttributeFlags, TypeNameCase

# Option 1: Explicit name override
class Name(String):
    flags = AttributeFlags(name="person_name")
# TypeDB: attribute person_name, value string;

# Option 2: Case formatting
class UserEmail(String):
    flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)
# TypeDB: attribute user_email, value string;
```

### 2. Define Entities

```python
from type_bridge import Entity, TypeFlags, Flag, Key, Card

class Person(Entity):
    flags = TypeFlags(name="person")  # Optional, defaults to lowercase class name

    # Use Flag() for key/unique markers and Card for cardinality
    name: Name = Flag(Key)                   # @key (implies @card(1..1))
    age: Age | None = None                   # @card(0..1) - optional field (explicit default)
    email: Email                             # @card(1..1) - default cardinality
    tags: list[Tag] = Flag(Card(min=2))      # @card(2..) - two or more (unordered set)
```

> **Note**: `list[Type]` represents an **unordered set** in TypeDB. TypeDB has no list type - order is never preserved.

### 3. Create Instances

```python
# Create entity instances with attribute values (keyword arguments required)
alice = Person(
    name=Name("Alice"),
    age=Age(30),
    email=Email("alice@example.com")
)

# Pydantic handles validation and type coercion automatically
print(alice.name.value)  # "Alice"
```

### 4. Work with Data

```python
from type_bridge import Database, SchemaManager

# Connect to database
db = Database(address="localhost:1729", database="mydb")
db.connect()
db.create_database()

# Define schema
schema_manager = SchemaManager(db)
schema_manager.register(Person, Company, Employment)
schema_manager.sync_schema()

# Insert entities - use typed instances
alice = Person(
    name=Name("Alice"),
    age=Age(30),
    email=Email("alice@example.com")
)
Person.manager(db).insert(alice)

# Or use PUT for idempotent insert (safe to run multiple times!)
Person.manager(db).put(alice)  # Won't create duplicates

# Insert relations - use typed instances
employment = Employment(
    employee=alice,
    employer=techcorp,
    position=Position("Engineer"),
    salary=Salary(100000)
)
Employment.manager(db).insert(employment)
```

### 5. Cardinality Constraints

```python
from type_bridge import Card, Flag

class Person(Entity):
    flags = TypeFlags(name="person")

    # Cardinality options:
    name: Name                              # @card(1..1) - exactly one (default)
    age: Age | None = None                  # @card(0..1) - zero or one (explicit default)
    tags: list[Tag] = Flag(Card(min=2))     # @card(2..) - two or more (unbounded)
    skills: list[Skill] = Flag(Card(max=5)) # @card(0..5) - zero to five
    jobs: list[Job] = Flag(Card(1, 3))      # @card(1..3) - one to three
```

### 6. Define Relations

```python
from type_bridge import Relation, TypeFlags, Role

class Employment(Relation):
    flags = TypeFlags(name="employment")

    # Define roles with type-safe Role[T] syntax
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Relations can own attributes
    position: Position                   # @card(1..1)
    salary: Salary | None = None         # @card(0..1) - explicit default

# Multi-player role example (one role, multiple entity types)
class Document(Entity):
    flags = TypeFlags(name="document")
    name: Name = Flag(Key)

class Email(Entity):
    flags = TypeFlags(name="email")
    name: Name = Flag(Key)

class Trace(Relation):
    flags = TypeFlags(name="trace")
    origin: Role[Document | Email] = Role.multi("origin", Document, Email)
```

### 7. Using Python Inheritance

```python
class Animal(Entity):
    flags = TypeFlags(abstract=True)  # Abstract entity
    name: Name

class Dog(Animal):  # Automatically: dog sub animal in TypeDB
    breed: Breed
```

### 8. Generate Models from TypeQL Schema

Instead of writing Python classes manually, generate them from your TypeQL schema:

```bash
# Generate Python models from a schema file
python -m type_bridge.generator schema.tql -o ./myapp/models/
```

Or programmatically:

```python
from type_bridge.generator import generate_models

generate_models("schema.tql", "./myapp/models/")
```

This generates a complete Python package:

```text
myapp/models/
├── __init__.py      # Package exports, SCHEMA_VERSION, schema_text()
├── attributes.py    # Attribute class definitions
├── entities.py      # Entity class definitions
├── relations.py     # Relation class definitions
├── registry.py      # Schema metadata, JSON Schema fragments, lookup functions
└── schema.tql       # Copy of original schema
```

The generator supports:

- Entity/relation/attribute inheritance (`sub` keyword)
- `@key`, `@unique`, `@card` constraints (including on `plays` and `relates`)
- `@regex` and `@values` constraints
- `@abstract` and `@independent` types
- `@range(min..max)` constraints (integers, floats, dates, datetimes)
- Role overrides (`relates X as Y`)
- TypeDB function definitions with precise return type hints
- Registry module generation for schema metadata and JSON Schema fragments
- Both `#` and `//` comment styles

See the [Code Generator guide](https://ds1sqe.github.io/type-bridge/guide/generator/) for full documentation.

## Documentation

**[https://ds1sqe.github.io/type-bridge/](https://ds1sqe.github.io/type-bridge/)** — Full documentation site with user guide, API reference, and development guides.

- [Getting Started](https://ds1sqe.github.io/type-bridge/getting-started/) — Installation and quick start
- [User Guide](https://ds1sqe.github.io/type-bridge/guide/) — Attributes, entities, relations, CRUD, queries, and more
- [API Reference](https://ds1sqe.github.io/type-bridge/reference/) — Auto-generated from source docstrings
- [Development](https://ds1sqe.github.io/type-bridge/development/) — Setup, testing, and internals

## Pydantic Integration

TypeBridge is built on Pydantic v2, giving you powerful features:

```python
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age

# Automatic validation and type coercion
alice = Person(name=Name("Alice"), age=Age(30))

# JSON serialization
json_data = alice.model_dump_json()

# JSON deserialization
bob = Person.model_validate_json('{"name": "Bob", "age": 25}')

# Model copying
alice_copy = alice.model_copy(update={"age": Age(31)})
```

## Running Examples

TypeBridge includes comprehensive examples organized by complexity:

```bash
# Basic CRUD examples (start here!)
uv run python examples/basic/crud_01_define.py  # Schema definition
uv run python examples/basic/crud_02_insert.py  # Data insertion
uv run python examples/basic/crud_03_read.py    # Fetching API
uv run python examples/basic/crud_04_update.py  # Update operations

# Additional basic examples
uv run python examples/basic/crud_05_filter.py    # Advanced filtering
uv run python examples/basic/crud_06_aggregate.py # Aggregations
uv run python examples/basic/crud_07_delete.py    # Delete operations
uv run python examples/basic/crud_08_put.py       # Idempotent PUT operations

# Advanced examples
uv run python examples/advanced/schema_01_manager.py       # Schema operations
uv run python examples/advanced/schema_02_comparison.py    # Schema comparison
uv run python examples/advanced/schema_03_conflict.py      # Conflict detection
uv run python examples/advanced/features_01_pydantic.py    # Pydantic integration
uv run python examples/advanced/features_02_type_safety.py # Literal types
uv run python examples/advanced/query_01_expressions.py    # Query expressions
uv run python examples/advanced/validation_01_reserved_words.py  # Keyword validation
```

## Running Tests

TypeBridge uses a two-tier testing approach with **100% test pass rate**:

```bash
# Unit tests (fast, no external dependencies) - DEFAULT
uv run pytest                              # Run unit tests
uv run pytest tests/unit/attributes/ -v   # Test all 9 attribute types
uv run pytest tests/unit/core/ -v         # Test core functionality
uv run pytest tests/unit/flags/ -v        # Test flag system
uv run pytest tests/unit/expressions/ -v  # Test query expressions

# Integration tests (requires TypeDB; ./test.sh manages one by default)
# Option 1: Isolated (recommended) — test.sh brings up TypeDB and tears it down
./test.sh                                 # Full suite (Rust + Python + Node), isolated

# Option 2: Use an existing TypeDB server
./test.sh --no-isolated                   # Full suite against a running TypeDB
USE_DOCKER=false uv run pytest -m integration -v  # Python integration only (~60s)

# Run specific integration test categories
uv run pytest tests/integration/crud/entities/ -v      # Entity CRUD tests
uv run pytest tests/integration/crud/relations/ -v    # Relation CRUD tests
uv run pytest tests/integration/queries/ -v           # Query expression tests
uv run pytest tests/integration/schema/ -v            # Schema operation tests

# All tests
uv run pytest -m "" -v                    # Run all Python tests
./test.sh                                 # Full suite (Rust + Python + Node), detailed
./scripts/check.sh                        # CI-shaped checks (rust|python|node|all)
```

## Rust Core

The project includes a Rust core (`type-bridge-core/`) that provides high-performance implementations of the query compiler, validation engine, and value coercer. When the native extension is installed, Python automatically delegates to Rust for:

- **Validation** — up to 40x faster schema-aware query validation
- **Compilation** — up to 2.5x faster AST-to-TypeQL compilation via serde bridge
- **Value coercion** — Type-safe value coercion and TypeQL literal formatting

The Rust core is a Cargo workspace with five crates:

| Crate | Description |
|-------|-------------|
| `type-bridge-core-lib` | Pure-Rust AST, schema parser, query compiler, and validation engine |
| `type-bridge-orm` | Async ORM with entity/relation managers, chainable queries, and batch operations |
| `type-bridge-orm-derive` | Derive macros for `TypeBridgeEntity`, `TypeBridgeRelation`, `TypeBridgeAttribute` |
| `type-bridge-core` | PyO3 bindings exposing the Rust core to Python |
| `type-bridge-server` | Transport-agnostic query pipeline with interceptor chain and HTTP API |

See [`type-bridge-core/README.md`](type-bridge-core/README.md) for build instructions and architecture details.

## Requirements

- Python 3.13+
- TypeDB 3.10.4 server
- type-bridge-core>=1.4.5
- pydantic>=2.12.4
- isodate==0.7.2 (for Duration type support)
- jinja2>=3.1.0 (for code generation)
- typer>=0.15.0 (for CLI)

`typedb-driver==3.10.0` is required only for direct Python driver APIs and
development/integration tests.

## Release Notes

See the [CHANGELOG.md](CHANGELOG.md) for detailed release notes and version history.

## License

MIT License
