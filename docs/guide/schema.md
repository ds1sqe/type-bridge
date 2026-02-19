# Schema Management

Complete reference for schema operations, conflict detection, and migrations in TypeBridge.

## Overview

TypeBridge provides comprehensive schema management with automatic conflict detection to prevent accidental data loss during schema changes. The schema system ensures safe evolution of your database schema over time.

## SchemaManager

The `SchemaManager` class handles schema registration, generation, and synchronization:

```python
from type_bridge import SchemaManager, Database

# Connect to database
db = Database(address="localhost:1729", database="mydb")
db.connect()

# Create schema manager
schema_manager = SchemaManager(db)
```

### SchemaManager Methods

```python
class SchemaManager:
    def register(self, *models: type[Entity] | type[Relation]) -> None:
        """Register entity or relation types."""

    def generate_schema(self) -> str:
        """Generate TypeQL schema from registered models."""

    def sync_schema(self, force: bool = False) -> None:
        """Sync schema to database. Use force=True to recreate (⚠️ DATA LOSS)."""

    def collect_schema_info(self) -> SchemaInfo:
        """Collect current schema information."""

    def compare_schemas(self, old: SchemaInfo, new: SchemaInfo) -> SchemaDiff:
        """Compare two schemas and return differences."""
```

## Basic Schema Operations

### Register Models

Register entity and relation types before generating schema:

```python
from type_bridge import Entity, Relation, TypeFlags, Role

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None

class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)

class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position

# Register all models
schema_manager.register(Person, Company, Employment)
```

### Generate TypeQL Schema

Generate TypeQL schema from registered models:

```python
# Generate schema string
typeql_schema = schema_manager.generate_schema()

print(typeql_schema)
# Output:
# define
#
# attribute name, value string;
# attribute age, value integer;
# attribute position, value string;
#
# entity person,
#     owns name @key,
#     owns age @card(0..1);
#
# entity company,
#     owns name @key;
#
# relation employment,
#     relates employee,
#     relates employer,
#     owns position @card(1..1);
#
# person plays employment:employee;
# company plays employment:employer;
```

### Sync Schema to Database

Synchronize generated schema to TypeDB:

```python
# First time - creates schema
schema_manager.sync_schema()  # ✅ Success

# Subsequent calls - validates compatibility
schema_manager.sync_schema()  # ✅ Success if compatible
```

## Conflict Detection

SchemaManager automatically detects breaking changes and prevents data loss:

### Detecting Conflicts

```python
from type_bridge.schema import SchemaConflictError

# Initial schema creation
schema_manager.register(Person, Company, Employment)
schema_manager.sync_schema()  # ✅ Creates schema

# Modify your models (remove an attribute - BREAKING CHANGE)
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    # age attribute removed!

# Re-register and attempt to sync
schema_manager = SchemaManager(db)
schema_manager.register(Person, Company, Employment)

try:
    schema_manager.sync_schema()  # ❌ Raises SchemaConflictError
except SchemaConflictError as e:
    print("Schema conflict detected!")
    print(e.diff.summary())
    # Output:
    # Schema Differences:
    # Modified Entities:
    #   person:
    #     - Removed attributes: age
```

### Force Recreate (DATA LOSS)

Use `force=True` to drop and recreate the database:

```python
# ⚠️ WARNING: This drops the entire database and recreates it
# ALL DATA WILL BE LOST
schema_manager.sync_schema(force=True)
```

**Use force mode only when:**
- You're in development and don't need existing data
- You've backed up your data
- You're intentionally resetting the database

## Schema Comparison

Compare schemas to understand changes before applying them:

### Collect Schema Information

```python
from type_bridge.schema import SchemaInfo

# Collect current schema
old_schema = schema_manager.collect_schema_info()

# Modify your models
class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None
    email: Email = Flag(Unique)  # ✅ New attribute added!

# Collect new schema
schema_manager = SchemaManager(db)
schema_manager.register(Person)
new_schema = schema_manager.collect_schema_info()

# Compare
diff = old_schema.compare(new_schema)
```

### View Schema Differences

```python
# Check if there are changes
if diff.has_changes():
    print(diff.summary())
    # Output:
    # Schema Differences:
    # Modified Entities:
    #   person:
    #     + Added attributes: email (unique)

# Check for breaking changes
if diff.has_breaking_changes():
    print("WARNING: Breaking changes detected!")
else:
    print("Safe to apply changes")
```

## Schema Diff Details

The `SchemaDiff` class provides granular change tracking:

### Top-Level Changes

```python
# Added/removed types
print(f"Added entities: {diff.added_entities}")
print(f"Removed entities: {diff.removed_entities}")
print(f"Added relations: {diff.added_relations}")
print(f"Removed relations: {diff.removed_relations}")
print(f"Added attributes: {diff.added_attributes}")
print(f"Removed attributes: {diff.removed_attributes}")
```

### Entity Changes

```python
# Check entity modifications
for entity_type, changes in diff.modified_entities.items():
    print(f"\n{entity_type} changes:")
    print(f"  Added attributes: {changes.added_attributes}")
    print(f"  Removed attributes: {changes.removed_attributes}")

    # Check attribute flag changes (cardinality, key, unique)
    for attr_name, flag_change in changes.modified_attributes.items():
        print(f"  Modified {attr_name}:")
        print(f"    Old flags: {flag_change.old_flags}")
        print(f"    New flags: {flag_change.new_flags}")
```

### Relation Changes

```python
# Check relation modifications
for relation_type, changes in diff.modified_relations.items():
    print(f"\n{relation_type} changes:")
    print(f"  Added roles: {changes.added_roles}")
    print(f"  Removed roles: {changes.removed_roles}")
    print(f"  Added attributes: {changes.added_attributes}")
    print(f"  Removed attributes: {changes.removed_attributes}")

    # Check role changes
    for role_name, role_change in changes.modified_roles.items():
        print(f"  Modified role {role_name}:")
        print(f"    Old type: {role_change.old_type}")
        print(f"    New type: {role_change.new_type}")
```

## SchemaDiff Structure

```python
@dataclass
class SchemaDiff:
    # Top-level changes
    added_entities: set[str]
    removed_entities: set[str]
    added_relations: set[str]
    removed_relations: set[str]
    added_attributes: set[str]
    removed_attributes: set[str]

    # Detailed changes
    modified_entities: dict[str, EntityChanges]
    modified_relations: dict[str, RelationChanges]

    def has_changes(self) -> bool:
        """Check if there are any changes."""

    def has_breaking_changes(self) -> bool:
        """Check if there are breaking changes (removals, type changes)."""

    def summary(self) -> str:
        """Get human-readable summary of changes."""

@dataclass
class EntityChanges:
    added_attributes: set[str]
    removed_attributes: set[str]
    modified_attributes: dict[str, FlagChange]

@dataclass
class RelationChanges:
    added_roles: set[str]
    removed_roles: set[str]
    added_attributes: set[str]
    removed_attributes: set[str]
    modified_roles: dict[str, RoleChange]
    modified_attributes: dict[str, FlagChange]
```

## Schema Validation Rules

TypeBridge enforces validation rules during schema generation to prevent TypeDB errors:

### Duplicate Attribute Type Detection

Each semantic field must use a distinct attribute type:

```python
from type_bridge.schema import SchemaValidationError

# ❌ WRONG: Duplicate attribute types
class TimeStamp(DateTime):
    pass

class Issue(Entity):
    key: IssueKey = Flag(Key)
    created: TimeStamp   # Error: duplicate attribute type
    modified: TimeStamp  # TypeDB sees only one ownership

# Raises SchemaValidationError during generate_schema():
# "TimeStamp used in fields: 'created', 'modified'"

# ✅ CORRECT: Distinct attribute types
class CreatedStamp(DateTime):
    pass

class ModifiedStamp(DateTime):
    pass

class Issue(Entity):
    key: IssueKey = Flag(Key)
    created: CreatedStamp   # ✅ Distinct type
    modified: ModifiedStamp # ✅ Distinct type
```

**Why this matters**: TypeDB stores attribute types, not field names. Using the same type for multiple fields causes cardinality conflicts because TypeDB sees a single ownership relationship.

### Reserved Word Validation

TypeBridge validates that type names don't use TypeDB/TypeQL reserved words:

```python
# ❌ WRONG: Using reserved words
class Type(Entity):  # Error: 'type' is reserved
    pass

class Match(Entity):  # Error: 'match' is reserved
    pass

# ✅ CORRECT: Use different names
class ContentType(Entity):
    pass

class MatchResult(Entity):
    pass
```

## Migration Manager

For complex schema migrations, use `MigrationManager`:

```python
from type_bridge.schema import MigrationManager

migration_manager = MigrationManager(db)
```

### Add Migrations

```python
# Add individual migration
migration_manager.add_migration(
    name="add_email_to_person",
    schema="define person owns email;"
)

# Add complex migration
migration_manager.add_migration(
    name="add_company_entity",
    schema="""
    define
    entity company,
        owns name @key,
        owns industry;

    attribute industry, value string;
    """
)

# Add migration with data transformation
migration_manager.add_migration(
    name="split_name_field",
    schema="""
    define
    person owns first-name,
        owns last-name;

    attribute first-name, value string;
    attribute last-name, value string;
    """,
    data_script="""
    # TypeQL to transform data
    match
    $p isa person, has name $n;
    # ... split logic
    """
)
```

### Apply Migrations

```python
# Apply all pending migrations in order
migration_manager.apply_migrations()

# Apply specific migration
migration_manager.apply_migration("add_email_to_person")

# Check migration status
status = migration_manager.get_migration_status()
print(f"Applied: {status.applied}")
print(f"Pending: {status.pending}")
```

## Complete Schema Workflow

```python
from type_bridge import (
    Database, SchemaManager,
    Entity, Relation, TypeFlags, Role,
    String, Integer, Date,
    Flag, Key, Unique, Card
)
from type_bridge.schema import SchemaConflictError

# 1. Define models
class UserID(String):
    pass

class Username(String):
    pass

class Email(String):
    pass

class User(Entity):
    flags = TypeFlags(name="user")
    user_id: UserID = Flag(Key)
    username: Username
    email: Email = Flag(Unique)

# 2. Connect to database
db = Database(address="localhost:1729", database="mydb")
db.connect()

# 3. Create schema manager
schema_manager = SchemaManager(db)

# 4. Register models
schema_manager.register(User)

# 5. Generate and view schema
typeql = schema_manager.generate_schema()
print("Generated schema:")
print(typeql)

# 6. Sync to database
try:
    schema_manager.sync_schema()
    print("✅ Schema synchronized successfully")
except SchemaConflictError as e:
    print("❌ Schema conflict detected:")
    print(e.diff.summary())
    print("\nUse force=True to recreate (⚠️ DATA LOSS)")

# 7. Later: Modify models
class User(Entity):
    flags = TypeFlags(name="user")
    user_id: UserID = Flag(Key)
    username: Username
    email: Email = Flag(Unique)
    created_at: CreatedAt  # ✅ New attribute - safe change

# 8. Compare schemas
old_schema = schema_manager.collect_schema_info()
schema_manager = SchemaManager(db)
schema_manager.register(User)
new_schema = schema_manager.collect_schema_info()

diff = old_schema.compare(new_schema)
if diff.has_changes():
    print("Schema changes detected:")
    print(diff.summary())

    if diff.has_breaking_changes():
        print("⚠️ Breaking changes - review carefully")
    else:
        print("✅ Safe to apply")

# 9. Apply changes
schema_manager.sync_schema()  # Applies safe changes
```

## Best Practices

### 1. Always Check for Conflicts

```python
# ✅ GOOD: Handle conflicts
try:
    schema_manager.sync_schema()
except SchemaConflictError as e:
    print(e.diff.summary())
    # Decide whether to force or modify schema

# ❌ POOR: Ignore conflicts
schema_manager.sync_schema(force=True)  # ⚠️ DATA LOSS
```

### 2. Use Distinct Attribute Types

```python
# ✅ GOOD: Each field has distinct type
class CreatedAt(DateTime):
    pass

class UpdatedAt(DateTime):
    pass

class Record(Entity):
    created_at: CreatedAt
    updated_at: UpdatedAt

# ❌ WRONG: Duplicate attribute types
class Timestamp(DateTime):
    pass

class Record(Entity):
    created_at: Timestamp  # Error!
    updated_at: Timestamp
```

### 3. Review Changes Before Applying

```python
# ✅ GOOD: Review changes
diff = old_schema.compare(new_schema)
print(diff.summary())

if diff.has_breaking_changes():
    confirm = input("Breaking changes detected. Continue? (yes/no): ")
    if confirm.lower() == "yes":
        schema_manager.sync_schema(force=True)
else:
    schema_manager.sync_schema()

# ❌ POOR: Blindly apply
schema_manager.sync_schema(force=True)
```

### 4. Use Migrations for Complex Changes

```python
# ✅ GOOD: Use migrations for multi-step changes
migration_manager.add_migration(
    name="refactor_user_fields",
    schema="""...""",
    data_script="""..."""
)
migration_manager.apply_migrations()

# ❌ POOR: Force recreate loses data
schema_manager.sync_schema(force=True)
```

### 5. Version Control Your Schemas

```python
# ✅ GOOD: Keep schema history
# models/v1/user.py
class User(Entity):
    user_id: UserID = Flag(Key)

# models/v2/user.py
class User(Entity):
    user_id: UserID = Flag(Key)
    email: Email = Flag(Unique)  # Added in v2
```

## See Also

- [Entities](entities.md) - Entity definition
- [Relations](relations.md) - Relation definition
- [Attributes](attributes.md) - Attribute types
- [Validation](validation.md) - Schema validation rules
