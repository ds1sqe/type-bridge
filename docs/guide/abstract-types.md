# Abstract Types API

This document covers how to use abstract types in TypeBridge, including implementation patterns and known issues.

For conceptual understanding of abstract types in TypeDB, see [docs/development/abstract-types.md](../development/abstract-types.md).

## Table of Contents

1. [Defining Abstract Types](#defining-abstract-types)
2. [Querying Abstract Types](#querying-abstract-types)
3. [Common Patterns](#common-patterns)
4. [Known Issues and Solutions](#known-issues-and-solutions)
5. [Summary](#summary)

---

## Defining Abstract Types

Use `TypeFlags(abstract=True)` to mark entities, relations, or attributes as abstract:

```python
from type_bridge import Entity, Relation, String, TypeFlags, Flag, Key

class ISBN(String):
    """Abstract ISBN attribute."""
    flags = TypeFlags(name="isbn", abstract=True)

class ISBN10(ISBN):
    """Concrete ISBN-10."""
    flags = TypeFlags(name="isbn-10")

class ISBN13(ISBN):
    """Concrete ISBN-13."""
    flags = TypeFlags(name="isbn-13")

class Book(Entity):
    """Abstract book entity."""
    flags = TypeFlags(name="book", abstract=True)
    isbn: ISBN = Flag(Key)

class Paperback(Book):
    """Concrete paperback book."""
    flags = TypeFlags(name="paperback")

class Hardback(Book):
    """Concrete hardback book."""
    flags = TypeFlags(name="hardback")
```

### Inherited Attributes

**Critical**: Concrete subtypes inherit attributes from abstract parents. When creating instances, you must use the concrete attribute types:

```python
# ✅ CORRECT: Use concrete attribute type
paperback = Paperback(isbn=ISBN10("0123456789"))

# ❌ WRONG: Cannot instantiate abstract attribute
paperback = Paperback(isbn=ISBN("0123456789"))  # Error!
```

---

## Querying Abstract Types

TypeBridge managers support polymorphic queries:

```python
# Create manager for abstract type
book_manager = Book.manager(db)

# Query returns ALL concrete subtypes (paperback, hardback, etc.)
all_books = book_manager.all()

# Filter by inherited attribute
books_with_isbn = book_manager.filter(isbn="0123456789")
```

---

## Common Patterns

### Pattern 1: Abstract Base with Common Attributes

Define common attributes once on an abstract parent:

```python
class Token(Entity):
    """Abstract base for all token types."""
    flags = TypeFlags(name="token", abstract=True)
    text: TokenText = Flag(Key)
    confidence: Confidence | None

class Symptom(Token):
    """Concrete symptom token."""
    flags = TypeFlags(name="symptom")

class Problem(Token):
    """Concrete problem token."""
    flags = TypeFlags(name="problem")

class Hypothesis(Token):
    """Concrete hypothesis token."""
    flags = TypeFlags(name="hypothesis")
```

**Benefits**:
- DRY principle - define `text` and `confidence` once
- Polymorphic queries - `Token.manager(db).all()` returns all token types
- Type safety - each concrete type is distinct

### Pattern 2: Abstract Relations with Polymorphic Roles

Use abstract types in role definitions for flexibility:

```python
class TokenOrigin(Relation):
    """Links tokens to their source documents."""
    flags = TypeFlags(name="token_origin")
    token: Role[Token] = Role("token", Token)  # Abstract type!
    document: Role[Document] = Role("document", Document)

# Works with ANY concrete token type
symptom = Symptom(text=TokenText("fever"))
doc = Document(id=DocId("DOC-123"))
origin = TokenOrigin(token=symptom, document=doc)
```

**Benefits**:
- Flexible - accepts any token subtype (Symptom, Problem, Hypothesis)
- Single relation type - no need for SymptomOrigin, ProblemOrigin, etc.
- Polymorphic queries - find origins for any token type

### Pattern 3: Interface Hierarchies for Polymorphism

Avoid redundant interfaces by using unified abstract types:

```python
# ❌ BAD: Redundant interfaces
class Location(Relation):
    relates located
    relates location

class Publishing(Relation):
    relates published
    relates location  # Redundant with Location.location!

# ✅ GOOD: Nested relations with unified interface
class Locating(Relation):
    """Abstract location relation."""
    flags = TypeFlags(name="locating", abstract=True)
    relates located
    relates location

class CityLocation(Locating):
    """City is located in country."""
    flags = TypeFlags(name="city_location")

class Publishing(Relation):
    """Publishing plays 'located' role in Locating."""
    # Publishing instances can play the 'located' role
    pass
```

---

## Known Issues and Solutions

### Issue: Relation Insertion with Abstract Role Types (FIXED)

**Problem**: When a relation uses an abstract type in a role definition (e.g., `Role[Token]`), and you try to insert the relation with a concrete entity (e.g., `Symptom`), the insertion fails with:

```
[INF10] Typing information for the variable 'token' is not available.
```

**Root Cause**: The `RelationManager.insert()` method used `get_owned_attributes()` to find key attributes for matching entities. When an entity inherits its key attribute from an abstract parent (e.g., `Symptom` inherits `text` from `Token`), `get_owned_attributes()` returns an empty dict, so no match clause is generated.

**Solution**: Changed `RelationManager.insert()` and `insert_many()` to use `get_all_attributes()` instead of `get_owned_attributes()`. This includes inherited attributes when building match clauses:

```python
# Before (BROKEN):
key_attrs = {
    field_name: attr_info
    for field_name, attr_info in entity.__class__.get_owned_attributes().items()
    if attr_info.flags.is_key
}

# After (FIXED):
key_attrs = {
    field_name: attr_info
    for field_name, attr_info in entity.__class__.get_all_attributes().items()
    if attr_info.flags.is_key
}
```

**Generated TypeQL (after fix)**:

```typeql
match
$token isa symptom, has TokenText "fever";
$doc isa document, has DocId "DOC-123";
insert
(token: $token, document: $doc) isa token_origin;
```

### Best Practice: Use Concrete Types for Instances

Always instantiate with **concrete types**, never abstract types:

```python
# ✅ CORRECT
symptom = Symptom(text=TokenText("fever"))

# ❌ WRONG - Cannot instantiate abstract type
token = Token(text=TokenText("fever"))  # Error!
```

### Best Practice: get_all_attributes() for Inherited Properties

When working with entities that may inherit attributes, always use `get_all_attributes()` instead of `get_owned_attributes()`:

```python
# ✅ CORRECT: Includes inherited attributes
all_attrs = entity.__class__.get_all_attributes()

# ❌ WRONG: Only includes attributes defined directly on the class
owned_attrs = entity.__class__.get_owned_attributes()
```

---

## Summary

1. **Abstract types cannot be instantiated** but can be queried polymorphically
2. **Use `TypeFlags(abstract=True)`** to define abstract entities, relations, and attributes
3. **Concrete subtypes inherit attributes** from abstract parents
4. **Always instantiate with concrete types**, never abstract types
5. **Use `get_all_attributes()` for inherited properties** in queries and CRUD operations
6. **Abstract types in role definitions** enable flexible, polymorphic relations
7. **Polymorphic queries work seamlessly** with abstract type managers

For conceptual understanding, see [docs/development/abstract-types.md](../development/abstract-types.md).
