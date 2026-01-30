# CRUD Operations

Complete reference for Create, Read, Update, Delete operations in TypeBridge.

## Overview

TypeBridge provides type-safe CRUD managers for entities and relations with a modern fetching API. All operations preserve type information and generate optimized TypeQL queries.

> **Note**: The CRUD module has been refactored into a modular structure for better maintainability, but all imports remain backward compatible. You can continue using `from type_bridge import EntityManager, RelationManager` as before.

## EntityManager

Type-safe manager for entity CRUD operations.

### Creating a Manager

```python
from type_bridge import Database, Entity, TypeFlags

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None

# Connect to database
db = Database(address="localhost:1729", database="mydb")
db.connect()

# Create manager
person_manager = Person.manager(db)
```

### EntityManager Methods

```python
class EntityManager[E: Entity]:
    def insert(self, entity: E) -> E:
        """Insert a single entity."""

    def insert_many(self, entities: list[E]) -> list[E]:
        """Insert multiple entities (bulk operation)."""

    def update(self, entity: E) -> E:
        """Update a single entity."""

    def update_many(self, entities: list[E]) -> list[E]:
        """Update multiple entities in one transaction."""

    def put(self, entity: E) -> E:
        """Put a single entity (idempotent insert)."""

    def put_many(self, entities: list[E]) -> list[E]:
        """Put multiple entities (idempotent bulk operation)."""

    def get(self, **filters) -> list[E]:
        """Get entities matching attribute filters."""

    def filter(self, **filters) -> EntityQuery[E]:
        """Create chainable query with filters."""

    def all(self) -> list[E]:
        """Get all entities of this type."""

    def delete(self, entity: E) -> E:
        """Delete entity by instance. Returns deleted entity."""

    def delete_many(self, entities: list[E], *, strict: bool = False) -> list[E]:
        """Delete multiple entities. Returns list of actually-deleted entities.
        Idempotent by default; use strict=True to raise on missing entities."""

    # Managers can be bound to an existing Transaction/TransactionContext
    # Person.manager(tx) reuses the provided transaction
```

### Sharing Transactions (Atomic Workflows)

Use a shared transaction when you need multiple operations to commit together:

```python
from typedb.driver import TransactionType
from type_bridge import Database, Person, Artifact

db = Database(address="localhost:1730", database="mydb")

with db.transaction(TransactionType.WRITE) as tx:
    person_mgr = Person.manager(tx)     # reuses tx
    artifact_mgr = Artifact.manager(tx) # same tx

    alice = person_mgr.get(name=Name("Alice"))[0]
    alice.age = Age(alice.age.value + 1)
    person_mgr.update(alice)

    artifact_mgr.insert(Artifact(display_id=DisplayId(f"AL-{alice.age.value}")))
# commit happens automatically on context exit; rollback on exception
```

Notes:

- `Database.transaction` returns a context manager; pass the returned context or its `transaction` to managers/queries.
- Entity/Relation managers and queries automatically reuse the provided transaction instead of opening new ones.
- READ transactions are never rolled back (no writes); WRITE/SCHEMA auto-commit on success and rollback on exception.

## Insert Operations

### Single Insert

Insert one entity at a time:

```python
# Create entity instance
alice = Person(
    name=Name("Alice Johnson"),
    age=Age(30),
    email=Email("alice@example.com")
)

# Insert into database
person_manager.insert(alice)
```

### Bulk Insert

Insert multiple entities efficiently in a single transaction:

```python
# Create multiple entities
persons = [
    Person(name=Name("Alice"), age=Age(30)),
    Person(name=Name("Bob"), age=Age(25)),
    Person(name=Name("Charlie"), age=Age(35)),
    Person(name=Name("Diana"), age=Age(28)),
]

# Bulk insert (more efficient than multiple insert() calls)
person_manager.insert_many(persons)
```

**Performance tip**: Use `insert_many()` for multiple entities - it's significantly faster than calling `insert()` multiple times.

Both `insert()` and `insert_many()` run in a single write transaction when a transaction/context is provided to the manager. Without one, each call opens exactly one write transaction (no per-entity commits).

**Note on special characters**: TypeBridge automatically escapes special characters in string attributes (quotes, backslashes) when generating TypeQL queries. You don't need to manually escape values - just pass them as normal Python strings.

## PUT Operations (Idempotent Insert)

PUT operations are idempotent - they insert only if the pattern doesn't exist, making them safe to run multiple times.

| Operation  | Behavior                                   |
| ---------- | ------------------------------------------ |
| **INSERT** | Always creates new instances               |
| **PUT**    | Idempotent - inserts only if doesn't exist |

```python
# Single PUT
alice = Person(name=Name("Alice"), age=Age(30))
person_manager.put(alice)
person_manager.put(alice)  # No duplicate created

# Bulk PUT
persons = [Person(name=Name("Bob"), age=Age(25)), ...]
person_manager.put_many(persons)
person_manager.put_many(persons)  # No duplicates
```

**Use cases**: Data import scripts, ensuring reference data exists, synchronization with external systems.

**All-or-nothing semantics**: PUT matches the entire pattern - if ANY part doesn't match, ALL is inserted. Use `put_many()` when entities either all exist or all don't exist together.

Both `put()` and `put_many()` reuse a provided transaction/context; otherwise each call wraps a single write transaction (no per-entity commits inside a bulk call).

## Read Operations

### Get All Entities

```python
# Fetch all persons
all_persons = person_manager.all()

for person in all_persons:
    print(f"{person.name}: {person.age}")
```

### Get with Filters

Filter by attribute values:

```python
# Get persons with specific age
young_persons = person_manager.get(age=25)

# Get person by name (key attribute)
alice = person_manager.get(name="Alice")

# Multiple filters (AND logic)
results = person_manager.get(age=30, status="active")
```

### Chainable Queries

Create complex queries with method chaining:

```python
# Basic query
query = person_manager.filter(age=30)
results = query.execute()

# Chained query with pagination
results = person_manager.filter(age=30).limit(10).offset(5).execute()

# Get first matching entity (returns Person | None)
first_person = person_manager.filter(name="Alice").first()

if first_person:
    print(f"Found: {first_person.name}")
else:
    print("Not found")

# Count matching entities
count = person_manager.filter(age=30).count()
print(f"Found {count} persons aged 30")
```

### Django-style lookup suffixes

`filter()` also accepts Django-style suffix operators that translate into TypeQL expressions:

- `field__contains="sub"`
- `field__startswith="pre"`
- `field__endswith="suf"`
- `field__regex="^A.*"`
- `field__gt/gte/lt/lte=value`
- `field__in=[v1, v2, v3]` (non-empty iterable)
- `field__isnull=True|False`

Example:

```python
people = person_manager.filter(name__startswith="Al", age__gt=30).execute()
gmail = person_manager.filter(email__contains="@gmail.com").execute()
nullable = person_manager.filter(age__isnull=True).execute()
```

More examples (TypeQL mapping shown for clarity):

```python
# contains/startswith/endswith/regex
emails = person_manager.filter(email__contains="@acme.com")
# -> has email like ".*@acme\\.com.*"

prefixed = person_manager.filter(display_id__startswith="US-")
# -> has display_id like "^US\\-.*"

suffixed = person_manager.filter(name__endswith="son")
# -> has name like ".*son$"

regexed = person_manager.filter(city__regex="^New(\\s|-)York$")
# -> has city like "^New(\\s|-)York$"

# numeric comparisons
seniors = person_manager.filter(age__gte=65)
# -> has age >= 65

# disjunction via __in (folded into OR)
statuses = person_manager.filter(status__in=["active", "pending"])
# -> { has status "active"; } or { has status "pending"; }

# null checks (uses presence/absence of the attribute)
missing_age = person_manager.filter(age__isnull=True)
present_age = person_manager.filter(age__isnull=False)
```

Rules and validation:

- Attribute names cannot contain `__` when using lookups.
- `__in` requires a non-empty iterable; mixed raw values and Attribute instances are allowed.
- String lookups (`contains`, `startswith`, `endswith`, `regex`) require `String` attributes.
- `__isnull` requires a boolean.

### EntityQuery Methods

```python
class EntityQuery[E: Entity]:
    def filter(self, **filters) -> EntityQuery[E]:
        """Add additional filters."""

    def order_by(self, *fields: str) -> EntityQuery[E]:
        """Sort results by one or more fields. Prefix with '-' for descending."""

    def limit(self, n: int) -> EntityQuery[E]:
        """Limit number of results."""

    def offset(self, n: int) -> EntityQuery[E]:
        """Skip first n results."""

    def execute(self) -> list[E]:
        """Execute query and return results."""

    def first(self) -> E | None:
        """Get first result or None."""

    def count(self) -> int:
        """Count matching entities."""

    def delete(self) -> int:
        """Delete all matching entities. Returns count deleted."""

    def update_with(self, func: Callable[[E], None]) -> list[E]:
        """Update entities by applying function. Returns updated entities."""
```

### Sorting Results

Use `order_by()` to sort query results:

```python
# Ascending order (default)
results = person_manager.filter().order_by('age').execute()

# Descending order (prefix with '-')
results = person_manager.filter().order_by('-age').execute()

# Multiple sort fields (primary, then secondary)
results = person_manager.filter().order_by('city', '-age').execute()

# Combined with filter and pagination
results = (
    person_manager
    .filter(Person.city.eq(City("NYC")))
    .order_by('-age')
    .limit(10)
    .execute()
)
```

#### Role-Player Sorting (Relations Only)

For relations, you can sort by role-player attributes using `role__attr` syntax:

```python
# Sort by employee's age
results = employment_manager.filter().order_by('employee__age').execute()

# Descending by role-player attribute
results = employment_manager.filter().order_by('-employee__age').execute()

# Mixed: role-player and relation attributes
results = employment_manager.filter().order_by('employee__age', '-salary').execute()
```

> **Note**: Multi-value attributes (those with `Card(max=None)`) cannot be used for sorting.

## Update Operations

The update API supports two patterns:

1. **Instance-based**: fetch → modify → update (traditional ORM pattern)
2. **Bulk functional**: filter → update_with function (efficient bulk updates)

### Basic Update

```python
# Step 1: Fetch entity
alice = person_manager.get(name="Alice")[0]

# Step 2: Modify attributes
alice.age = Age(31)
alice.status = Status("active")

# Step 3: Persist changes
person_manager.update(alice)
```

### Important: @key Attributes Required for update()

The `update()` method uses `@key` attributes to identify which entity to update in the database. Your entity class must have at least one `@key` attribute defined:

```python
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)  # @key attribute - required for update()
    age: Age | None = None

# This works because Person has @key attribute "name"
alice = person_manager.get(name="Alice")[0]
alice.age = Age(31)
person_manager.update(alice)  # Uses "name" to match entity in database
```

**Without a @key attribute**, `update()` will raise `KeyAttributeError`:

```python
class Counter(Entity):
    flags = TypeFlags(name="counter")
    value: Value  # No @key attribute!

counter = counter_manager.get(value=42)[0]
counter.value = Value(100)
counter_manager.update(counter)  # Raises KeyAttributeError
```

**If a @key attribute value is None**, `update()` will also raise `KeyAttributeError`:

```python
alice = Person(name=None, age=Age(30))  # Invalid: @key is None
person_manager.update(alice)  # Raises KeyAttributeError
```

If your entity uses a UUID or ID field, make sure it's marked as `@key`:

```python
class Document(Entity):
    flags = TypeFlags(name="document")
    id: Id = Flag(Key)  # Mark as @key for update() to work
    title: Title
```

See [Exception Handling](#exception-handling) for details on handling `KeyAttributeError`.

### Update Single-Value Attributes

```python
# Fetch entity
bob = person_manager.get(name="Bob")[0]

# Modify single-value attributes
bob.age = Age(26)
bob.email = Email("bob.new@example.com")
bob.is_active = IsActive(True)

# Persist changes
person_manager.update(bob)
```

### Update Multi-Value Attributes

```python
# Fetch entity
alice = person_manager.get(name="Alice")[0]

# Replace all values (deletes old, inserts new)
alice.tags = [Tag("python"), Tag("typedb"), Tag("machine-learning")]

# Persist changes
person_manager.update(alice)

# Clear multi-value attribute
alice.tags = []
person_manager.update(alice)
```

### Update Multiple Attributes

```python
# Fetch entity
charlie = person_manager.get(name="Charlie")[0]

# Modify multiple attributes at once
charlie.age = Age(36)
charlie.status = Status("active")
charlie.tags = [Tag("java"), Tag("python"), Tag("kubernetes")]
charlie.is_verified = IsVerified(True)

# Single update call persists all changes
person_manager.update(charlie)
```

### Bulk Update with Function

Update multiple entities efficiently using `update_with()`:

```python
# Increment age for all persons over 30
updated = person_manager.filter(Age.gt(Age(30))).update_with(
    lambda person: setattr(person, 'age', Age(person.age.value + 1))
)
print(f"Updated {len(updated)} persons")

# Complex updates with function
def promote_to_senior(person):
    """Promote eligible persons to senior status."""
    person.status = Status("senior")
    if person.salary:
        # 10% raise
        person.salary = Salary(int(person.salary.value * 1.1))

# Apply to filtered entities
promoted = person_manager.filter(
    Age.gte(Age(35)),
    Status.eq(Status("regular"))
).update_with(promote_to_senior)

# All updates happen in single transaction
print(f"Promoted {len(promoted)} persons")
```

### Bulk Update with Entities (`update_many`)

`update_many()` updates multiple entity instances in one write transaction while preserving the same per-entity semantics as `update()`:

```python
people = [
    Person(name=Name("Alice"), age=Age(30)),
    Person(name=Name("Bob"), age=Age(40)),
]
person_manager.insert_many(people)

# Modify locally
people[0].age = Age(31)
people[1].age = Age(41)

# Persist in one transaction
person_manager.update_many(people)
```

`update_many()` reuses a provided transaction/context; otherwise it opens exactly one write transaction for the batch.

**How `update_with()` works**:

1. Fetches all entities matching the filter
2. Applies the function to each entity in-place
3. Updates all entities in a single atomic transaction
4. Returns list of updated entities

**Error handling**: If the function raises an error on any entity, the operation stops immediately and raises the error. No partial updates occur (atomic transaction).

**Empty results**: Returns empty list if no entities match the filter.

### TypeQL Update Semantics

The update method generates different TypeQL based on cardinality:

**Single-value attributes** (`@card(0..1)` or `@card(1..1)`):

- Uses TypeQL `update` clause for efficient in-place updates

**Multi-value attributes** (e.g., `@card(1..)`, `@card(2..5)`):

- Deletes all old values
- Inserts new values

**Example TypeQL generated**:

```typeql
match
$e isa person, has name "Alice";
delete
has $tags of $e;
insert
$e has tags "python";
$e has tags "typedb";
$e has tags "machine-learning";
update
$e has age 31;
$e has status "active";
```

## Delete Operations

TypeBridge supports two delete patterns:

1. **Instance delete**: `manager.delete(entity)` - delete by entity instance (recommended)
2. **Filter delete**: `manager.filter(...).delete()` - delete matching entities by filter

### Instance Delete (Recommended)

Delete entities by instance, similar to `update()`:

```python
# Step 1: Get entity instance
alice = person_manager.get(name="Alice")[0]

# Step 2: Delete using manager
deleted = person_manager.delete(alice)
print(f"Deleted: {deleted.name.value}")  # Returns the deleted entity

# OR use instance method directly
alice.delete(db)  # Returns alice for chaining
```

**How instance delete works**:

- Uses `@key` attributes to identify the entity (same as `update()`)
- Returns the deleted entity instance (not a count)
- Raises `ValueError` if key attribute is None
- Raises `EntityNotFoundError` if entity doesn't exist in database

### Delete Entities Without @key

For entities without `@key` attributes, delete matches by ALL attributes:

```python
class Counter(Entity):
    flags = TypeFlags(name="counter")
    value: Value  # No @key attribute

counter = Counter(value=Value(42))
manager.insert(counter)

# Works if exactly 1 match exists
manager.delete(counter)  # Matches by value=42

# Raises ValueError if multiple matches exist
manager.delete(counter)  # Error: found 2 matches
```

**Behavior**:

- Matches by ALL non-None attributes
- Only deletes if exactly 1 match found
- Raises `EntityNotFoundError` if 0 matches
- Raises `NotUniqueError` if >1 matches

### Batch Delete with `delete_many`

Delete multiple entity instances in a single transaction:

```python
# Get entities to delete
alice = person_manager.get(name="Alice")[0]
bob = person_manager.get(name="Bob")[0]

# Delete multiple entities
deleted = person_manager.delete_many([alice, bob])
print(f"Deleted {len(deleted)} entities")  # Returns list of deleted entities

# Empty list returns empty list
deleted = person_manager.delete_many([])
assert deleted == []
```

**Idempotent by default**: Missing entities are silently ignored:

```python
# Create entity that doesn't exist in DB
nonexistent = Person(name=Name("NonExistent"))

# Delete mix of existing and nonexistent - no error raised
deleted = person_manager.delete_many([alice, nonexistent])
print(f"Deleted {len(deleted)}")  # 1 (only alice was deleted)
assert alice in deleted
assert nonexistent not in deleted  # Not in result since it didn't exist
```

**Strict mode**: Use `strict=True` to raise an error if any entity doesn't exist:

```python
# Raises EntityNotFoundError if any entity is missing
deleted = person_manager.delete_many([alice, nonexistent], strict=True)
# EntityNotFoundError: Cannot delete: 1 entity(ies) not found...
```

### Filter-Based Delete

For bulk deletion by criteria, use `filter().delete()`:

```python
# Delete all persons over 65
count = person_manager.filter(Age.gt(Age(65))).delete()
print(f"Deleted {count} seniors")  # Returns count (int)

# Delete with multiple expression filters
count = person_manager.filter(
    Age.lt(Age(18)),
    Status.eq(Status("inactive"))
).delete()
print(f"Deleted {count} inactive minors")

# Delete by multiple values using __in filter
count = person_manager.filter(name__in=["Alice", "Bob", "Charlie"]).delete()

# Delete with range filter
count = person_manager.filter(
    Age.gte(Age(18)),
    Age.lt(Age(21))
).delete()

# Returns 0 if no matches
count = person_manager.filter(Age.gt(Age(150))).delete()
assert count == 0
```

**How filter delete works**:

- Builds TypeQL delete query from all filters
- Executes in single atomic transaction
- Returns count of deleted entities (int)

### Instance Delete Method

Entities can delete themselves:

```python
# Create and insert entity
alice = Person(name=Name("Alice"), age=Age(30))
alice.insert(db)

# Delete using instance method
alice.delete(db)  # Returns alice for chaining

# Chaining example
Person(name=Name("Temp")).insert(db).delete(db)
```

**Warning**: Delete operations are permanent and cannot be undone!

### Exception Handling

Delete and update operations raise specific exceptions for better error handling:

```python
from type_bridge import EntityNotFoundError, KeyAttributeError, NotUniqueError

# Handle non-existent entity
try:
    manager.delete(nonexistent_entity)
except EntityNotFoundError:
    print("Entity was already deleted or never existed")

# Handle multiple matches for keyless entity
try:
    manager.delete(keyless_entity)
except NotUniqueError:
    # Use filter().delete() for bulk deletion instead
    count = manager.filter(value=keyless_entity.value).delete()

# Handle @key validation failures
try:
    manager.update(entity_with_none_key)
except KeyAttributeError as e:
    print(f"Cannot {e.operation} {e.entity_type}: key '{e.field_name}' is None")
    # e.entity_type: "Person"
    # e.operation: "update" or "delete"
    # e.field_name: "name" (if key is None)
    # e.all_fields: ["title", "desc"] (if no @key defined)
```

**Exception hierarchy**:

- `EntityNotFoundError(LookupError)` - Entity not found in database
- `NotUniqueError(ValueError)` - Multiple matches for keyless entity
- `KeyAttributeError(ValueError)` - @key attribute is None or no @key defined

## RelationManager

Type-safe manager for relation CRUD operations.

### Creating a Relation Manager

```python
from type_bridge import Relation, TypeFlags, Role

class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position
    salary: Salary

# Create manager
employment_manager = Employment.manager(db)
```

### RelationManager Methods

```python
class RelationManager[R: Relation]:
    def insert(self, relation: R) -> R:
        """Insert a single relation."""

    def insert_many(self, relations: list[R]) -> list[R]:
        """Insert multiple relations (bulk operation)."""

    def put(self, relation: R) -> R:
        """Put a single relation (idempotent insert)."""

    def put_many(self, relations: list[R]) -> list[R]:
        """Put multiple relations (idempotent bulk operation)."""

    def get(self, **filters) -> list[R]:
        """Get relations matching attribute/role player filters."""

    def filter(self, **filters) -> RelationQuery[R]:
        """Create chainable query with filters."""

    def group_by(self, *fields) -> RelationGroupByQuery[R]:
        """Create group-by query for aggregations."""

    def all(self) -> list[R]:
        """Get all relations of this type."""

    def delete(self, relation: R) -> R:
        """Delete relation by instance. Returns deleted relation."""

    def delete_many(self, relations: list[R]) -> list[R]:
        """Delete multiple relations. Returns list of deleted relations."""

    def update(self, relation: R) -> R:
        """Update relation in database."""

# Relation class method (not manager)
class Relation:
    @classmethod
    def get_roles(cls) -> dict[str, Role]:
        """Get all roles defined on this relation."""
```

### Accessing Relation Roles

Use `get_roles()` to introspect relation roles:

```python
# Get all roles for a relation
roles = Employment.get_roles()
# Returns: {'employee': Role(...), 'employer': Role(...)}

# Access specific role
employee_role = Employment.get_roles()['employee']
print(employee_role.role_name)  # 'employee'
print(employee_role.player_entity_types)  # (Person,)
```

### Role Player Matching

When performing relation CRUD operations (`insert`, `put`, `update`, `delete`), TypeBridge needs to identify the role player entities in the database. It uses an **IID-preferring** matching strategy:

1. **IID (preferred)**: If the entity has `_iid` set (from being fetched from the database), uses fast IID-based matching
2. **Key attributes (fallback)**: If no IID, uses the entity's `@key` attributes to identify it
3. **Error**: If neither is available, raises `ValueError` with clear guidance

```python
# Pattern 1: Fetch entities first (uses IID matching - faster, more precise)
alice = person_manager.filter(name=Name("Alice")).first()  # alice._iid is set
company = company_manager.filter(name=Name("TechCorp")).first()  # company._iid is set
emp = Employment(employee=alice, employer=company, position=Position("Engineer"))
emp_manager.insert(emp)  # Uses IIDs for matching

# Pattern 2: Create stub entities (uses key attribute matching)
alice = Person(name=Name("Alice"))  # No _iid - just the key attribute
company = Company(name=Name("TechCorp"))
emp = Employment(employee=alice, employer=company, position=Position("Engineer"))
emp_manager.insert(emp)  # Uses name (@key) for matching

# Pattern 3: Entity without IID or @key - raises error
# class NoKeyEntity(Entity):
#     flags = TypeFlags(name="no_key")
#     value: Value  # No @key attribute
# no_key = NoKeyEntity(value=Value(42))
# emp = SomeRelation(player=no_key)
# relation_manager.insert(emp)  # ValueError: cannot identify role player
```

**Best practice**: Fetch entities from the database when you need to use them as role players. This populates `_iid` and enables faster, more precise matching.

### Insert Relations

```python
# Single insert
employment = Employment(
    employee=alice,
    employer=techcorp,
    position=Position("Senior Engineer"),
    salary=Salary(120000)
)
employment_manager.insert(employment)

# Bulk insert
employments = [
    Employment(employee=alice, employer=techcorp, position=Position("Engineer"), salary=Salary(100000)),
    Employment(employee=bob, employer=startup, position=Position("Designer"), salary=Salary(90000)),
    Employment(employee=charlie, employer=techcorp, position=Position("Manager"), salary=Salary(130000)),
]
employment_manager.insert_many(employments)
```

### PUT Relations (Idempotent Insert)

PUT operations for relations work the same as entities - idempotent and safe to run multiple times:

```python
# Single PUT
employment = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
employment_manager.put(employment)
employment_manager.put(employment)  # No duplicate

# Bulk PUT
employments = [Employment(employee=alice, employer=techcorp, ...), ...]
employment_manager.put_many(employments)
employment_manager.put_many(employments)  # No duplicates
```

### Fetch Relations

#### Get All Relations

```python
# Fetch all employments
all_employments = employment_manager.all()

for employment in all_employments:
    print(f"{employment.employee.name}: {employment.position}")
```

#### Get Relations with Filters

Filter by both attributes and role players:

```python
# Filter by attribute
engineers = employment_manager.get(position="Engineer")

# Filter by role player
alice_jobs = employment_manager.get(employee=alice)
techcorp_employees = employment_manager.get(employer=techcorp)

# Multiple filters (AND logic)
results = employment_manager.get(
    employee=alice,
    position="Senior Engineer"
)

# Filter by both role players
specific_employment = employment_manager.get(
    employee=alice,
    employer=techcorp
)
```

#### Chainable Relation Queries

RelationManager now supports the same chainable query API as EntityManager:

```python
# Basic query
query = employment_manager.filter(position="Engineer")
results = query.execute()

# Chained query with pagination
results = employment_manager.filter(position="Engineer").limit(10).offset(5).execute()

# Get first matching relation (returns Relation | None)
first_employment = employment_manager.filter(employee=alice).first()

if first_employment:
    print(f"Found: {first_employment.position}")
else:
    print("Not found")

# Count matching relations
count = employment_manager.filter(position="Engineer").count()
print(f"Found {count} engineers")
```

#### RelationQuery Methods

Complete API parity with EntityQuery.

Type-safe role player expressions and `**kwargs` support in chained `filter()`.

```python
class RelationQuery[R: Relation]:
    def filter(self, *expressions, **filters) -> RelationQuery[R]:
        """Add filters. Supports type-safe expressions and Django-style kwargs."""

    def order_by(self, *fields: str) -> RelationQuery[R]:
        """Sort results by fields. Use 'role__attr' for role-player attributes."""

    def limit(self, n: int) -> RelationQuery[R]:
        """Limit number of results."""

    def offset(self, n: int) -> RelationQuery[R]:
        """Skip first n results."""

    def execute(self) -> list[R]:
        """Execute query and return results."""

    def first(self) -> R | None:
        """Get first result or None."""

    def count(self) -> int:
        """Count matching relations."""

    def delete(self) -> int:
        """Delete all matching relations. Returns count deleted."""

    def update_with(self, func: Callable[[R], None]) -> list[R]:
        """Update relations by applying function. Returns updated relations."""

    def aggregate(self, *aggregates) -> dict[str, Any]:
        """Execute aggregation queries."""

    def group_by(self, *fields) -> RelationGroupByQuery[R]:
        """Group relations by field values."""
```

#### Type-Safe Role Player Expressions

Filter relations using type-safe role player field access:

```python
# Type-safe role player expressions
results = manager.filter(
    Employment.employee.age.gte(Age(30))
).execute()

# String operations on role player attributes
results = manager.filter(
    Employment.employer.name.contains(Name("Tech"))
).execute()

# Combine with Django-style filters
results = manager.filter(
    Employment.employee.age.gt(Age(25)),
    employer__industry__eq="Technology"
).execute()

# Full query with sorting and pagination
results = (
    manager.filter(Employment.employee.age.gte(Age(25)), salary__gte=80000)
    .order_by("employee__age", "-salary")
    .limit(10)
    .execute()
)
```

See [Queries - Type-Safe Role Player Expressions](queries.md#type-safe-role-player-expressions) for full documentation.

### Update Relations

The update API supports two patterns (same as EntityManager):

1. **Instance-based**: fetch → modify → update (traditional ORM pattern)
2. **Bulk functional**: filter → update_with function (efficient bulk updates)

#### Basic Relation Update

```python
# Step 1: Fetch relation
employment = employment_manager.get(employee=alice, employer=techcorp)[0]

# Step 2: Modify attributes
employment.position = Position("Staff Engineer")
employment.salary = Salary(150000)

# Step 3: Persist changes
employment_manager.update(employment)
```

#### Update Relation Single-Value Attributes

```python
# Fetch relation
employment = employment_manager.get(employee=alice)[0]

# Modify attributes
employment.position = Position("Principal Engineer")
employment.salary = Salary(180000)
employment.start_date = StartDate("2024-01-01")

# Persist changes
employment_manager.update(employment)
```

#### Update Relation Multi-Value Attributes

```python
# Fetch relation
employment = employment_manager.get(employee=alice)[0]

# Replace all values (deletes old, inserts new)
employment.responsibilities = [
    Responsibility("Team lead"),
    Responsibility("Architecture"),
    Responsibility("Mentoring")
]

# Persist changes
employment_manager.update(employment)

# Clear multi-value attribute
employment.responsibilities = []
employment_manager.update(employment)
```

#### Bulk Relation Update with Function

Update multiple relations efficiently using `update_with()`:

```python
# Give all engineers a 10% raise
updated = employment_manager.filter(position="Engineer").update_with(
    lambda emp: setattr(emp, 'salary', Salary(int(emp.salary.value * 1.1)))
)
print(f"Updated {len(updated)} engineers")

# Complex updates with function
def promote_to_senior(employment):
    """Promote engineers to senior level."""
    # Add "Senior" prefix
    employment.position = Position(f"Senior {employment.position.value}")
    # 20% raise
    if employment.salary:
        employment.salary = Salary(int(employment.salary.value * 1.2))

# Apply to filtered relations
promoted = employment_manager.filter(
    position="Engineer",
    employee=alice  # Only Alice's employments
).update_with(promote_to_senior)

# All updates happen in single transaction
print(f"Promoted {len(promoted)} employments")
```

**How `update_with()` works for relations**:

1. Fetches all relations matching the filter
2. Stores original attribute values (needed to uniquely identify relations)
3. Applies the function to each relation in-place
4. Updates all relations in a single atomic transaction using original values for matching
5. Returns list of updated relations

**Why original values matter**: In TypeDB, multiple relations can have the same role players (e.g., Alice can have multiple employments at TechCorp). The update query matches each relation by both its role players AND its original attribute values to ensure the correct relation is updated.

**Error handling**: If the function raises an error on any relation, the operation stops immediately and raises the error. No partial updates occur (atomic transaction).

**Empty results**: Returns empty list if no relations match the filter.

### Delete Relations

Delete API refactored to instance-based pattern.

Raises `RelationNotFoundError` when relation doesn't exist.

TypeBridge supports two delete patterns for relations:

1. **Instance delete**: `manager.delete(relation)` - delete by relation instance (recommended)
2. **Filter delete**: `manager.filter(...).delete()` - delete matching relations by filter

#### Relation Instance Delete (Recommended)

Delete relations by instance, using role players' `@key` attributes:

```python
# Get or create relation instance
employment = employment_manager.get(employee=alice, employer=techcorp)[0]

# Delete using manager
deleted = employment_manager.delete(employment)
print(f"Deleted: {deleted.position.value}")  # Returns deleted relation

# OR use instance method directly
employment.delete(db)  # Returns employment for chaining
```

**How instance delete works**:

- Uses role players' `@key` attributes to identify the relation
- Returns the deleted relation instance (not a count)
- Raises `ValueError` if role player is missing or has None key
- Raises `RelationNotFoundError` if relation doesn't exist

#### Relation Batch Delete with `delete_many`

Delete multiple relation instances in a single transaction:

```python
# Get relations to delete
emp1 = employment_manager.get(employee=alice)[0]
emp2 = employment_manager.get(employee=bob)[0]

# Delete multiple relations
deleted = employment_manager.delete_many([emp1, emp2])
print(f"Deleted {len(deleted)} relations")  # Returns list of deleted relations

# Empty list returns empty list
deleted = employment_manager.delete_many([])
assert deleted == []
```

#### Relation Filter-Based Delete

For bulk deletion by criteria, use `filter().delete()`:

```python
# Delete high-salary employments
count = employment_manager.filter(Salary.gt(Salary(150000))).delete()
print(f"Deleted {count} high-salary employments")  # Returns count (int)

# Delete with multiple expression filters
count = employment_manager.filter(
    Salary.lt(Salary(50000)),
    Position.eq(Position("Intern"))
).delete()
print(f"Deleted {count} low-paid interns")

# Delete by role player using filter
count = employment_manager.filter(employee=alice).delete()
print(f"Deleted all of Alice's employments: {count}")

# Returns 0 if no matches
count = employment_manager.filter(Salary.gt(Salary(1000000))).delete()
assert count == 0
```

**How filter delete works**:

- Builds TypeQL delete query from all filters
- Executes in single atomic transaction
- Returns count of deleted relations (int)

#### Relation Instance Delete Method

Relations can delete themselves:

```python
# Create and insert relation
emp = Employment(employee=alice, employer=techcorp, position=Position("Engineer"))
emp.insert(db)

# Delete using instance method
emp.delete(db)  # Returns emp for chaining
```

**Warning**: Delete operations are permanent and cannot be undone!

#### Relation Exception Handling

Relation delete operations raise `RelationNotFoundError` when the relation doesn't exist:

```python
from type_bridge import RelationNotFoundError

try:
    manager.delete(nonexistent_relation)
except RelationNotFoundError:
    print("Relation was already deleted or never existed")
```

## Type Safety

Managers use Python's generic type syntax to preserve type information:

```python
class EntityManager[E: Entity]:
    def insert(self, entity: E) -> E: ...
    def get(self, **filters) -> list[E]: ...
    def all(self) -> list[E]: ...
```

Type checkers understand the returned types:

```python
# ✅ Type-safe: alice is inferred as Person
alice = Person(name=Name("Alice"), age=Age(30))
person_manager.insert(alice)

# ✅ Type-safe: persons is inferred as list[Person]
persons: list[Person] = person_manager.all()

# ✅ Type-safe: first_person is inferred as Person | None
first_person = person_manager.filter(age=30).first()

# ❌ Type error: Cannot insert Company into Person manager
company = Company(name=Name("TechCorp"))
person_manager.insert(company)  # Type checker catches this!
```

## Complete CRUD Workflow

```python
from type_bridge import (
    Database, Entity, TypeFlags,
    String, Integer, Boolean,
    Flag, Key, Unique, Card
)

# 1. Define schema
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

class Tag(String):
    pass

class User(Entity):
    flags = TypeFlags(name="user")
    user_id: UserID = Flag(Key)
    username: Username
    email: Email = Flag(Unique)
    age: Age | None = None
    is_active: IsActive | None = None
    tags: list[Tag] = Flag(Card(min=0))

# 2. Connect to database
db = Database(address="localhost:1729", database="mydb")
db.connect()

# 3. Create manager
user_manager = User.manager(db)

# 4. CREATE: Insert users
users = [
    User(
        user_id=UserID("u1"),
        username=Username("alice"),
        email=Email("alice@example.com"),
        age=Age(30),
        is_active=IsActive(True),
        tags=[Tag("python"), Tag("typedb")]
    ),
    User(
        user_id=UserID("u2"),
        username=Username("bob"),
        email=Email("bob@example.com"),
        age=Age(25),
        is_active=IsActive(True),
        tags=[Tag("javascript"), Tag("react")]
    ),
]
user_manager.insert_many(users)

# 5. READ: Fetch users
all_users = user_manager.all()
alice = user_manager.get(username="alice")[0]
active_users = user_manager.filter(is_active=True).execute()

# 6. UPDATE: Modify user
alice = user_manager.get(username="alice")[0]
alice.age = Age(31)
alice.tags = [Tag("python"), Tag("typedb"), Tag("fastapi")]
user_manager.update(alice)

# 7. DELETE: Remove user
bob = user_manager.get(username="bob")[0]
deleted = user_manager.delete(bob)
print(f"Deleted user: {deleted.username.value}")

# OR use instance method
# bob.delete(db)
```

## Best Practices

### 1. Use Bulk Insert for Multiple Entities

```python
# ✅ GOOD: Bulk insert (single transaction)
user_manager.insert_many(users)

# ❌ POOR: Multiple inserts (multiple transactions)
for user in users:
    user_manager.insert(user)
```

### 2. Use `first()` for Single Results

```python
# ✅ GOOD: Use first() for single result
user = user_manager.filter(username="alice").first()
if user:
    print(user.email)

# ❌ POOR: Use get() and index
users = user_manager.get(username="alice")
if users:
    print(users[0].email)
```

### 3. Fetch Before Update

Always fetch the current entity before updating:

```python
# ✅ GOOD: Fetch → Modify → Update
alice = user_manager.get(username="alice")[0]
alice.age = Age(31)
user_manager.update(alice)

# ❌ WRONG: Cannot update without fetching first
alice = User(username=Username("alice"), age=Age(31))
user_manager.update(alice)  # Error: entity not from database
```

### 4. Use Specific Filters

Use key or unique attributes for efficient queries:

```python
# ✅ GOOD: Filter by key or unique attribute
alice = user_manager.get(user_id="u1")[0]
alice = user_manager.get(email="alice@example.com")[0]

# ⚠️ SLOWER: Filter by non-indexed attribute
alice = user_manager.get(age=30)[0]  # May return multiple results
```

### 5. Use Instance Delete Pattern

Delete entities by instance, not by filter:

```python
# ✅ GOOD: Instance-based delete
alice = user_manager.get(user_id="u1")[0]
deleted = user_manager.delete(alice)  # Returns alice
print(f"Deleted {deleted.username.value}")

# ✅ GOOD: Instance method
alice.delete(db)

# ✅ GOOD: Filter-based for bulk operations
count = user_manager.filter(Age.gt(Age(65))).delete()  # Returns count

# Use filter().delete() for filter-based deletion
# Use filter().delete() instead for filter-based deletion
```

## Database Configuration

### Basic Connection

```python
from type_bridge import Database

# Default connection
db = Database()  # localhost:1729, database="typedb"

# Custom connection
db = Database(
    address="192.168.1.100:1729",
    database="mydb",
    username="admin",
    password="secret"
)
db.connect()

# Context manager (auto-connects and closes)
with Database(database="mydb") as db:
    person_manager = Person.manager(db)
    # ... operations ...
```

### Driver Injection

For advanced use cases, you can inject an external `Driver` instance instead of having `Database` create one internally. This enables:

- **Connection sharing** across multiple `Database` instances
- **Resource pooling** with custom driver management
- **Easier testing** via mock driver injection

```python
from typedb.driver import TypeDB, Credentials, DriverOptions

# Create a shared driver
driver = TypeDB.driver(
    "localhost:1729",
    Credentials("admin", "password"),
    DriverOptions()
)

# Multiple databases share one connection
db1 = Database(database="project_a", driver=driver)
db2 = Database(database="project_b", driver=driver)

# Use both databases
with db1.transaction("write") as tx:
    Person.manager(tx).insert(alice)

with db2.transaction("read") as tx:
    results = Artifact.manager(tx).all()

# Close databases (only clears references, doesn't close driver)
db1.close()
db2.close()

# Close driver when done (caller's responsibility)
driver.close()
```

**Ownership semantics:**

- `driver=None` (default): `Database` creates and owns the driver, `close()` closes it
- `driver=<Driver>`: `Database` uses but doesn't own it, `close()` only clears the reference

### Testing with Mock Driver

```python
from unittest.mock import MagicMock

def test_database_operations():
    mock_driver = MagicMock()
    mock_driver.databases.contains.return_value = True

    db = Database(database="test_db", driver=mock_driver)

    assert db.database_exists() is True
    mock_driver.databases.contains.assert_called_with("test_db")
```

## See Also

- [Entities](entities.md) - Entity definition
- [Relations](relations.md) - Relation definition
- [Queries](queries.md) - Advanced query expressions
- [Schema Management](schema.md) - Schema operations
