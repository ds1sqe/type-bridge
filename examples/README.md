# TypeBridge Examples

This directory contains comprehensive examples demonstrating all features of TypeBridge, organized by complexity and topic.

## Quick Start

If you're new to TypeBridge, start with the **Basic CRUD Tutorial** series in `basic/`.

## Directory Structure

```
examples/
├── basic/          # CRUD tutorial series (start here!)
├── patterns/       # Common modeling patterns
├── advanced/       # Advanced features and techniques
├── schema/         # Schema management (moved to advanced/)
├── features/       # Feature demonstrations (moved to advanced/)
├── config/         # Configuration examples (moved to advanced/)
└── validation/     # Validation examples (moved to advanced/)
```

## Learning Path

### 1. Basic CRUD Tutorial (Start Here!)

Complete tutorial series covering fundamental operations:

1. **crud_01_define.py** - Schema Definition
   - Defining attribute types
   - Creating entities and relations
   - Using TypeFlags and cardinality
   - Generating and syncing schema

2. **crud_02_insert.py** - Inserting Data
   - Creating entity instances
   - Single and bulk inserts
   - Creating relations with role players

3. **crud_03_read.py** - Reading and Querying
   - Getting all entities
   - Dictionary-based filtering
   - Chainable queries (`.filter()`, `.limit()`, `.first()`)
   - Querying relations and role players

4. **crud_04_update.py** - Updating Data
   - Fetch-Modify-Update pattern
   - Updating single-value attributes
   - Updating multi-value attributes
   - Understanding TypeQL update semantics

5. **crud_05_filter.py** - Filtering with Expressions
   - Comparison operators (>, <, >=, <=, ==, !=)
   - String operations (contains, like/regex)
   - Range queries (combining filters)
   - Query modifiers (limit, offset, first)

6. **crud_06_aggregate.py** - Aggregations and Grouping
   - Counting with `.count()`
   - Database-side aggregations (avg, sum, max, min)
   - Filtered aggregations
   - Group-by queries

7. **crud_07_delete.py** - Deleting Data
   - Deleting by key attributes
   - Deleting with filters
   - Chainable delete operations
   - Understanding delete return values
   - Deletion best practices

8. **crud_08_put.py** - PUT Operations (Idempotent Insert)
   - Understanding PUT vs INSERT semantics
   - Single and bulk PUT operations
   - All-or-nothing behavior
   - Use cases for idempotent operations
   - Data loading and synchronization patterns

**Prerequisites**: Running TypeDB server on localhost:1729

**Run the series**:
```bash
uv run python examples/basic/crud_01_define.py
uv run python examples/basic/crud_02_insert.py
uv run python examples/basic/crud_03_read.py
uv run python examples/basic/crud_04_update.py
uv run python examples/basic/crud_05_filter.py
uv run python examples/basic/crud_06_aggregate.py
uv run python examples/basic/crud_07_delete.py
uv run python examples/basic/crud_08_put.py
```

### 2. Pattern Examples

Common modeling patterns and best practices:

#### **patterns/inheritance_01_abstract.py** - Abstract Types and Inheritance
- Defining abstract entity types
- Creating concrete subtypes with inheritance
- Polymorphic queries
- Shared attribute ownership via inheritance
- Using abstract types in relation roles

**Concepts**: abstract=True, Person → Student/Employee, polymorphic queries

#### **patterns/cardinality_01_multi_value.py** - Cardinality and Multi-Value Attributes
- Single-value attributes (required/optional)
- Multi-value attributes (list[Type])
- Bounded cardinality (Card(min, max))
- CRUD operations on multi-value attributes
- Cardinality validation

**Concepts**: Card API, list[Tag], Flag(Card(1, 5)), replace semantics

#### **patterns/cardinality_02_plays_side.py** - Plays-Side Role Cardinality
- Relates-side role cardinality (`relates role @card`)
- Plays-side role cardinality (`plays relation:role @card`)
- Generating TypeQL without a running database

**Concepts**: Role(..., cardinality=Card(...)), Role(..., plays_cardinality=Card(...))

### 3. Advanced Examples

Advanced features and techniques:

#### Schema Management (advanced/)

- **schema_01_manager.py** - SchemaManager Basics
  - Registering models
  - Generating TypeQL schema
  - Syncing schema to database

- **schema_02_comparison.py** - Schema Comparison
  - Collecting schema information
  - Comparing old vs new schemas
  - Viewing schema diffs

- **schema_03_conflict.py** - Conflict Detection
  - Automatic conflict detection
  - Handling breaking schema changes
  - Force recreate with sync_schema(force=True)

#### Features (advanced/)

- **features_01_pydantic.py** - Pydantic Integration
  - Automatic validation
  - JSON serialization/deserialization
  - Model copying
  - Type coercion

- **features_02_type_safety.py** - Type Safety
  - Generic managers (EntityManager[E])
  - IDE autocomplete
  - Type inference

- **features_03_string_repr.py** - String Representations
  - `__str__` vs `__repr__`
  - User-friendly vs developer views

- **features_04_base_flag.py** - Base Flag
  - base=True for Python-only classes
  - Skipping intermediate classes in hierarchy
  - get_supertype() behavior

- **features_05_implicit_flags.py** - Implicit Flags
  - Automatic TypeFlags creation
  - When explicit flags are needed

#### Configuration (advanced/)

- **config_01_typename_case.py** - Type Name Casing
  - TypeNameCase options (LOWERCASE, CLASS_NAME, SNAKE_CASE)
  - Global and per-type configuration

- **config_02_attribute_case.py** - Attribute Name Casing
  - Attribute-level case formatting
  - Mixing different formats

- **config_03_typeflags.py** - TypeFlags API
  - Unified flags for entities/relations
  - name, abstract, base configuration

#### Validation (advanced/)

- **validation_01_reserved_words.py** - Reserved Word Validation
  - TypeQL keyword conflicts
  - Automatic validation
  - Error messages and suggestions

- **validation_02_edge_cases.py** - Validation Edge Cases
  - Built-in type conflicts (entity, relation, attribute)
  - Name collision avoidance

#### Query Expressions (advanced/)

- **query_01_expressions.py** - Advanced Query Expressions
  - Complex query patterns
  - Boolean logic (or_, and_, not_)
  - Multiple aggregations
  - Real-world query examples

#### Chainable Operations (advanced/)

- **crud_07_chainable_operations.py** - Chainable Delete and Update
  - Chainable delete with expression filters
  - update_with using lambda functions
  - update_with using named functions
  - Atomic transaction behavior

**New in v0.6.0**: EntityQuery.delete(), EntityQuery.update_with()

## Example Categories

### By Feature

| Feature | Examples |
|---------|----------|
| Schema Definition | crud_01_define, schema_01_manager |
| Insert Operations | crud_02_insert |
| Put Operations | crud_08_put |
| Read/Query | crud_03_read, crud_05_filter, query_01_expressions |
| Update Operations | crud_04_update, crud_07_chainable_operations |
| Delete Operations | crud_07_delete, crud_07_chainable_operations |
| Aggregations | crud_06_aggregate, query_01_expressions |
| Abstract Types | inheritance_01_abstract |
| Cardinality | cardinality_01_multi_value, cardinality_02_plays_side |
| Pydantic Integration | features_01_pydantic |
| Type Safety | features_02_type_safety |
| Validation | validation_01_reserved_words, validation_02_edge_cases |

### By Complexity

| Level | Examples |
|-------|----------|
| Beginner | crud_01 through crud_07 (basic series) |
| Intermediate | patterns/*, query_01_expressions |
| Advanced | crud_07_chainable_operations, schema_*, features_* |

## Prerequisites

### Running TypeDB Server

Most examples require a TypeDB server running locally:

```bash
# Using Docker
docker run -d -p 1729:1729 --name typedb vaticle/typedb:latest

# Or download from https://typedb.com/download
```

### Installing TypeBridge

```bash
# Install with dev dependencies
uv sync --extra dev

# Or minimal install
pip install -e .
```

## Running Examples

### Interactive Examples

Most examples are interactive tutorials with explanations:

```bash
uv run python examples/basic/crud_01_define.py
# Follow prompts, press Enter to continue through steps
```

### Non-Interactive Testing

For automated testing, mock the `input()` function:

```python
# test_wrapper.py
import sys
import builtins
builtins.input = lambda *args: ""
sys.path.insert(0, '/path/to/type_bridge')
exec(open('examples/basic/crud_01_define.py').read())
```

## Example Conventions

### Database Names

Each example uses a specific database name:
- Basic CRUD series: `crud_demo`
- Pattern examples: `pattern_*` (e.g., `pattern_inheritance`)
- Advanced examples: varies by example

### Cleanup

Examples offer database cleanup at the end:
```
Delete 'database_name' database? [y/N]:
```

Choose 'y' to clean up, or 'N' to preserve for exploration.

### Code Style

All examples follow these conventions:
- Attribute types defined first (e.g., `class Name(String)`)
- Entities and relations after attributes
- Clear step-by-step demonstrations
- Inline code examples with explanations
- Summary sections at the end

## Common Patterns

### Basic Entity Definition

```python
from type_bridge import Entity, String, Integer, Flag, Key, TypeFlags

class Name(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    flags: TypeFlags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None
```

### Basic Relation Definition

```python
from type_bridge import Relation, Role, TypeFlags

class Employment(Relation):
    flags: TypeFlags = TypeFlags(name="employment")

    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    position: Position
    salary: Salary | None
```

### Schema Setup

```python
from type_bridge import Database, SchemaManager

db = Database(address="localhost:1729", database="my_db")
db.connect()

if db.database_exists():
    db.delete_database()

db.create_database()

schema_mgr = SchemaManager(db)
schema_mgr.register(Person, Company, Employment)
schema_mgr.sync_schema()
```

### CRUD Operations

```python
# Insert
person_mgr = Person.manager(db)
alice = Person(name=Name("Alice"), age=Age(30))
person_mgr.insert(alice)

# PUT (Idempotent Insert)
person_mgr.put(alice)  # Safe to run multiple times

# Read
persons = person_mgr.all()
alice = person_mgr.get(name="Alice")[0]
filtered = person_mgr.filter(Age.gt(Age(25))).execute()

# Update
alice.age = Age(31)
person_mgr.update(alice)

# Delete
person_mgr.delete(name="Alice")
```

## Troubleshooting

### TypeDB Connection Errors

```
tcp connect error: Connection refused (os error 111)
```

**Solution**: Start TypeDB server on port 1729.

### Database Already Exists

**Solution**: Delete existing database:
```python
db.delete_database()
db.create_database()
```

### Schema Conflicts

```
SchemaConflictError: Attribute 'name' is already defined
```

**Solution**: Use force recreate:
```python
schema_mgr.sync_schema(force=True)
```

## Next Steps

After completing the examples:

1. **Read the API docs** in `docs/api/`
2. **Explore advanced patterns** in the TypeDB documentation
3. **Build your own application** using TypeBridge
4. **Contribute examples** for patterns you discover!

## Contributing Examples

Have a useful pattern or use case? Contribute it!

1. Follow the example structure (step-by-step with explanations)
2. Include code snippets with clear output
3. Add interactive prompts for learning
4. Document prerequisites and setup
5. Submit a pull request

## Resources

- **TypeBridge Documentation**: `docs/`
- **TypeDB Documentation**: https://typedb.com/docs
- **TypeQL Reference**: https://typedb.com/docs/typeql/overview
- **Issues & Support**: https://github.com/anthropics/claude-code/issues
