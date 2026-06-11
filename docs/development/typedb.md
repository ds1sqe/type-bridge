# TypeDB Integration Guide

This guide covers TypeDB-specific concepts, driver API, TypeQL syntax, and integration considerations for TypeBridge.

## Table of Contents

- [Server and Driver Compatibility](#server-and-driver-compatibility)
- [Key TypeDB Concepts](#key-typedb-concepts)
- [TypeDB ORM Design Considerations](#typedb-orm-design-considerations)
- [TypeQL Syntax Requirements](#typeql-syntax-requirements)
- [TypeDB Driver 3.x API](#typedb-driver-3x-api)
- [TypeDB 3.x Syntax and Behavior Changes](#typedb-3x-syntax-and-behavior-changes)

## Server and Driver Compatibility

### Support window

TypeBridge supports TypeDB servers in the **3.8.x through 3.11.x** range. There is no
published 3.9 line (TypeDB skipped it); verified available tags are `3.8.3`, `3.10.4`,
and `3.11.5`. Server `3.7.x` is protocol-compatible with band 7 (see below) but falls
below the declared floor and is not supported.

### Protocol bands

TypeDB uses a protocol-band model. Compatibility is measured between driver and server
within a band; cross-band connections always fail at connect time.

| Band | Server / driver versions | Intra-band interop |
|------|--------------------------|--------------------|
| 7    | 3.7\*, 3.8, 3.10         | Full (3.7 is below the support floor) |
| 8    | 3.11                     | Full |

\* 3.7 is protocol-compatible with band 7 but unsupported. The band map is declared once
in `crates/core`'s version module and consumed by every tier.

### Dual-band runtime

TypeBridge's wheel embeds both TypeDB Rust driver lines — band-7 (3.8.1, vendored fork)
and band-8 (3.11.5, upstream) — and dispatches to the correct driver at connect time based
on the server's protocol band. A single type-bridge release therefore serves the full
supported window without any user-side configuration.

**This release line** serves TypeDB 3.8 through 3.11:

| Dimension | Supported range | Notes |
|-----------|-----------------|-------|
| TypeDB server | 3.8.0–3.11.x | Band-7 (3.8.x, 3.10.x) and band-8 (3.11.x); dispatch is automatic |
| Python `typedb-driver` | `~=3.11` (e.g. 3.11.5) | The `dev` extra pins `~=3.11.5`; the `typedb-driver` extra allows `>=3.8,<3.12`. This is the installed Python driver used for direct driver APIs and tests — it does not control which TypeDB server you can connect to via the ORM |
| CPython interpreter | 3.12–3.13 | Floor: PEP 695 syntax in the codebase; ceiling tracks the highest CPython the typedb-driver publishes wheels for |

### Update-safety contract

`Database.connect()` raises a human-readable, actionable error when connecting to a server
outside the supported window (e.g. 3.7.x) or when the installed Python `typedb-driver`
mismatches the server's band — before any transaction is attempted, never mid-operation.
The error names the relevant versions and tells you exactly what to do. It never exposes
raw protocol numbers.

The probe calls `GET :8000/v1/version` on the server's HTTP API port. If that endpoint is
unreachable the probe fails closed (connection refused, not a silent pass).

## Key TypeDB Concepts

When implementing features, keep these TypeDB-specific concepts in mind:

### 1. TypeQL Schema Definition Language

TypeDB requires schema definitions before data insertion. The schema defines:
- **Attribute types**: Value types (string, integer, double, etc.)
- **Entity types**: Independent objects that own attributes
- **Relation types**: Connections with explicit role players

### 2. Role Players

Relations in TypeDB are first-class citizens with explicit role players (not just foreign keys).

**Example:**

```typeql
relation employment,
    relates employee,
    relates employer;

person plays employment:employee;
company plays employment:employer;
```

This is fundamentally different from relational databases where foreign keys create implicit relationships.

### 3. Attribute Ownership

Attributes can be owned by multiple entity/relation types. This enables powerful data modeling:

```typeql
attribute name, value string;

entity person,
    owns name;

entity company,
    owns name;
```

Both `person` and `company` can own the same `name` attribute type.

### 4. Inheritance

TypeDB supports type hierarchies for entities, relations, and attributes:

```typeql
entity animal @abstract,
    owns name;

entity dog sub animal,
    owns breed;

entity cat sub animal,
    owns color;
```

Subtypes inherit all attributes and roles from their parent types.

### 5. Rule-based Inference

TypeDB can derive facts using rules. This is important for query design:

```typeql
rule transitive-location:
    when {
        (located: $x, location: $y) isa locating;
        (located: $y, location: $z) isa locating;
    } then {
        (located: $x, location: $z) isa locating;
    };
```

Rules allow queries to match both explicit and inferred data.

## TypeDB ORM Design Considerations

When implementing ORM features for TypeDB:

### 1. Mapping Challenge

TypeDB's type system is richer than traditional ORMs:
- Relations are not simple foreign keys
- Attributes are independent types, not columns
- Role players create explicit, typed connections

**TypeBridge approach:**
- Model attributes as Python classes (subclasses of `Attribute`)
- Model entities/relations as Python classes with `TypeFlags`
- Use `Role[T]` for type-safe role player definitions

### 2. TypeQL Generation

The ORM needs to generate valid TypeQL queries from Python API calls.

**Example: Insert query generation**

```python
# Python API
person = Person(name=Name("Alice"), age=Age(30))
manager.insert(person)

# Generated TypeQL
insert $e isa person,
    has name "Alice",
    has age 30;
```

**Example: Relation insert with role players**

```python
# Python API
employment = Employment(
    employee=alice,
    employer=techcorp,
    position=Position("Engineer")
)
manager.insert(employment)

# Generated TypeQL
match
$employee isa person, has name "Alice";
$employer isa company, has name "TechCorp";
insert
(employee: $employee, employer: $employer) isa employment,
    has position "Engineer";
```

### 3. Transaction Semantics

TypeDB has strict transaction types that must be respected:

- **READ**: For read-only queries (match, fetch)
- **WRITE**: For data modification (insert, delete, update)
- **SCHEMA**: For schema definition (define, undefine)

TypeBridge automatically selects the correct transaction type based on the operation.

### 4. Schema Evolution

Consider how Python model changes map to TypeDB schema updates:

**Adding a field:**
```python
# Before
class Person(Entity):
    name: Name = Flag(Key)

# After (add email)
class Person(Entity):
    name: Name = Flag(Key)
    email: Email  # New field
```

TypeBridge detects this as an **additive change** (safe).

**Removing a field:**
```python
# Before
class Person(Entity):
    name: Name = Flag(Key)
    age: Age

# After (remove age)
class Person(Entity):
    name: Name = Flag(Key)
```

TypeBridge detects this as a **breaking change** and raises `SchemaConflictError` (prevents data loss).

### 5. Role Handling

Relations require explicit role mapping:

```python
class Employment(Relation):
    flags = TypeFlags(name="employment")

    # Explicit role definitions with types
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)

    # Attributes
    position: Position
```

This generates:

```typeql
relation employment,
    relates employee,
    relates employer,
    owns position;

person plays employment:employee;
company plays employment:employer;
```

## TypeQL Syntax Requirements

When generating TypeQL schema definitions, always use the following correct syntax:

### 1. Attribute Definitions

```typeql
# ✅ CORRECT
attribute name, value string;

# ❌ WRONG
name sub attribute, value string;
```

### 2. Entity Definitions

```typeql
# ✅ CORRECT
entity person,
    owns name @key,
    owns age @card(0..1);

# ❌ WRONG
person sub entity,
    owns name @key;
```

### 3. Entity Inheritance with Abstract

```typeql
# ✅ CORRECT: Abstract entity without parent
entity content @abstract,
    owns id @key;

# ✅ CORRECT: Abstract entity with inheritance
entity page @abstract, sub content,
    owns page-id,
    owns bio;

# ✅ CORRECT: Concrete entity with inheritance
entity person sub page,
    owns email;
```

**Note**: `@abstract` comes before `sub`, separated by comma.

### 4. Relation Definitions

```typeql
# ✅ CORRECT
relation employment,
    relates employee,
    relates employer,
    owns salary @card(0..1);

# ❌ WRONG
employment sub relation,
    relates employee;
```

### 5. Relation Inheritance with Abstract

```typeql
# ✅ CORRECT: Abstract relation
relation social-relation @abstract,
    relates related @card(2);

# ✅ CORRECT: Concrete relation with inheritance
relation friendship sub social-relation,
    relates friend as related @card(2);
```

### 6. Cardinality Annotations

```typeql
# ✅ CORRECT: Use .. (double dot) syntax
@card(1..5)
@card(2..)     # Unbounded max
@card(0..1)

# ❌ WRONG: Comma syntax
@card(1,5)
```

### 7. Key and Unique Annotations

- `@key` implies `@card(1..1)`, never output both
- `@unique` with default `@card(1..1)`, omit `@card` annotation
- Only output explicit `@card` when it differs from the implied cardinality

```typeql
# ✅ CORRECT
entity person,
    owns email @key;              # Implies @card(1..1)

# ❌ WRONG (redundant)
entity person,
    owns email @key @card(1..1);  # Don't specify both
```

## TypeDB Driver 3.x API

The driver API for 3.x differs from earlier versions:

### 1. No Separate Sessions

Transactions are created directly on the driver:

```python
# ✅ TypeDB 3.x
driver.transaction(database_name, TransactionType.READ)

# ❌ Old API (TypeDB 2.x)
session = driver.session(database_name, SessionType.DATA)
transaction = session.transaction(TransactionType.READ)
```

### 2. Single Query Method

`transaction.query(query_string)` returns `Promise[QueryAnswer]`:

```python
# Execute query
promise = transaction.query("match $x isa person; fetch $x;")

# Must call .resolve() to get results
result = promise.resolve()
```

This works for all query types:
- `define` (schema definition)
- `insert` (data insertion)
- `match` (data querying)
- `fetch` (data fetching)
- `delete` (data deletion)
- `update` (data modification)

### 3. TransactionType Enum

Three transaction types:
- `TransactionType.READ`: Read-only queries
- `TransactionType.WRITE`: Data modification
- `TransactionType.SCHEMA`: Schema definition

```python
from typedb.driver import TransactionType

# Schema transaction
tx = driver.transaction(db_name, TransactionType.SCHEMA)
tx.query("define entity person, owns name;").resolve()
tx.commit()

# Write transaction
tx = driver.transaction(db_name, TransactionType.WRITE)
tx.query('insert $x isa person, has name "Alice";').resolve()
tx.commit()

# Read transaction
tx = driver.transaction(db_name, TransactionType.READ)
result = tx.query("match $x isa person; fetch $x;").resolve()
tx.close()  # No commit needed for READ
```

### 4. Authentication

Requires `Credentials(username, password)` even for local development:

```python
from typedb.driver import TypeDB, Credentials

# ✅ With credentials (required)
driver = TypeDB.core_driver(
    address="localhost:1729",
    credentials=Credentials("admin", "password")
)

# ❌ Without credentials (will fail in 3.x)
driver = TypeDB.core_driver(address="localhost:1729")
```

## TypeDB 3.x Syntax and Behavior Changes

TypeDB 3.x introduced important syntax and behavior changes that affect query generation:

### Query Syntax Changes

#### 1. Type Queries Use `isa` Instead of `sub`

```typeql
# ✅ TypeDB 3.x (correct)
match $x isa person;

# ❌ TypeDB 2.x (deprecated)
match $x sub person;
```

**TypeBridge implementation:**
- All generated queries use `isa` for type matching
- `sub` is only used in schema definitions for inheritance

#### 2. Cannot Query Root Types Directly

Cannot match on `entity`, `relation`, or `attribute` root types:

```typeql
# ❌ This will fail in TypeDB 3.x
match $x isa entity;

# ✅ Query specific entity types
match $x isa person;
```

**TypeBridge implementation:**
- Never generates queries for root types
- Always queries specific entity/relation types

#### 3. Pagination Requires Explicit Sorting

`offset` relies on consistent sort order:

```typeql
# ✅ CORRECT: Explicit sorting for pagination
match $p isa person;
sort $p asc;
offset 10;
limit 5;

# ⚠️ UNPREDICTABLE: No sort order
match $p isa person;
offset 10;
limit 5;
```

**TypeBridge implementation:**
- Always includes `sort` clause when using `offset`
- Default sort order: ascending by entity variable

#### 4. Clause Ordering Matters

`offset` must come before `limit`:

```typeql
# ✅ CORRECT order
match $p isa person;
sort $p asc;
offset 10;
limit 5;

# ❌ WRONG order (syntax error)
match $p isa person;
limit 5;
offset 10;
```

**TypeBridge implementation:**
- Query builder enforces correct clause order
- Clause order: `match` → `sort` → `offset` → `limit`

### Implementation Considerations

When generating TypeQL queries:

1. **Use `isa` for type matching** in all queries
2. **Avoid querying root types** (`entity`, `relation`, `attribute`)
3. **Always include explicit `sort` clause** when using `offset` for pagination
4. **Ensure clause order**: `match` → `sort` → `offset` → `limit`

### Migration from TypeDB 2.x

If migrating from TypeDB 2.x:

**Schema changes:**
- No changes needed (schema syntax is compatible)

**Query changes:**
- Replace `$x sub person` with `$x isa person`
- Add `sort` clause when using `offset`
- Ensure correct clause ordering

**Driver changes:**
- Update `typedb-driver` to the 3.11 line (`~=3.11`)
- Remove session management code
- Add credentials for authentication
- Use `transaction.query()` instead of separate query methods

### Example: Complete TypeDB 3.x Query

```python
from typedb.driver import TypeDB, TransactionType, Credentials

# Connect with credentials
driver = TypeDB.core_driver(
    address="localhost:1729",
    credentials=Credentials("admin", "password")
)

# Create/use database
if not driver.databases().contains("mydb"):
    driver.databases().create("mydb")

# Query with proper syntax
tx = driver.transaction("mydb", TransactionType.READ)

# TypeDB 3.x query: isa, sort, offset, limit
query = """
match
$p isa person, has name $name;
sort $name asc;
offset 10;
limit 5;
fetch
$p: name;
"""

result = tx.query(query).resolve()
tx.close()
```

### TypeDB 3.x Resources

- [TypeDB 3.x Documentation](https://typedb.com/docs)
- [TypeDB 3.x Release Notes](https://github.com/typedb/typedb/releases)
- [TypeDB Python Driver 3.x](https://github.com/typedb/typedb-driver-python)

---

For abstract types and interface hierarchies, see [abstract-types.md](abstract-types.md).

For internal implementation details, see [internals.md](internals.md).

For API reference, see the [User Guide](../guide/index.md).
