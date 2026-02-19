# Quick Start

This guide walks you through defining TypeDB models and performing basic operations with TypeBridge.

## 1. Define Attribute Types

TypeBridge supports all TypeDB value types:

```python
from type_bridge import String, Integer, Double, Decimal, Boolean, Date, DateTime, DateTimeTZ, Duration

class Name(String):
    pass

class Age(Integer):
    pass

class Balance(Decimal):  # High-precision fixed-point numbers
    pass
```

**Configuring attribute type names:**

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

## 2. Define Entities

```python
from type_bridge import Entity, TypeFlags, Flag, Key, Card

class Person(Entity):
    flags = TypeFlags(name="person")

    name: Name = Flag(Key)                   # @key (implies @card(1..1))
    age: Age | None = None                   # @card(0..1) - optional field
    email: Email                             # @card(1..1) - default cardinality
    tags: list[Tag] = Flag(Card(min=2))      # @card(2..) - two or more
```

> **Note**: `list[Type]` represents an **unordered set** in TypeDB. TypeDB has no list type -- order is never preserved.

## 3. Create Instances

```python
alice = Person(
    name=Name("Alice"),
    age=Age(30),
    email=Email("alice@example.com")
)

# Pydantic handles validation and type coercion automatically
print(alice.name.value)  # "Alice"
```

## 4. Work with Data

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

# Insert entities
Person.manager(db).insert(alice)

# Or use PUT for idempotent insert (safe to run multiple times!)
Person.manager(db).put(alice)
```

## 5. Define Relations

```python
from type_bridge import Relation, Role

class Employment(Relation):
    flags = TypeFlags(name="employment")

    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    position: Position
    salary: Salary | None = None
```

## 6. Use Python Inheritance

```python
class Animal(Entity):
    flags = TypeFlags(abstract=True)  # Abstract entity
    name: Name

class Dog(Animal):  # Automatically: dog sub animal in TypeDB
    breed: Breed
```

## Next Steps

- [User Guide](../guide/index.md) -- Full API documentation for attributes, entities, relations, CRUD, and more
- [Code Generator](../guide/generator.md) -- Generate Python models from TypeQL schema files
- [API Reference](../reference/index.md) -- Auto-generated reference from source docstrings
