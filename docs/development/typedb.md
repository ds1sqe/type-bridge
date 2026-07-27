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

The current 1.x release line and every TypeBridge 2.0.x release support TypeDB
servers in the **3.8.x through 3.12.x** range. TypeBridge 2.0.x is the final
release line to support TypeDB 3.8.x and 3.10.x; those server lines are
deprecated in
`v2.0.0` to provide downstream users a migration window. Starting with
TypeBridge `v2.1.0`, the supported server window is **TypeDB 3.11.x through
3.12.x**. The
complete V2 schema/query feature set and its conformance fixtures target
**TypeDB 3.12.1** exactly, matching the exact release-artifact lane. Verified
compatibility tags include `3.8.3`, `3.10.4`, `3.11.5`, and `3.12.1`. There is
no published 3.9 line (TypeDB skipped it). TypeDB 3.7.x is protocol-compatible
with band 7 but falls below the declared floor and remains unsupported.

### Protocol bands

TypeDB uses a protocol-band model. A driver natively speaks exactly one band; a server
accepts a *set* of bands, and a connection succeeds only when the server accepts the
driver's band. Through 3.11 every server accepted exactly its own band, so cross-band
connections always failed. Starting with 3.12 acceptance is asymmetric (measured live):
server 3.12 retains backward compatibility with band-8 drivers, while a band-9 (3.12)
driver is refused by a 3.11 server at connect.

| Band | Driver versions | Servers accepting it |
|------|-----------------|----------------------|
| 7    | 3.7\*, 3.8, 3.10 | 3.7\*, 3.8, 3.10 (TypeBridge 2.x only) |
| 8    | 3.11            | 3.11, 3.12 |
| 9    | 3.12            | 3.12 |

\* 3.7 is protocol-compatible with band 7 but unsupported. The band and acceptance maps
are declared once in `crates/core`'s version module and consumed by every tier.

### Multi-band runtime

Throughout TypeBridge 2.0.x, the wheel embeds three TypeDB Rust driver lines:
band 7 (an unofficial namespaced packaging of upstream 3.8.1), band 8 (an
unofficial namespaced packaging of upstream 3.11.5), and band 9 (the official
upstream 3.12.1 package, currently the newest non-yanked stable 3.12.x Rust
driver release). The band-7 and band-8 Rust source is byte-identical to the
matching upstream archives; TypeBridge carries no transaction-close or other
behavioral driver patch. Their package and protocol names differ solely so
Cargo can keep all three protocol bands in one native dependency graph.
TypeBridge negotiates the connect band from the server's accepted-band set at
connect time.

Immediately before the first immutable release-graph package is published,
the release gate rechecks that band 9 is exact-pinned to the latest non-yanked
stable 3.12.x driver. Once that cutoff is crossed, a retry preserves the exact
graph already started even if a newer upstream patch is subsequently released.
A confirmed 3.12 server upgrades to band 9 so `given` rows are available; band 8
remains its safe discovery/fallback path. A single TypeBridge release therefore
serves the full supported window without user-side driver selection.
Starting with `v2.1.0`, the wheel embeds only bands 8 and 9: the band-7 line
and active TypeDB 3.8/3.10 support leave in that minor release, the scheduled
legacy-window exception to the ordinary major-version removal schedule.

TypeBridge explicitly closes a binding-owned embedded-driver connection before
releasing it. Automatic teardown is lease-aware: an open transaction may still
outlive the database handle that created it, and the final driver shutdown waits
for that transaction; an explicit `Database.close()` idempotently marks the
connection terminal and dispatches upstream shutdown. Once explicit close
begins, new operations fail on the closed connection, in-flight work is
cancelled rather than drained, and closing a retained transaction is treated as
terminal cleanup. The unmodified 3.12.1 driver releases its final callback
worker only when the last driver lease drops, so a retained closed handle does
not carry a synchronous worker-join guarantee. This also does not imply that
every binding can dispatch close concurrently with a synchronous call. A Node
synchronous native operation occupies the JavaScript event-loop thread, so
`RustDatabase.close()` cannot interrupt it out of band. Cancellable Node V2
execution uses the existing Promise-returning local surface and its deadline,
or the deadline bound into a prepared remote request. This is a temporary
response to the upstream 3.12.1 lifecycle
liveness defect tracked by [#196](https://github.com/ds1sqe/type-bridge/issues/196),
not a behavioral patch to any packaged driver or protocol. Remove the explicit
shutdown only after #196's upstream-release and packaged-artifact acceptance
criteria pass.

### Release support matrix

Compatibility is release-specific: every 2.0.x release retains the current
server window, and `v2.1.0` removes the deprecated band-7 runtime as the
scheduled legacy-window exception to the ordinary major-version boundary.

| Dimension | Supported range | Notes |
|-----------|-----------------|-------|
| TypeDB server, current 1.x | 3.8.0-3.12.x | Existing compatibility window |
| TypeDB server, `v2.x` | 3.8.0-3.12.x | Final major line supporting 3.8.x and 3.10.x; both are deprecated in `v2.0.0` |
| TypeDB server, `v3.x` | 3.11.x-3.12.x | Bands 8 and 9 only; 3.8.x and 3.10.x are rejected |
| V2 conformance baseline | TypeDB 3.12.1 | `v2.0.0` ships the complete planned V2 schema and advanced multi-type query specification against this exact baseline |
| Embedded Rust driver bands, `v2.x` | 7, 8, and 9 | Bands 7 and 8 are namespaced packaging-only, upstream-identical packages; native/default band 9 is official upstream `typedb-driver` 3.12.1, currently the latest stable 3.12.x release and exercised against the TypeDB 3.12.1 server baseline |
| Embedded Rust driver bands, `v3.x` | 8 and 9 | Band 7 is removed |
| Python `typedb-driver`, `v2.x` | 3.8–3.12 on CPython 3.12–3.13; 3.12.1 on CPython 3.14 | Final compatibility major line for direct 3.8/3.10 drivers; the embedded runtime remains separate |
| Python `typedb-driver`, `v3.x` | 3.11–3.12 on CPython 3.12–3.13; 3.12.1 on CPython 3.14 | Direct driver support narrows with the server window |
| CPython interpreter | 3.12–3.14 | Defaulted generic parameters use the compatible `typing_extensions` surface on 3.12; the abi3 native wheel supports all declared interpreter lines |
| Python native wheels | Linux x86_64/aarch64 GNU; macOS x86_64/arm64; Windows x86_64 | Core wheels use the CPython 3.12 stable ABI. The core sdist is a build fallback, not a promise that every unlisted native target is release-tested |
| Node.js runtime | 18 and newer | The declared floor is exercised on Linux x64; Node 20 accepts every published native target in the release workflow |
| Node native package | Linux x64/arm64 GNU; macOS x64/arm64; Windows x64/arm64 MSVC | These six binaries are packed into one npm artifact. No musl binary is advertised by this release line |

### Feature gates vs. the version window

The support window says which servers TypeBridge *connects to*; individual
TypeDB features can still require a newer server within that window. Feature
requirements are declared in `crates/core`'s version module (`Feature`) and
checked client-side against the server version detected at connect time, so
a feature used against a too-old server fails with a versioned TypeBridge
error naming both versions — never a server-side syntax error.

Current feature gates:

| Feature | Minimum server | Gated surfaces |
|---------|----------------|----------------|
| `@doc`/`@meta` schema annotations | 3.12.0 | `SchemaManager.sync_schema`, migration executor steps |
| `given`-stage parameterized queries | 3.12.0 | `Database.execute_with_rows`, `TransactionContext.execute_with_rows`; `insert_many` and typed-query predicate transport use it opportunistically |

When the server version is unknown (band-7 gRPC fallback without a
`server_version=` pin), gated DDL is sent as-is and the server decides;
given-stage queries are rejected by the runtime when the negotiated driver
band cannot carry input rows. Typed queries do not require the feature: they
automatically retain validated inline literal lowering on older bands and use
one bounded `given` row only when band-9 transport is active.

Throughout `v2.x`, the band map itself is `{7, 8, 9}` in the default build: band 9 is the
official upstream TypeDB 3.12 driver, and its protocol is the wire path for given
rows. One measured hazard shapes the connect design: a band-9 connection
attempt crashes a 3.11 server outright, so the gRPC fallback discovers
unknown servers through band 8 and upgrades to band 9 only after the
reported version proves it safe.
The `v3.x` default build uses `{8, 9}`.

### Update-safety contract

`Database.connect()` raises a human-readable, actionable error when its embedded
runtime connects to a server outside the supported window (e.g. 3.7.x), before
any transaction is attempted and never mid-operation. The optional installed
Python `typedb-driver` is not involved in this ORM path. When direct driver
access is requested through `Database.driver`, its separate gate validates the
installed driver against the server before opening that external connection.
Both errors name the relevant versions without exposing raw protocol numbers.

The direct Python driver follows its own protocol band. On CPython 3.12–3.13 the
development extra selects driver 3.11.5 for the direct-driver compatibility
lane. The isolated source-tree suite defaults to the TypeDB 3.12.1 server
baseline; the embedded Rust runtime selects official band 9 for that server.
On CPython 3.14 it selects driver 3.12.1, the current direct-driver patch with
a CPython 3.14 native wheel, and direct driver connections must therefore
target TypeDB 3.12.
This interpreter-specific restriction does not apply to `Database.connect()`,
typed queries, or other ORM operations backed by TypeBridge's embedded Rust
runtime.

By default, the gate calls `GET :<http_port>/v1/version` on the server's HTTP API port.
If that endpoint is unreachable and no exact server version was supplied,
TypeBridge falls back to gRPC protocol negotiation. Throughout `v2.x`, it tries
the band-8 driver first and then band 7; `v3.x` tries band 8 only. If all
permitted gRPC attempts fail, the gate fails loudly with every attempted path
in the error.

### HTTP version-probe port

TypeDB exposes a version endpoint over HTTP in addition to its gRPC port. TypeBridge
probes this endpoint at connect time to determine the server version before committing
to a driver construction. The probe port defaults to `8000` but must match the HTTP port
of the specific TypeDB instance being targeted unless you supply `server_version`.

On a host running multiple TypeDB instances (for example, a primary on `:8000` and a
test instance remapped to `:9000`), probing the wrong port silently validates the wrong
server — the gate passes against instance A while the gRPC connection goes to instance B.
Configuring the correct port prevents that mismatch.

**Python ORM**

```python
# Default port (8000) — no configuration needed for a standard single-instance setup
db = Database(address="localhost:1729", database="mydb")
db.connect()

# Explicit port — required when TypeDB's HTTP port is remapped
db = Database(address="localhost:1729", database="mydb", http_port=9000)
db.connect()
```

### gRPC-only deployments

If the TypeDB server exposes gRPC but disables or firewalls the HTTP API,
TypeBridge can still connect by falling back to gRPC protocol negotiation.
Throughout `v2.x`, the fallback tries band 8 before band 7; `v3.x` tries band
8 only. It reports every permitted failure if no driver can open a connection.

For strict exact-version validation on gRPC-only deployments, pass the exact server
version explicitly:

```python
db = Database(
    address="localhost:1729",
    database="mydb",
    server_version="3.10.4",
)
db.connect()
```

When `server_version` is set, TypeBridge skips the HTTP probe and gRPC fallback,
validates the supplied semantic version against the same support window, derives the
protocol band from the validated version, and then opens the matching embedded Rust
driver.

For `v2.0.x`, use an exact TypeDB version such as `3.8.3`, `3.10.4`, or
`3.11.5`, or `3.12.1`; do not substitute a raw protocol band. Band 7 includes
unsupported TypeDB `3.7.x` as well as supported `3.8.x` and `3.10.x`. When
HTTP is unavailable, automatic band-7 fallback can identify the protocol band
but not the exact semantic version; use `server_version` when that exact
validation is required. Starting with `v2.1.0`, pin only TypeDB 3.11.x or
3.12.x. Invalid or unsupported pinned versions still fail with `VersionError`.

**Node binding**

```typescript
import { RustDatabase, ensureDatabase } from "@type-bridge/node";

// Pass httpPort in the options object when TypeDB's HTTP port is remapped.
const db = RustDatabase.connect("localhost:1729", "mydb", { httpPort: 9000 });

// Pass serverVersion to skip HTTP probing for gRPC-only deployments.
ensureDatabase("localhost:1729", "mydb", { serverVersion: "3.10.4" });
const grpcOnly = RustDatabase.connect("localhost:1729", "mydb", {
  serverVersion: "3.10.4",
});
```

**Server config**

```toml
[typedb]
address = "localhost:1729"
database = "mydb"
http_port = 9000

# Optional: exact server version for gRPC-only deployments.
server_version = "3.10.4"
```

The same settings can be supplied with `TYPEDB_HTTP_PORT` and
`TYPEDB_SERVER_VERSION`.

**Migration CLI**

```bash
# The _generate command accepts --http-port for the same reason
python -m type_bridge._generate --address localhost:1729 --http-port 9000 ...
```

**Test suite environment variable**

Set `TYPEDB_HTTP_PORT` to override the HTTP probe port across all pytest integration tiers:

```bash
TYPEDB_HTTP_PORT=9000 ./test.sh --no-isolated
```

The variable is read by `tests/utils/typedb_lifecycle.py` as `TEST_DB_HTTP_PORT` and
forwarded to every fixture-level `Database` construction in `tests/integration/conftest.py`.
`test.sh` passes it inline to the Python integration, parity, and Node integration tiers.

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
- `@unique` does not imply cardinality; preserve the independently declared
  `@card` when a unique ownership is required or multi-valued
- Bare TypeQL `@unique` retains the server default `@card(0..1)`

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
from type_bridge import Credentials, TypeDB, create_driver_options

# ✅ With credentials (required)
driver = TypeDB.driver(
    "localhost:1729",
    Credentials("admin", "password"),
    create_driver_options(),
)

# Omitting Credentials is invalid in TypeDB 3.x.
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
- Install `type-bridge[typedb-driver]` and select the driver line matching the
  target TypeDB server. On CPython 3.14, use driver and server 3.12.
- Remove session management code
- Add credentials for authentication
- Use `transaction.query()` instead of separate query methods

### Example: Complete TypeDB 3.x Query

```python
from type_bridge import Credentials, TransactionType, TypeDB, create_driver_options

# Connect with credentials
driver = TypeDB.driver(
    "localhost:1729",
    Credentials("admin", "password"),
    create_driver_options(),
)

# Create/use database
if not driver.databases.contains("mydb"):
    driver.databases.create("mydb")

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
