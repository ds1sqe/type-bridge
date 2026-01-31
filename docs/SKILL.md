---
name: type-bridge
description: Use the type-bridge Python ORM for TypeDB. Covers defining entities, relations, attributes, CRUD operations, queries, expressions, and schema management. Use when working with TypeDB in Python projects.
---

# type-bridge Python ORM for TypeDB

type-bridge is a Pythonic ORM for TypeDB that provides type-safe abstractions over TypeQL.

## Quick Start

```python
from type_bridge import (
    Entity, Relation, Role, String, Integer, Double, Boolean,
    Flag, Key, Unique, Card, TypeFlags, Database, SchemaManager
)

# 1. Define attribute types (reusable across entities)
class Name(String):
    pass

class Email(String):
    pass

class Age(Integer):
    pass

# 2. Define entity with ownership
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)           # Primary key
    email: Email = Flag(Unique)      # Unique constraint
    age: Age | None = None           # Optional field

# 3. Connect and sync schema
db = Database(address="localhost:1729", database="mydb")
with db:
    db.create_database()
    schema = SchemaManager(db)
    schema.register(Person)
    schema.sync_schema()

    # 4. CRUD operations
    manager = Person.manager(db)
    alice = Person(name=Name("Alice"), email=Email("alice@example.com"), age=Age(30))
    manager.insert(alice)
```

---

## Defining Models

### Attribute Types

Attributes are independent types that can be owned by entities and relations.

```python
from type_bridge import String, Integer, Double, Boolean, DateTime, Date, Duration, Decimal

# Simple attributes (inherit from value types)
class Name(String):
    pass

class Score(Double):
    pass

class IsActive(Boolean):
    pass

class CreatedAt(DateTime):
    pass

# Attribute with custom TypeDB name
from type_bridge import AttributeFlags

class PersonEmail(String):
    flags = AttributeFlags(name="email")  # TypeDB name: "email", not "person_email"
```

### Entities

```python
from type_bridge import Entity, Flag, Key, Unique, Card, TypeFlags

class Person(Entity):
    flags = TypeFlags(name="person")

    # Key attribute (required, unique identifier)
    person_id: PersonId = Flag(Key)

    # Unique attribute (unique but not primary key)
    email: Email = Flag(Unique)

    # Required attribute (no default)
    name: Name

    # Optional attribute
    age: Age | None = None

    # Multi-valued attribute (list)
    tags: list[Tag] = Flag(Card(min=0))

# Abstract entity (cannot be instantiated, only inherited)
class Artifact(Entity):
    flags = TypeFlags(name="artifact", abstract=True)
    name: Name

# Inherited entity
class Document(Artifact):
    flags = TypeFlags(name="document")
    content: Content
```

### Relations

```python
from type_bridge import Relation, Role, TypeFlags

class Employment(Relation):
    flags = TypeFlags(name="employment")

    # Define roles with their player types
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Relations can own attributes too
    start_date: StartDate
    end_date: EndDate | None = None

# Entities must declare they play roles
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    # ... plays employment:employee (implicit from Role definition)

class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)
```

---

## CRUD Operations

### Entity Manager

```python
# Get manager for an entity type
manager = Person.manager(db)

# Insert
alice = Person(name=Name("Alice"), email=Email("alice@example.com"))
manager.insert(alice)

# Get all
all_persons = manager.all()

# Filter with dict
adults = manager.filter(age=Age(18)).all()

# Filter with expressions
seniors = manager.filter(Person.age.gte(Age(65))).all()

# Get first match
first = manager.filter(name=Name("Alice")).first()

# Count
total = manager.filter().count()

# Update (updates in-place)
alice.age = Age(31)
manager.update(alice)

# Delete
manager.delete(alice)

# Put (upsert - insert or update by key)
manager.put(Person(name=Name("Bob"), email=Email("bob@example.com")))
```

### Relation Manager

```python
# Get manager for relation type
emp_manager = Employment.manager(db)

# Insert relation (entities must exist)
alice = Person(name=Name("Alice"))
acme = Company(name=Name("Acme"))
employment = Employment(
    employee=alice,
    employer=acme,
    start_date=StartDate(date(2024, 1, 15))
)
emp_manager.insert(employment)

# Query relations
acme_employees = emp_manager.filter(employer=acme).all()
```

### Transactions

```python
# Explicit transaction for multiple operations
with db.transaction("write") as tx:
    person_mgr = Person.manager(tx)
    company_mgr = Company.manager(tx)

    alice = Person(name=Name("Alice"))
    acme = Company(name=Name("Acme"))

    person_mgr.insert(alice)
    company_mgr.insert(acme)
    # Auto-commits on exit, rolls back on exception
```

---

## Query Expressions

### Comparison Expressions

```python
# Available on all attribute field references
Person.age.eq(Age(30))      # ==
Person.age.neq(Age(30))     # !=
Person.age.gt(Age(18))      # >
Person.age.gte(Age(18))     # >=
Person.age.lt(Age(65))      # <
Person.age.lte(Age(65))     # <=

# String-specific
Person.name.contains("Ali")
Person.name.like("^A.*")    # Regex match

# Chaining filters (AND)
manager.filter(Person.age.gte(Age(18))).filter(Person.age.lt(Age(65))).all()
```

### Boolean Expressions

```python
from type_bridge.expressions import BooleanExpr

# OR
manager.filter(
    BooleanExpr.or_(
        Person.name.eq(Name("Alice")),
        Person.name.eq(Name("Bob"))
    )
).all()

# NOT
manager.filter(
    BooleanExpr.not_(Person.status.eq(Status("inactive")))
).all()
```

### Aggregations

```python
# Single aggregation
result = manager.filter().aggregate(Person.age.avg())
avg_age = result["avg_age"]

# Multiple aggregations
result = manager.filter().aggregate(
    Person.age.avg(),
    Person.salary.sum(),
    Person.score.max()
)

# Available: .avg(), .sum(), .min(), .max(), .count(), .std(), .median()
```

### Group By

```python
# Group by single field
result = manager.group_by(Person.department).aggregate(
    Person.salary.avg(),
    Person.age.avg()
)
# Returns: {"Engineering": {"avg_salary": 95000, "avg_age": 32}, ...}

# Group by multiple fields
result = manager.group_by(Person.department, Person.level).aggregate(
    Person.salary.avg()
)
```

### Pagination

```python
# Limit and offset
page = manager.filter().limit(10).offset(20).all()

# First N results
top_5 = manager.filter(Person.score.gte(Score(90))).limit(5).all()
```

---

## Schema Management

```python
from type_bridge import SchemaManager

schema = SchemaManager(db)

# Register models
schema.register(Person, Company, Employment)

# Sync schema (creates types in TypeDB)
schema.sync_schema()

# Force sync (recreates even if exists)
schema.sync_schema(force=True)

# Get current schema
current = schema.get_schema()

# Compare schemas
from type_bridge import SchemaDiff
diff = SchemaDiff.compare(old_schema, new_schema)
```

---

## Built-in Functions (TypeDB 3.8+)

```python
from type_bridge.expressions import iid, label

# These are used internally by type-bridge for IID and type fetching
# The library handles this automatically in manager operations

# Direct usage in custom queries:
expr = iid("$e")       # Generates: iid($e)
expr = label("$t")     # Generates: label($t)

# IMPORTANT: label() only works on TYPE variables, not instance variables
# Pattern: $e isa! $t; $t sub person; ... label($t)
```

---

## Common Patterns

### Get by IID

```python
# Fetch entity by internal ID (fast direct lookup)
person = manager.get_by_iid("0x1e00000000000000000123")
```

### Polymorphic Queries

```python
# Query abstract type to get all subtypes
class Animal(Entity):
    flags = TypeFlags(name="animal", abstract=True)

class Dog(Animal):
    flags = TypeFlags(name="dog")

class Cat(Animal):
    flags = TypeFlags(name="cat")

# Gets both Dogs and Cats
animal_manager = Animal.manager(db)
all_animals = animal_manager.all()
```

### Serialization

```python
# Convert to dict
person_dict = person.to_dict()

# Create from dict
person = Person.from_dict(person_dict)
```

### Raw Queries

```python
# Execute raw TypeQL
results = db.execute_query("""
    match $p isa person, has name $n;
    fetch { "name": $n };
""", "read")
```

---

## Cardinality Flags

```python
from type_bridge import Flag, Key, Unique, Card

class Person(Entity):
    # Key: exactly one, unique identifier
    id: PersonId = Flag(Key)

    # Unique: exactly one, unique but not key
    email: Email = Flag(Unique)

    # Card(0..1): optional (0 or 1)
    nickname: Nickname | None = Flag(Card(max=1))

    # Card(1..): at least one required
    phone: Phone = Flag(Card(min=1))

    # Card(0..): zero or more (default for lists)
    tags: list[Tag] = Flag(Card(min=0))

    # Card(2..5): between 2 and 5
    references: list[Reference] = Flag(Card(min=2, max=5))
```

---

## Important Notes

1. **Keyword-only arguments**: All Entity/Relation constructors require keyword arguments

   ```python
   # Correct
   Person(name=Name("Alice"), age=Age(30))

   # Wrong - will fail
   Person(Name("Alice"), Age(30))
   ```

2. **Attribute instances**: Always wrap values in attribute types

   ```python
   # Correct
   person.age = Age(31)

   # Wrong
   person.age = 31
   ```

3. **TypeFlags required**: Entities and Relations need `flags = TypeFlags(name="...")`

4. **Connection management**: Use context managers or explicit connect/close

   ```python
   with Database(...) as db:
       # operations
   # Auto-closed
   ```

5. **Schema sync before data**: Always sync schema before inserting data

6. **Role player matching**: Relation CRUD operations identify role players using:
   - **IID (preferred)**: If the entity has `_iid` set (from being fetched from DB), uses fast IID matching
   - **Key attributes (fallback)**: If no IID, uses `Flag(Key)` attributes to identify the entity
   - **Error**: If neither is available, raises `ValueError` with clear guidance

   ```python
   # Pattern 1: Fetch entities first (uses IID matching - faster)
   alice = person_manager.filter(name=Name("Alice")).first()  # alice._iid is set
   emp = Employment(employee=alice, employer=company)
   emp_manager.insert(emp)  # Uses alice._iid for matching

   # Pattern 2: Create stub entities (uses key attribute matching)
   alice = Person(name=Name("Alice"))  # No _iid
   emp = Employment(employee=alice, employer=company)
   emp_manager.insert(emp)  # Uses name (key attr) for matching
   ```

7. **Transaction types**:
   - `"read"`: For queries (no commit needed)
   - `"write"`: For insert/update/delete (auto-commits)
   - `"schema"`: For schema changes (auto-commits)
