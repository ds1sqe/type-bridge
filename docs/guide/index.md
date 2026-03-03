# TypeBridge API Reference

Complete API reference for TypeBridge - a Python ORM for TypeDB with an Attribute-based API.

## Overview

TypeBridge provides a Pythonic interface to TypeDB that aligns with TypeDB's type system, where **attributes are base types** that **entities and relations own**.

## API Documentation

### Core Concepts

- **[Attributes](attributes.md)** - All 9 attribute types and value types
- **[Entities](entities.md)** - Entity definition, ownership, and inheritance
- **[Relations](relations.md)** - Relations, roles, and role players
- **[Cardinality](cardinality.md)** - Card API and Flag system for constraints

### Data Operations

- **[CRUD Operations](crud.md)** - Create, read, update, delete with type-safe managers
- **[Queries](queries.md)** - Query expressions, filtering, aggregations, and pagination
- **[Functions](functions.md)** - TypeDB schema-defined functions and FunctionQuery
- **[Schema Management](schema.md)** - Schema operations, conflict detection, and migrations

### Code Generation

- **[Generator](generator.md)** - Generate Python models from TypeDB schema files
- **[API DTOs](dto.md)** - Pydantic Data Transfer Objects for REST APIs

### Validation and Type Safety

- **[Validation](validation.md)** - Pydantic integration, type safety, and literal types

## Quick Reference

### Basic Usage Pattern

```python
from type_bridge import Entity, TypeFlags, String, Integer, Flag, Key

# 1. Define attribute types
class Name(String):
    pass

class Age(Integer):
    pass

# 2. Define entity with ownership
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None  # Optional field

# 3. Create instances (keyword arguments required)
alice = Person(name=Name("Alice"), age=Age(30))

# 4. CRUD operations
person_manager = Person.manager(db)
person_manager.insert(alice)
persons = person_manager.all()

# 5. Add lifecycle hooks (optional)
person_manager.add_hook(my_audit_hook)  # chainable
```

## Key Principles

### 1. Attributes Are Independent Types

Define attributes once, reuse across entities/relations:

```python
class Name(String):
    pass

class Person(Entity):
    name: Name  # Person owns 'name'

class Company(Entity):
    name: Name  # Company also owns 'name'
```

### 2. Use TypeFlags for Configuration

Clean API with `TypeFlags`:

```python
class Person(Entity):
    flags = TypeFlags(name="person")  # Clean API
```

### 3. Use Flag System for Annotations

```python
from type_bridge import Flag, Key, Unique, Card

name: Name = Flag(Key)                    # @key (implies @card(1..1))
email: Email = Flag(Unique)               # @unique (default @card(1..1))
age: Age | None = None                    # @card(0..1) - PEP 604 syntax
tags: list[Tag] = Flag(Card(min=2))       # @card(2..) - multi-value
```

### 4. Python Inheritance Maps to TypeDB Supertypes

```python
class Animal(Entity):
    flags = TypeFlags(abstract=True)

class Dog(Animal):  # Generates: entity dog, sub animal
    pass
```

### 5. Keyword-Only Arguments

All Entity/Relation constructors require keyword arguments:

```python
# ✅ CORRECT
person = Person(name=Name("Alice"), age=Age(30))

# ❌ WRONG
person = Person(Name("Alice"), Age(30))
```

### 6. TransactionContext for Shared Operations

Share transactions across multiple operations:

```python
with db.transaction(TransactionType.WRITE) as tx:
    Person.manager(tx).insert(alice)
    Company.manager(tx).insert(techcorp)
    # Both commit together
```

### 7. Django-style Lookup Filters

Filter with expressive suffix operators:

```python
person_manager.filter(name__startswith="Al", age__gt=30).execute()
person_manager.filter(status__in=["active", "pending"]).execute()
```

### 8. Sorting Results

Sort query results with `order_by()`:

```python
# Ascending (default)
person_manager.filter().order_by('age').execute()

# Descending (prefix with '-')
person_manager.filter().order_by('-age').execute()

# Multiple fields
person_manager.filter().order_by('city', '-age').execute()

# Role-player attributes (relations only)
employment_manager.filter().order_by('employee__age').execute()
```

### 9. Dict Helpers for Serialization

Easy conversion to/from dictionaries:

```python
data = person.to_dict()  # {'name': 'Alice', 'age': 30}
person = Person.from_dict(data)
```

## Generated Schema Example

The Python code above generates this TypeQL schema:

```typeql
define

# Attributes (defined once, can be owned by multiple types)
attribute name, value string;
attribute age, value integer;

# Entities declare ownership with cardinality annotations
entity person,
    owns name @key,
    owns age @card(0..1);
```

## Navigation

- [Attributes Documentation](attributes.md)
- [Entities Documentation](entities.md)
- [Relations Documentation](relations.md)
- [Cardinality Documentation](cardinality.md)
- [CRUD Operations Documentation](crud.md)
- [Queries Documentation](queries.md)
- [Functions Documentation](functions.md)
- [Schema Management Documentation](schema.md)
- [Generator Documentation](generator.md)
- [API DTOs Documentation](dto.md)
- [Validation Documentation](validation.md)

---

For TypeDB integration details, see [docs/development/typedb.md](../development/typedb.md).

For development guidelines, see [docs/development/setup.md](../development/setup.md).

For abstract types implementation, see [abstract-types.md](abstract-types.md). For TypeDB abstract types concepts, see [docs/development/abstract-types.md](../development/abstract-types.md).
