# Cardinality

Complete reference for cardinality constraints and the Flag system in TypeBridge.

## Overview

**Cardinality** defines how many values, role players, or played relations are
allowed at a TypeDB schema edge. TypeBridge provides the `Card` API and `Flag`
system for declaring constraints that map directly to TypeDB's `@card`
annotations.

## Card API

The `Card` class specifies minimum and maximum counts wherever TypeDB accepts
`@card`: owned attributes, relation roles, and player `plays` edges.

```python
from type_bridge import Card

# Positional arguments
Card(min, max)

# Keyword arguments
Card(min=N)        # At least N values
Card(max=N)        # At most N values
Card(min=N, max=M) # Between N and M values
```

### Card Constructors

```python
from type_bridge import Card

# Exact count
Card(1, 1)          # Exactly 1 → @card(1..1)
Card(2, 2)          # Exactly 2 → @card(2..2)

# Minimum bound
Card(min=1)         # At least 1 → @card(1..)
Card(min=2)         # At least 2 → @card(2..)

# Maximum bound
Card(max=5)         # At most 5 → @card(0..5)
Card(max=10)        # At most 10 → @card(0..10)

# Range
Card(1, 5)          # 1 to 5 → @card(1..5)
Card(2, 10)         # 2 to 10 → @card(2..10)
Card(min=2, max=8)  # 2 to 8 → @card(2..8)
```

## Cardinality Surfaces

TypeDB has three distinct `@card` surfaces. They look similar but constrain
different things:

| Surface | TypeQL form | Meaning |
|---------|-------------|---------|
| Owned attribute | `owns email @card(0..1)` | Attribute values per owner |
| Relates-side role | `relates employer @card(1..1)` | Players of that role per relation instance |
| Plays-side role | `company plays employment:employer @card(0..1)` | Relation instances a single player may play in that role |

Relates-side and plays-side cardinality are independent. A role can require one
employer per employment while also limiting each company to at most one
employment:

```python
from type_bridge import Card, Relation, Role, TypeFlags

class Employment(Relation):
    flags = TypeFlags(name="employment")

    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role(
        "employer",
        Company,
        cardinality=Card(1, 1),        # one employer per employment
        plays_cardinality=Card(0, 1),  # one employment per company
    )
```

**Generated TypeQL**:

```typeql
relation employment,
    relates employee,
    relates employer @card(1..1);

person plays employment:employee;
company plays employment:employer @card(0..1);
```

## Flag System

The `Flag` function combines cardinality with special annotations (Key, Unique):

```python
from type_bridge import Flag, Key, Unique, Card

# Key attribute (implies @card(1..1))
field: Type = Flag(Key)

# Unique and optional when absent
field: Type | None = Flag(Unique)

# Unique and required because the Python type is non-optional
required_field: Type = Flag(Unique)

# Multi-value with cardinality
field: list[Type] = Flag(Card(min=1))

# Key with multi-value
field: list[Type] = Flag(Key, Card(min=1))
```

## Cardinality Patterns

TypeBridge provides multiple patterns for specifying cardinality:

### Single-Value Attributes

#### Required Single Value (Default)

```python
# Pattern: Type (no annotation needed)
# Cardinality: @card(1..1) - exactly one
name: Name  # Required, exactly one
```

#### Optional Single Value

```python
# Pattern: Type | None with explicit = None
# Cardinality: @card(0..1) - zero or one
age: Age | None = None  # Optional, at most one
```

### Multi-Value Attributes

Multi-value attributes **must** use `list[Type]` with `Flag(Card(...))`:

#### At Least N Values

```python
# Pattern: list[Type] = Flag(Card(min=N))
# Cardinality: @card(N..) - at least N, unbounded
tags: list[Tag] = Flag(Card(min=1))     # @card(1..) - at least 1
admins: list[Admin] = Flag(Card(min=2)) # @card(2..) - at least 2
```

#### At Most N Values

```python
# Pattern: list[Type] = Flag(Card(max=N))
# Cardinality: @card(0..N) - zero to N
emails: list[Email] = Flag(Card(max=3))    # @card(0..3) - up to 3
phones: list[Phone] = Flag(Card(max=5))    # @card(0..5) - up to 5
```

#### Range of Values

```python
# Pattern: list[Type] = Flag(Card(min, max))
# Cardinality: @card(min..max) - min to max
jobs: list[Job] = Flag(Card(1, 5))         # @card(1..5) - 1 to 5
skills: list[Skill] = Flag(Card(min=2, max=10))  # @card(2..10) - 2 to 10
```

#### Zero or More Values

```python
# Pattern: list[Type] = Flag(Card(min=0))
# Cardinality: @card(0..) - zero or more (unbounded)
tags: list[Tag] = Flag(Card(min=0))  # @card(0..)
```

### Important: Card-based multi-value attributes are unordered sets

`Flag(Card(...))` multi-value attributes are **unordered sets**.  TypeBridge uses
`list[Type]` syntax for convenience, but the insertion order is never preserved.

```python
# You write this in Python:
person = Person(tags=[Tag("python"), Tag("rust"), Tag("go")])
manager.insert(person)

# But TypeDB stores it as an unordered set
# When you fetch, order is NEVER guaranteed:
fetched = manager.get(name="Alice")[0]
# fetched.tags might be: [Tag("rust"), Tag("go"), Tag("python")]
# or any other order - it's completely unpredictable
```

**Key points for `Flag(Card(...))`**:
- `list[Type]` is just Python syntax — internally the attribute set is unordered
- Order is never preserved or guaranteed
- Do not write code that depends on insertion order

### Ordered list attributes — `Flag(Ordered)` and `Flag(Ordered, Distinct)`

TypeDB 3.x introduced true *ordered* attribute ownership via `owns attr[]`.  Use
`Flag(Ordered)` or `Flag(Ordered, Distinct)` when you need a deterministically-ordered
list of attribute values:

```python
from type_bridge import Entity, Flag, Key, Ordered, Distinct, String, TypeFlags

class Tag(String):
    pass

class Article(Entity):
    flags = TypeFlags(name="article")
    # ordered list, duplicates allowed → owns tag[]
    category: list[Tag] = Flag(Ordered)
    # ordered list, no duplicates → owns tag[] @distinct
    label: list[Tag] = Flag(Ordered, Distinct)
```

**Engine caveat — REP256**: schema-side declarations are accepted, but instance-level
list operations (insert/read) are not yet implemented.  See
[Attributes — List Attributes](attributes.md#list-attributes) for the full caveat.

## Special Annotations

### Key Annotation

The `Key` annotation marks an attribute as a key (unique identifier):

```python
from type_bridge import Flag, Key

class Person(Entity):
    # Key implies @card(1..1) - required and unique
    user_id: UserID = Flag(Key)
```

**Generated TypeQL**:

```typeql
entity person,
    owns user_id @key;
```

**Properties**:
- Implies `@card(1..1)` (exactly one)
- Enforces uniqueness across all instances
- Used for entity identification

### Unique Annotation

The `Unique` annotation enforces uniqueness without making it a key:

```python
from type_bridge import Flag, Unique

class Person(Entity):
    email: Email = Flag(Unique)  # Required by the non-optional type
```

**Generated TypeQL**:

```typeql
entity person,
    owns email @unique @card(1..1);
```

**Properties**:
- `@unique` constrains distinctness; it does not imply cardinality
- `Email` emits `@card(1..1)` while `Email | None` emits `@card(0..1)`
- Enforces uniqueness across all instances
- Can have multiple unique attributes per entity

### Documentation and Metadata Annotations

The `Doc` and `Meta` markers declare TypeDB 3.12+ `@doc`/`@meta`
annotations on an ownership:

```python
from type_bridge import Doc, Flag, Key, Meta

class Person(Entity):
    name: Name = Flag(Key, Doc("Full legal name."), Meta("column", "name"))
    nickname: Nick | None = Flag(Doc("Preferred short name."))
```

**Generated TypeQL**:

```typeql
entity person,
    owns name @key @doc("Full legal name.") @meta("column", "name"),
    owns nickname @card(0..1) @doc("Preferred short name.");
```

TypeDB stores at most one value per `@meta` key per subject. Type-level
annotations use `TypeFlags(doc=..., meta=...)` /
`AttributeFlags(doc=..., meta=...)` instead — see the
[entities](entities.md#documentation-and-metadata-docmeta) and
[attributes](attributes.md#documentation-and-metadata-docmeta) guides.
Annotated schemas require a TypeDB 3.12+ server (see the
[schema guide](schema.md#schema-annotations-and-server-versions)).

### Combining Key/Unique with Card

```python
from type_bridge import Flag, Key, Unique, Card

class Person(Entity):
    # Key with multi-value
    ids: list[ID] = Flag(Key, Card(min=1))  # @key @card(1..)

    # Unique with custom cardinality
    emails: list[Email] = Flag(Unique, Card(1, 3))  # @unique @card(1..3)
```

## Cardinality Rules

### Rule 1: `Flag(Card(...))` Only with `list[Type]`

Multi-value attributes must use `list[Type]`:

```python
# ✅ CORRECT: list[Type] with Card
tags: list[Tag] = Flag(Card(min=2))

# ❌ WRONG: Card without list[Type]
age: Age = Flag(Card(0, 1))  # Use Age | None instead!
```

### Rule 2: `list[Type]` Must Have `Flag(Card(...))`

All multi-value attributes must specify cardinality:

```python
# ✅ CORRECT: list[Type] with Card
tags: list[Tag] = Flag(Card(min=1))

# ❌ WRONG: list[Type] without Card
tags: list[Tag]  # Error: missing cardinality!

# ❌ WRONG: Key/Unique alone is not enough
tags: list[Tag] = Flag(Key)  # Error: need Card too!
tags: list[Tag] = Flag(Key, Card(min=1))  # ✅ CORRECT
```

### Rule 3: Optional Single Values Use `Type | None`

For zero-or-one cardinality, use union types:

```python
# ✅ CORRECT: Type | None for optional
age: Age | None = None  # @card(0..1)

# ❌ WRONG: Don't use Card for single optional
age: Age = Flag(Card(0, 1))
```

### Rule 4: Explicit `= None` for Optional Fields

Optional fields must have explicit defaults:

```python
# ✅ CORRECT: Explicit default
age: Age | None = None

# ❌ WRONG: Missing default
age: Age | None
```

## Complete Examples

### Entity with Mixed Cardinality

```python
from type_bridge import (
    Entity, TypeFlags,
    String, Integer, Boolean,
    Flag, Key, Unique, Card
)

class UserID(String):
    pass

class Username(String):
    pass

class Email(String):
    pass

class Age(Integer):
    pass

class IsActive(Boolean):
    pass

class Role(String):
    pass

class Tag(String):
    pass

class User(Entity):
    flags = TypeFlags(name="user")

    # Key: exactly one, unique
    user_id: UserID = Flag(Key)

    # Required and unique but not key (required because the type is non-optional)
    email: Email = Flag(Unique)

    # Required: exactly one
    username: Username

    # Optional: zero or one
    age: Age | None = None
    is_active: IsActive | None = None

    # Multi-value: at least one
    roles: list[Role] = Flag(Card(min=1))

    # Multi-value: zero to five
    tags: list[Tag] = Flag(Card(max=5))
```

**Generated TypeQL**:

```typeql
define

attribute user_id, value string;
attribute username, value string;
attribute email, value string;
attribute age, value integer;
attribute is_active, value boolean;
attribute role, value string;
attribute tag, value string;

entity user,
    owns user_id @key,
    owns email @unique @card(1..1),
    owns username @card(1..1),
    owns age @card(0..1),
    owns is_active @card(0..1),
    owns role @card(1..),
    owns tag @card(0..5);
```

### Relation with Cardinality

```python
from type_bridge import Relation, TypeFlags, Role, Card

class Friendship(Relation):
    flags = TypeFlags(name="friendship")

    # Exactly 2 friends (symmetric relation)
    friend: Role[Person] = Role("friend", Person, cardinality=Card(2, 2))

    # Optional attributes
    since: StartDate | None = None
    is_active: IsActive | None = None

    # Multi-value attributes
    shared_interests: list[Interest] = Flag(Card(min=0))
```

**Generated TypeQL**:

```typeql
relation friendship,
    relates friend @card(2..2),
    owns since @card(0..1),
    owns is_active @card(0..1),
    owns shared_interests @card(0..);

person plays friendship:friend;
```

### Complex Cardinality Example

```python
from type_bridge import Entity, TypeFlags, Flag, Key, Unique, Card

class Product(Entity):
    flags = TypeFlags(name="product")

    # Key: product_id (exactly one, unique)
    product_id: ProductID = Flag(Key)

    # Required unique SKU (required because the type is non-optional)
    sku: SKU = Flag(Unique)

    # Required: name (exactly one)
    name: ProductName

    # Optional: description (zero or one)
    description: Description | None = None

    # Multi-value: at least one category
    categories: list[Category] = Flag(Card(min=1))

    # Multi-value: 1 to 5 images
    images: list[ImageURL] = Flag(Card(1, 5))

    # Multi-value: 0 to 10 tags
    tags: list[Tag] = Flag(Card(max=10))

    # Multi-value: at least 2 suppliers
    suppliers: list[SupplierID] = Flag(Card(min=2))
```

**Generated TypeQL**:

```typeql
entity product,
    owns product_id @key,
    owns sku @unique @card(1..1),
    owns name @card(1..1),
    owns description @card(0..1),
    owns category @card(1..),
    owns image_url @card(1..5),
    owns tag @card(0..10),
    owns supplier_id @card(2..);
```

## Cardinality Semantics

TypeBridge follows these cardinality semantics:

| Pattern | Annotation | Meaning |
|---------|------------|---------|
| `Type` | `@card(1..1)` | Exactly one (required) |
| `Type \| None` | `@card(0..1)` | Zero or one (optional) |
| `Flag(Key)` | `@key` | Exactly one, unique (implies `@card(1..1)`) |
| `Flag(Unique)` | `@unique` | Unique when present; the Python type supplies cardinality |
| `list[Type] = Flag(Card(min=N))` | `@card(N..)` | At least N, unbounded |
| `list[Type] = Flag(Card(max=N))` | `@card(0..N)` | Zero to N |
| `list[Type] = Flag(Card(min, max))` | `@card(min..max)` | Min to max |
| `Role(..., cardinality=Card(min, max))` | `relates role @card(min..max)` | Role players per relation instance |
| `Role(..., plays_cardinality=Card(min, max))` | `plays relation:role @card(min..max)` | Relation instances per player |

## Best Practices

### 1. Use Clear Cardinality Patterns

Follow the established patterns for clarity:

```python
# ✅ GOOD: Clear patterns
name: Name                          # Required (1..1)
age: Age | None = None              # Optional (0..1)
tags: list[Tag] = Flag(Card(min=1)) # Multi-value (1..)

# ❌ CONFUSING: Mixing patterns
name: Name | None = Name("default") # Don't provide default for optional
age: Age = Flag(Card(1, 1))         # Just use Age
```

### 2. Use Semantic Cardinality

Choose cardinality that reflects business logic:

```python
# ✅ GOOD: Reflects reality
class Person(Entity):
    # Everyone has exactly one birth date
    birth_date: BirthDate

    # Some people have a middle name, some don't
    middle_name: MiddleName | None = None

    # People can have multiple phone numbers
    phones: list[Phone] = Flag(Card(min=0))

# ❌ POOR: Doesn't reflect reality
class Person(Entity):
    birth_date: BirthDate | None = None  # Everyone has a birth date!
    middle_name: MiddleName               # Not everyone has one
```

### 3. Use Key for Primary Identifiers

Always use `Flag(Key)` for primary identifiers:

```python
# ✅ GOOD: Clear primary key
class User(Entity):
    user_id: UserID = Flag(Key)

# ❌ POOR: Unique without Key
class User(Entity):
    user_id: UserID = Flag(Unique)  # Should be Key
```

### 4. Use Unique for Secondary Identifiers

Use `Flag(Unique)` for fields that must be unique but aren't the primary key:

```python
class User(Entity):
    user_id: UserID = Flag(Key)       # Primary identifier
    email: Email = Flag(Unique)       # Required secondary identifier
    username: Username | None = Flag(Unique) # Optional secondary identifier
```

### 5. Choose Card-based set or ordered-list semantics explicitly

`list[Type]` describes the Python value shape for both forms. The ownership
flag determines the TypeDB schema semantics:

```python
class Person(Entity):
    # Ordinary owns tag @card(1..): unordered set semantics
    tags: list[Tag] = Flag(Card(min=1))

    # TypeDB 3.12 owns step[]: ordered schema semantics
    steps: list[Step] = Flag(Ordered)
```

**Key points**:

- `Flag(Card(...))` emits ordinary `owns attr @card(...)`; its values remain an
  unordered set, so application code must not depend on retrieval order.
- On TypeDB 3.12, `Flag(Ordered)` emits `owns attr[]` and declares ordered-list
  schema semantics.
- Schema-side ordered declarations are supported, but instance-level list
  insert/read operations remain subject to the
  [REP256 caveat](attributes.md#list-attributes).

## Deprecated APIs

The following cardinality APIs are deprecated:

```python
# ❌ DEPRECATED: Cardinal, Min, Max, Range
from type_bridge import Cardinal, Min, Max, Range

tags: Min[1, Tag]          # Use: list[Tag] = Flag(Card(min=1))
tags: Max[5, Tag]          # Use: list[Tag] = Flag(Card(max=5))
tags: Range[1, 5, Tag]     # Use: list[Tag] = Flag(Card(1, 5))
age: Optional[Age]         # Use: Age | None
```

Use modern `Card` API and PEP 604 union syntax instead.

## See Also

- [Attributes](attributes.md) - Attribute types that use cardinality
- [Entities](entities.md) - Entity ownership with cardinality constraints
- [Relations](relations.md) - Relations with role and attribute cardinality
- [Validation](validation.md) - Type validation and cardinality enforcement
