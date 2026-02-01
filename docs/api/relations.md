# Relations

Complete reference for defining relations in TypeBridge.

## Overview

**Relations** are first-class connections between entities in TypeDB. Unlike foreign keys in traditional databases, relations in TypeDB explicitly define role players and can own attributes themselves.

## Relation Base Class

The `Relation` base class provides the foundation for all relation types:

```python
from type_bridge import Relation

class Relation:
    """Base class for relations."""

    @classmethod
    def get_type_name(cls) -> str:
        """Returns type name from flags or lowercase class name."""

    @classmethod
    def get_supertype(cls) -> str | None:
        """Returns supertype from Python inheritance."""

    @classmethod
    def get_roles(cls) -> dict[str, Role]:
        """Returns mapping of role names to Role objects."""

    @classmethod
    def get_owned_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Returns mapping of field names to attribute info."""

    @classmethod
    def to_schema_definition(cls) -> str:
        """Generates relation schema with role and ownership declarations."""

    @classmethod
    def manager(cls, db: Database) -> RelationManager:
        """Creates a type-safe CRUD manager for this relation."""
```

## Basic Relation Definition

Define relations by inheriting from `Relation` and declaring roles and attribute ownership:

```python
from type_bridge import Relation, TypeFlags, Role, String

# Define entities
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)

class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)

# Define attribute for relation
class Position(String):
    pass

# Define relation
class Employment(Relation):
    flags = TypeFlags(name="employment")

    # Role players
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Owned attributes
    position: Position
```

**Generated TypeQL**:

```typeql
define

attribute position, value string;

relation employment,
    relates employee,
    relates employer,
    owns position @card(1..1);

person plays employment:employee;
company plays employment:employer;
```

## Role Syntax

Roles are defined using the `Role` class with type parameters:

```python
from type_bridge import Role

class Employment(Relation):
    # Syntax: field_name: Role[EntityType] = Role("role_name", EntityType)
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
```

**Components**:

- `field_name`: Python field name (e.g., `employee`, `employer`)
- `Role[EntityType]`: Type hint for role player type
- `Role("role_name", EntityType)`: Role definition with TypeDB role name and player type

## Type-Safe Role Access

**New in v0.9.0**: Access role player attributes with full type safety for query building.

### Class-Level vs Instance-Level Access

Role fields behave differently at class-level and instance-level:

```python
class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

# Class-level access returns RoleRef for query building
Employment.employee       # Returns RoleRef
Employment.employee.age   # Returns RolePlayerNumericFieldRef
Employment.employee.name  # Returns RolePlayerStringFieldRef

# Instance-level access returns actual entity instances
emp = Employment(employee=alice, employer=techcorp)
emp.employee              # Returns Person instance (alice)
emp.employer              # Returns Company instance (techcorp)
```

### Role Player Field References

Access role player attributes through the `RoleRef`:

```python
# Numeric field reference
age_ref = Employment.employee.age
# Type: RolePlayerNumericFieldRef

# String field reference
name_ref = Employment.employee.name
# Type: RolePlayerStringFieldRef
```

### Query Building with Role Player Fields

Use role player field references for type-safe filtering:

```python
manager = Employment.manager(db)

# Filter by role player attribute
results = manager.filter(
    Employment.employee.age.gte(Age(30))
).execute()

# String operations
results = manager.filter(
    Employment.employer.name.contains(Name("Tech"))
).execute()

# Combine multiple conditions
results = manager.filter(
    Employment.employee.age.gt(Age(25)),
    Employment.employer.name.like(Name("^Tech.*"))
).execute()
```

### Available Methods

**Numeric fields**:

- `.gt(value)`, `.lt(value)`, `.gte(value)`, `.lte(value)`
- `.eq(value)`, `.neq(value)`

**String fields**:

- `.contains(value)` - Substring match
- `.like(value)` - Regex pattern
- `.regex(value)` - Regex pattern (alias)

### Introspecting Role Attributes

Use `dir()` to discover available attributes:

```python
# List all available attributes on a role
print(dir(Employment.employee))
# ['name', 'age', 'email', ...]

# For multi-player roles, returns union of all player attributes
print(dir(Trace.origin))
# ['name', 'subject', 'sender', ...]  # From Document and Email
```

### Public Role Access

**New in v0.9.0**: Use `get_roles()` to access role definitions:

```python
# Get all roles
roles = Employment.get_roles()
# Returns: {'employee': Role(...), 'employer': Role(...)}

# Access role properties
role = roles['employee']
print(role.role_name)            # 'employee'
print(role.player_entity_types)  # (Person,)
```

## Multi-player Roles

A single role can be playable by multiple entity types without introducing an artificial supertype:

```python
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

- **Type hint**: Use a PEP 604 union for IDE/type-checker support (e.g., `Role[Document | Email]`).
- **API**: `Role.multi("role_name", TypeA, TypeB, ...)` requires at least two player types; otherwise it raises `ValueError`.
- **Runtime validation**: Assigning an instance of a disallowed type raises `TypeError`.
- **Schema emission**: Generates multiple `plays` entries for the single role:

```typeql
relation trace,
    relates origin;

document plays trace:origin;
email plays trace:origin;
```

### CRUD and queries

- Filtering or deleting by a multi-player role uses the actual player’s key attributes and works across all allowed entity types.
- Role uniqueness rules still apply: keep role names distinct (TypeDB requirement).

## TypeFlags Configuration

Configure relation metadata using `TypeFlags`:

```python
from type_bridge import TypeFlags

class Employment(Relation):
    flags = TypeFlags(
        name="employment",    # TypeDB type name (default: lowercase class name)
        abstract=False,            # Whether this is an abstract relation (default: False)
        case="snake_case"          # Type name case formatting (default: "snake_case")
    )
```

### Implicit TypeFlags

For simple relations, `TypeFlags` is automatically created if not specified:

```python
# These are equivalent:
class Employment(Relation):
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

class Employment(Relation):
    flags = TypeFlags()  # Not necessary
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
```

**When explicit flags ARE needed:**

- `abstract=True` - Abstract relations
- `base=True` - Python-only base classes
- Custom `name` - Override type name
- Custom `case` - Non-default case formatting

## Relation with Attributes

Relations can own attributes just like entities:

```python
from type_bridge import Relation, TypeFlags, Role, String, Integer, Date, Flag, Card

class Position(String):
    pass

class Salary(Integer):
    pass

class StartDate(Date):
    pass

class Skill(String):
    pass

class Employment(Relation):
    flags = TypeFlags(name="employment")

    # Roles
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Single-value attributes
    position: Position                    # @card(1..1) - required
    salary: Salary | None = None          # @card(0..1) - optional
    start_date: StartDate                 # @card(1..1) - required

    # Multi-value attributes
    skills: list[Skill] = Flag(Card(min=1))  # @card(1..) - at least one
```

**Generated TypeQL**:

```typeql
define

attribute position, value string;
attribute salary, value integer;
attribute start_date, value date;
attribute skill, value string;

relation employment,
    relates employee,
    relates employer,
    owns position @card(1..1),
    owns salary @card(0..1),
    owns start_date @card(1..1),
    owns skill @card(1..);

person plays employment:employee;
company plays employment:employer;
```

## Role Cardinality

Specify how many times an entity can play a role:

```python
from type_bridge import Relation, TypeFlags, Role, Card

class Friendship(Relation):
    flags = TypeFlags(name="friendship")

    # Both friends must be Person, exactly 2 role players
    friend: Role[Person] = Role("friend", Person, Card(2, 2))
```

**Generated TypeQL**:

```typeql
relation friendship,
    relates friend @card(2..2);

person plays friendship:friend;
```

## Python Inheritance for Relations

Relations support inheritance just like entities:

### Basic Relation Inheritance

```python
from type_bridge import Relation, TypeFlags, Role

class SocialRelation(Relation):
    flags = TypeFlags(name="social-relation", abstract=True)

    # Abstract role (will be overridden in subclasses)
    related: Role[Person] = Role("related", Person, Card(2))

class Friendship(SocialRelation):
    flags = TypeFlags(name="friendship")

    # Override role with specific semantics
    friend: Role[Person] = Role("friend", Person, Card(2))
```

**Generated TypeQL**:

```typeql
relation social-relation @abstract,
    relates related @card(2);

relation friendship, sub social-relation,
    relates friend as related @card(2);

person plays social-relation:related;
person plays friendship:friend;
```

### Role Specialization

Subrelations can specialize roles from parent relations:

```python
class Employment(Relation):
    flags = TypeFlags(name="employment", abstract=True)

    employee: Role[Person] = Role("employee", Person)
    employer: Role[Entity] = Role("employer", Entity)  # Generic employer

class CompanyEmployment(Employment):
    flags = TypeFlags(name="company-employment")

    # Specialize employer role to only Company entities
    employer: Role[Company] = Role("employer", Company)
```

## Abstract Relations

Abstract relations cannot be instantiated directly but serve as base types:

```python
from type_bridge import Relation, TypeFlags, Role

class Relation(Relation):
    flags = TypeFlags(name="relation", abstract=True)

    related: Role[Entity] = Role("related", Entity, Card(2))

class Friendship(Relation):
    flags = TypeFlags(name="friendship")

    friend: Role[Person] = Role("friend", Person, Card(2))
```

**Generated TypeQL**:

```typeql
relation relation @abstract,
    relates related @card(2);

relation friendship, sub relation,
    relates friend as related @card(2);
```

## Creating Relation Instances

Relations use keyword-only arguments for type safety:

```python
from datetime import date

# Create entity instances
alice = Person(name=Name("Alice"))
techcorp = Company(name=Name("TechCorp"))

# ✅ CORRECT: Keyword arguments with entity instances
employment = Employment(
    employee=alice,
    employer=techcorp,
    position=Position("Senior Engineer"),
    salary=Salary(120000),
    start_date=StartDate(date(2024, 1, 15)),
    skills=[Skill("Python"), Skill("TypeDB"), Skill("FastAPI")]
)

# ❌ WRONG: Positional arguments
employment = Employment(alice, techcorp, Position("Engineer"))  # Type error!
```

## Complete Example

```python
from type_bridge import (
    Entity, Relation, TypeFlags, Role,
    String, Integer, Date, Boolean,
    Flag, Key, Unique, Card
)
from datetime import date

# Define attribute types
class Name(String):
    pass

class Email(String):
    pass

class CompanyID(String):
    pass

class Position(String):
    pass

class Salary(Integer):
    pass

class StartDate(Date):
    pass

class IsActive(Boolean):
    pass

class Benefit(String):
    pass

# Define entities
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    email: Email = Flag(Unique)

class Company(Entity):
    flags = TypeFlags(name="company")
    company_id: CompanyID = Flag(Key)
    name: Name

# Define relation
class Employment(Relation):
    flags = TypeFlags(name="employment")

    # Roles
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Single-value attributes
    position: Position
    salary: Salary
    start_date: StartDate
    is_active: IsActive | None = None

    # Multi-value attributes
    benefits: list[Benefit] = Flag(Card(min=0))

# Create instances
alice = Person(
    name=Name("Alice Johnson"),
    email=Email("alice@example.com")
)

techcorp = Company(
    company_id=CompanyID("C001"),
    name=Name("TechCorp Inc.")
)

employment = Employment(
    employee=alice,
    employer=techcorp,
    position=Position("Senior Software Engineer"),
    salary=Salary(120000),
    start_date=StartDate(date(2024, 1, 15)),
    is_active=IsActive(True),
    benefits=[
        Benefit("Health Insurance"),
        Benefit("401k Match"),
        Benefit("Remote Work")
    ]
)
```

**Generated TypeQL**:

```typeql
define

attribute name, value string;
attribute email, value string;
attribute company_id, value string;
attribute position, value string;
attribute salary, value integer;
attribute start_date, value date;
attribute is_active, value boolean;
attribute benefit, value string;

entity person,
    owns name @key,
    owns email @unique;

entity company,
    owns company_id @key,
    owns name @card(1..1);

relation employment,
    relates employee,
    relates employer,
    owns position @card(1..1),
    owns salary @card(1..1),
    owns start_date @card(1..1),
    owns is_active @card(0..1),
    owns benefit @card(0..);

person plays employment:employee;
company plays employment:employer;
```

## Abstract Entity Types in Role Definitions

Relations can use abstract entity types in role definitions, allowing any subtype to play the role:

```python
from type_bridge import Entity, Relation, TypeFlags, Role

# Abstract base entity
class Content(Entity):
    flags = TypeFlags(name="content", abstract=True)
    title: Title

# Concrete subtypes
class Article(Content):
    flags = TypeFlags(name="article")
    body: Body

class Video(Content):
    flags = TypeFlags(name="video")
    url: URL

# Relation using abstract type
class Authorship(Relation):
    flags = TypeFlags(name="authorship")

    author: Role[Person] = Role("author", Person)
    content: Role[Content] = Role("content", Content)  # Abstract type!

# Usage - any Content subtype can play the role
article = Article(title=Title("Python Guide"))
video = Video(title=Title("TypeDB Tutorial"), url=URL("https://..."))
author = Person(name=Name("Alice"))

# Both work because Article and Video inherit from Content
article_authorship = Authorship(author=author, content=article)
video_authorship = Authorship(author=author, content=video)
```

**Generated TypeQL**:

```typeql
relation authorship,
    relates author,
    relates content;

person plays authorship:author;
content plays authorship:content;  # Abstract type in role player definition
```

### Polymorphic Role Player Type Resolution

When querying relations with polymorphic role players, TypeBridge resolves each role player to its **concrete type**, not the abstract declared type. This enables type-safe access to subtype-specific attributes:

```python
# Query authorships - role players are resolved to concrete types
authorships = Authorship.manager(db).all()

for authorship in authorships:
    content = authorship.content

    # content is Article, Video, or other concrete subtype - NOT abstract Content
    if isinstance(content, Article):
        print(f"Article body: {content.body}")
    elif isinstance(content, Video):
        print(f"Video URL: {content.url}")

    # Common attributes from abstract type are always accessible
    print(f"Title: {content.title}")
```

**How it works**:

TypeBridge uses TypeDB's `label()` built-in function to fetch the actual type of each role player, then resolves it to the correct Python class. This happens automatically in:

- `RelationManager.get()` - filter-based queries
- `RelationManager.all()` - fetch all relations
- `RelationManager.get_by_iid()` - IID-based lookup
- `RelationQuery.execute()` - chainable query execution

**Benefits**:

- Access concrete type-specific attributes directly
- Use `isinstance()` for type-safe branching
- Proper Python type inference in IDEs
- No need to manually resolve types from IIDs

## Best Practices

### 1. Use Descriptive Role Names

Choose role names that clearly describe the relationship:

```python
# ✅ GOOD: Clear role names
class Employment(Relation):
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

# ❌ POOR: Generic role names
class Employment(Relation):
    person: Role[Person] = Role("person", Person)
    company: Role[Company] = Role("company", Company)
```

### 2. Match Field Name to Role Name

For consistency, use the same name for the field and role:

```python
# ✅ GOOD: Consistent naming
employee: Role[Person] = Role("employee", Person)

# ⚠️ CONFUSING: Different names
emp: Role[Person] = Role("employee", Person)
```

### 3. Use Abstract Relations for Hierarchies

Create abstract base relations for common patterns:

```python
class SocialRelation(Relation):
    flags = TypeFlags(abstract=True)
    related: Role[Person] = Role("related", Person, Card(2))

class Friendship(SocialRelation):
    friend: Role[Person] = Role("friend", Person, Card(2))

class Partnership(SocialRelation):
    partner: Role[Person] = Role("partner", Person, Card(2))
```

### 4. Explicit Defaults for Optional Attributes

Always provide `= None` for optional attributes on relations:

```python
# ✅ CORRECT
salary: Salary | None = None

# ❌ WRONG
salary: Salary | None
```

## See Also

- [Entities](entities.md) - How to define entities that play roles
- [Attributes](attributes.md) - Attributes that relations can own
- [Abstract Types](abstract_types.md) - Working with abstract relations and polymorphic roles
- [Cardinality](cardinality.md) - Cardinality constraints for roles and attributes
- [CRUD Operations](crud.md) - Working with relations in the database
- [Queries](queries.md) - Querying relations and role players
