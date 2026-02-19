# Entities

Complete reference for defining entities in TypeBridge.

## Overview

**Entities** are independent objects in TypeDB that own attributes. In TypeBridge, entities are defined as Python classes that inherit from the `Entity` base class and declare ownership of attribute types.

## Entity Base Class

The `Entity` base class provides the foundation for all entity types:

```python
from type_bridge import Entity

class Entity:
    """Base class for entities."""

    @classmethod
    def get_type_name(cls) -> str:
        """Returns type name from flags or lowercase class name."""

    @classmethod
    def get_supertype(cls) -> str | None:
        """Returns supertype from Python inheritance."""

    @classmethod
    def get_owned_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Returns mapping of field names to attribute info."""

    @classmethod
    def to_schema_definition(cls) -> str:
        """Generates entity schema with ownership declarations."""

    @classmethod
    def manager(cls, db: Database) -> EntityManager:
        """Creates a type-safe CRUD manager for this entity."""

    def to_dict(
        self,
        *,
        include: set[str] | None = None,
        exclude: set[str] | None = None,
        by_alias: bool = False,
        exclude_unset: bool = False,
    ) -> dict[str, Any]:
        """Serialize to primitives with optional alias names."""

    @classmethod
    def from_dict(
        cls,
        data: dict[str, Any],
        *,
        field_mapping: dict[str, str] | None = None,
        strict: bool = True,
    ) -> Self:
        """Construct an entity from primitives with optional field mapping."""
```

## Basic Entity Definition

Define entities by inheriting from `Entity` and declaring attribute ownership:

```python
from type_bridge import Entity, TypeFlags, String, Integer, Flag, Key

# 1. Define attribute types
class Name(String):
    pass

class Age(Integer):
    pass

class Email(String):
    pass

# 2. Define entity with ownership
class Person(Entity):
    flags = TypeFlags(name="person")

    name: Name = Flag(Key)        # @key (required, unique)
    age: Age | None = None         # @card(0..1) (optional)
    email: Email                   # @card(1..1) (required, default)
```

**Generated TypeQL**:

```typeql
define

attribute name, value string;
attribute age, value integer;
attribute email, value string;

entity person,
    owns name @key,
    owns age @card(0..1),
    owns email @card(1..1);
```

## Serialization Helpers

Entities support safe round-trip serialization for API payloads and logs:

```python
person = Person(name=Name("Alice"), age=Age(30))

# Default field names
person.to_dict()
# {'name': 'Alice', 'age': 30, 'email': None}

# Alias names (uses AttributeFlags/name or case rules)
person.to_dict(by_alias=True)

# Include/exclude and unset handling
person.to_dict(include={"name"})
person.to_dict(exclude_unset=True)

# From raw data with optional mapping
payload = {"display-id": "US-1", "name": "Roadmap"}
item = Artifact.from_dict(payload, field_mapping={"display-id": "display_id"})

# Round-trip with aliases preserved
data = item.to_dict(by_alias=True)
restored = Artifact.from_dict(data)
assert restored == item

# Field mapping and relaxed unknown handling
payload = {
    "display-id": "US-42",
    "created-at": "2024-01-01T00:00:00Z",
    "extra": "ignore me",
}
artifact = Artifact.from_dict(
    payload,
    field_mapping={"display-id": "display_id", "created-at": "created_at"},
    strict=False,  # skip unknown "extra"
)
```

Behavior:
- Unwraps Attribute instances to `.value`, handling lists.
- `include`/`exclude` filter fields; `exclude_unset` hides fields never assigned.
- `field_mapping` maps external keys to internal field names; `strict=False` ignores unknowns.
- Empty strings/None are skipped during `from_dict`.
- Backed by Pydantic `model_dump`/`model_construct` for consistent validation and alias handling.

## TypeFlags Configuration

Configure entity metadata using `TypeFlags`:

```python
from type_bridge import TypeFlags

class Person(Entity):
    flags = TypeFlags(
        name="person",        # TypeDB type name (default: lowercase class name)
        abstract=False,            # Whether this is an abstract entity (default: False)
        case="snake_case"          # Type name case formatting (default: "snake_case")
    )
```

### Type Name Configuration

By default, TypeBridge uses the lowercase class name:

```python
class Person(Entity):
    pass  # Type name: "person"

class UserAccount(Entity):
    pass  # Type name: "useraccount"
```

Override with `TypeFlags`:

```python
class Person(Entity):
    flags = TypeFlags(name="human")  # Type name: "human"

class UserAccount(Entity):
    flags = TypeFlags(
        name="user_account",  # Explicit name
        case="snake_case"          # Or use case formatting
    )
```

### Case Formatting Options

Supported case formats for automatic type name conversion:

- `"snake_case"`: `user_account` (default)
- `"kebab-case"`: `user-account`
- `"camelCase"`: `userAccount`
- `"PascalCase"`: `UserAccount`
- `"lower"`: `useraccount`
- `"UPPER"`: `USERACCOUNT`

```python
class UserAccount(Entity):
    flags = TypeFlags(case="kebab-case")  # Type name: "user-account"
```

### Implicit TypeFlags

For simple entities, `TypeFlags` is automatically created if not specified:

```python
# These are equivalent:
class Person(Entity):
    name: Name = Flag(Key)  # TypeFlags() is implicit

class Person(Entity):
    flags = TypeFlags()  # Explicit but not necessary
    name: Name = Flag(Key)
```

**When explicit flags ARE needed:**
- `abstract=True` - Abstract entities
- `base=True` - Python-only base classes (see below)
- Custom `name` - Override type name
- Custom `case` - Non-default case formatting

### Python-Only Base Classes (base=True)

Use `base=True` for intermediate Python base classes that should NOT appear in the TypeDB schema:

```python
class SharedMixin(Entity):
    flags = TypeFlags(base=True)  # Won't appear in TypeDB schema

    def shared_method(self):
        return "Shared functionality"

class Person(SharedMixin):
    # Gets Entity as supertype, skipping SharedMixin
    name: Name = Flag(Key)
```

**Use cases:**
- Shared methods and utility functions
- Python-side inheritance without TypeDB supertypes
- Schema filtering for cleaner database design
- Avoiding conflicting type names in TypeDB

**Behavior:**
- Base classes are excluded from `SchemaManager.registered_types`
- Child classes resolve to their first non-base ancestor as supertype
- Use `entity.is_base()` to check if an entity type is a base class

## Ownership Model

In TypeBridge (and TypeDB), entities **own** attributes rather than defining them inline:

```python
# Step 1: Define attribute types (independent types)
class Name(String):
    pass

class Email(String):
    pass

# Step 2: Multiple entities can own the same attribute
class Person(Entity):
    name: Name    # Person owns 'name'
    email: Email  # Person owns 'email'

class Company(Entity):
    name: Name    # Company also owns 'name'
    email: Email  # Company also owns 'email'
```

**Generated TypeQL** (attributes defined once):

```typeql
define

attribute name, value string;
attribute email, value string;

entity person,
    owns name @card(1..1),
    owns email @card(1..1);

entity company,
    owns name @card(1..1),
    owns email @card(1..1);
```

## Python Inheritance Mapping

TypeBridge automatically maps Python inheritance to TypeDB supertypes:

### Basic Inheritance

```python
from type_bridge import Entity, TypeFlags

class Animal(Entity):
    flags = TypeFlags(name="animal")
    name: Name

class Dog(Animal):
    flags = TypeFlags(name="dog")
    breed: Breed

class Cat(Animal):
    flags = TypeFlags(name="cat")
    color: Color
```

**Generated TypeQL**:

```typeql
entity animal,
    owns name @card(1..1);

entity dog, sub animal,
    owns breed @card(1..1);

entity cat, sub animal,
    owns color @card(1..1);
```

### Multi-Level Inheritance

```python
class Content(Entity):
    flags = TypeFlags(name="content", abstract=True)
    id: ID = Flag(Key)

class Page(Content):
    flags = TypeFlags(name="page", abstract=True)
    page_id: PageID
    bio: Bio

class Person(Page):
    flags = TypeFlags(name="person")
    email: Email
```

**Generated TypeQL**:

```typeql
entity content @abstract,
    owns id @key;

entity page @abstract, sub content,
    owns page_id @card(1..1),
    owns bio @card(1..1);

entity person, sub page,
    owns email @card(1..1);
```

## Abstract Entities

Abstract entities cannot be instantiated directly but serve as base types:

```python
from type_bridge import Entity, TypeFlags

class Animal(Entity):
    flags = TypeFlags(name="animal", abstract=True)
    name: Name

class Dog(Animal):
    flags = TypeFlags(name="dog")
    breed: Breed
```

**Generated TypeQL**:

```typeql
entity animal @abstract,
    owns name @card(1..1);

entity dog, sub animal,
    owns breed @card(1..1);
```

**Usage**:

```python
# ❌ Cannot instantiate abstract entity
animal = Animal(name=Name("Rex"))  # Error!

# ✅ Can instantiate concrete subtype
dog = Dog(name=Name("Rex"), breed=Breed("Golden Retriever"))
```

## Creating Entity Instances

Entities use keyword-only arguments for type safety:

```python
from type_bridge import Entity, TypeFlags, String, Integer, Flag, Key

class Name(String):
    pass

class Age(Integer):
    pass

class Email(String):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None
    email: Email

# ✅ CORRECT: Keyword arguments
alice = Person(
    name=Name("Alice Johnson"),
    age=Age(30),
    email=Email("alice@example.com")
)

# ✅ CORRECT: Optional fields can be omitted
bob = Person(
    name=Name("Bob Smith"),
    email=Email("bob@example.com")
)

# ❌ WRONG: Positional arguments not allowed
charlie = Person(Name("Charlie"), Age(25), Email("charlie@example.com"))  # Type error!
```

## Optional Fields Require Explicit Defaults

Optional fields (marked with `| None`) must have an explicit `= None` default:

```python
# ✅ CORRECT: Explicit defaults
class Person(Entity):
    name: Name = Flag(Key)          # Required field
    age: Age | None = None           # Optional with explicit = None
    email: Email | None = None       # Optional with explicit = None

# ❌ WRONG: Missing defaults
class Person(Entity):
    name: Name = Flag(Key)
    age: Age | None                  # Type error: missing default!
    email: Email | None              # Type error: missing default!
```

## Complete Example

```python
from type_bridge import (
    Entity, TypeFlags,
    String, Integer, Boolean,
    Flag, Key, Unique, Card
)

# Define attribute types
class UserID(String):
    pass

class Username(String):
    pass

class Email(String):
    pass

class Age(Integer):
    pass

class IsVerified(Boolean):
    pass

class Tag(String):
    pass

# Define entity with various attribute patterns
class User(Entity):
    flags = TypeFlags(name="user")

    # Key attribute (required, unique, exactly one)
    user_id: UserID = Flag(Key)

    # Unique attribute (required, unique, exactly one)
    email: Email = Flag(Unique)

    # Required single-value attribute (exactly one)
    username: Username

    # Optional single-value attribute (at most one)
    age: Age | None = None
    is_verified: IsVerified | None = None

    # Multi-value attribute (at least 1)
    tags: list[Tag] = Flag(Card(min=1))

# Create instance
user = User(
    user_id=UserID("u123"),
    email=Email("alice@example.com"),
    username=Username("alice"),
    age=Age(30),
    is_verified=IsVerified(True),
    tags=[Tag("python"), Tag("typedb"), Tag("developer")]
)
```

**Generated TypeQL**:

```typeql
define

attribute user_id, value string;
attribute username, value string;
attribute email, value string;
attribute age, value integer;
attribute is_verified, value boolean;
attribute tag, value string;

entity user,
    owns user_id @key,
    owns email @unique,
    owns username @card(1..1),
    owns age @card(0..1),
    owns is_verified @card(0..1),
    owns tag @card(1..);
```

**⚠️ Note**: Multi-value attributes (like `tags` above) are **unordered sets**. TypeDB has no list type - only sets. TypeBridge uses `list[Type]` syntax for convenience, but order is never preserved. See [Cardinality](cardinality.md#important-no-lists-in-typedb-only-sets) for details.

## Best Practices

### 1. Use Distinct Attribute Types

Each semantic field should use a distinct attribute type to avoid ownership conflicts:

```python
# ✅ CORRECT: Distinct types
class CreatedStamp(DateTime):
    pass

class ModifiedStamp(DateTime):
    pass

class Issue(Entity):
    created: CreatedStamp
    modified: ModifiedStamp

# ❌ WRONG: Duplicate attribute types
class TimeStamp(DateTime):
    pass

class Issue(Entity):
    created: TimeStamp   # Error: duplicate attribute type
    modified: TimeStamp  # TypeDB sees only one ownership
```

### 2. Use TypeFlags for Configuration

Always use `TypeFlags` instead of deprecated dunder attributes:

```python
# ✅ CORRECT: TypeFlags API
class Person(Entity):
    flags = TypeFlags(name="person", abstract=False)

# ❌ DEPRECATED: Dunder attributes
class Person(Entity):
    __type_name__ = "person"
    __abstract__ = False
```

### 3. Explicit Defaults for Optional Fields

Always provide `= None` for optional fields:

```python
# ✅ CORRECT
age: Age | None = None

# ❌ WRONG
age: Age | None
```

### 4. Use Modern Python Syntax

Use PEP 604 union syntax (`X | Y`) instead of `Optional[X]`:

```python
# ✅ MODERN: PEP 604 syntax
age: Age | None = None

# ❌ OLD: Optional syntax
from typing import Optional
age: Optional[Age] = None
```

## See Also

- [Attributes](attributes.md) - Attribute types and value types
- [Relations](relations.md) - How to define relations
- [Abstract Types](abstract-types.md) - Working with abstract entities and inheritance
- [Cardinality](cardinality.md) - Cardinality constraints
- [CRUD Operations](crud.md) - Working with entities in the database
- [Queries](queries.md) - Querying entities
