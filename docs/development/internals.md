# Internal Architecture Guide

This guide covers TypeBridge's internal type system, architecture decisions, and implementation details.

## Table of Contents

- [Internal Type System](#internal-type-system)
- [Modern Python Type Hints](#modern-python-type-hints)
- [Type Checking and Static Analysis](#type-checking-and-static-analysis)
- [Keyword-Only Arguments](#keyword-only-arguments)
- [Modular Architecture](#modular-architecture)
- [Connection Architecture](#connection-architecture)
- [Descriptor Contract](#descriptor-contract)
- [Deprecated APIs](#deprecated-apis)

## Internal Type System

### ModelAttrInfo Dataclass

The codebase uses `ModelAttrInfo` (defined in `models/utils.py`) as a structured type for attribute metadata:

```python
@dataclass
class ModelAttrInfo:
    typ: type[Attribute]  # The attribute class (e.g., Name, Age)
    flags: AttributeFlags  # Metadata (Key, Unique, Card)
```

**IMPORTANT**: Always use dataclass attribute access, never dictionary-style access:

```python
# ✅ CORRECT
owned_attrs = Entity.get_owned_attributes()
for field_name, attr_info in owned_attrs.items():
    attr_class = attr_info.typ
    flags = attr_info.flags

# ❌ WRONG - Never use dict-style access
attr_class = attr_info["type"]   # Will fail!
flags = attr_info["flags"]       # Will fail!
```

### AttributeFlags

The `AttributeFlags` dataclass stores attribute metadata:

```python
@dataclass
class AttributeFlags:
    is_key: bool = False
    is_unique: bool = False
    card_min: int | None = None
    card_max: int | None = None
    has_explicit_card: bool = False
    name: str | None = None  # Override attribute type name
    case: TypeNameCase | None = None  # Case formatting for type name
```

**Usage in code:**

```python
# Check if attribute is a key
if attr_info.flags.is_key:
    # Handle key attribute

# Get cardinality
if attr_info.flags.card_min is not None or attr_info.flags.card_max is not None:
    # Handle cardinality constraints

# Override attribute type name
class Name(String):
    flags = AttributeFlags(name="person_name")

# Use case formatting
class UserEmail(String):
    flags = AttributeFlags(case=TypeNameCase.SNAKE_CASE)  # -> user_email
```

### TypeFlags

The `TypeFlags` dataclass stores entity/relation metadata:

```python
@dataclass
class TypeFlags:
    type_name: str | None = None
    abstract: bool = False
    case: str = "snake_case"  # Or "kebab-case", "camelCase", etc.
```

**Usage patterns:**

```python
# Define entity with TypeFlags
class Person(Entity):
    flags = TypeFlags(name="person")

# Define abstract entity
class Animal(Entity):
    flags = TypeFlags(abstract=True)

# Custom type name casing
class MyEntity(Entity):
    flags = TypeFlags(name="my-entity", case="kebab-case")
```

### Attribute Metadata Collection

TypeBridge automatically collects attribute metadata during class definition:

```python
class Entity:
    def __init_subclass__(cls):
        """Automatically collects TypeFlags and owned attributes from type annotations."""
        # 1. Collect TypeFlags
        cls._flags = getattr(cls, "flags", TypeFlags())

        # 2. Collect owned attributes from annotations
        cls._owned_attrs = {}
        for field_name, field_type in get_type_hints(cls).items():
            if is_attribute_type(field_type):
                # Extract attribute class and flags
                attr_class, flags = extract_attribute_info(field_type, field_name, cls)
                cls._owned_attrs[field_name] = ModelAttrInfo(typ=attr_class, flags=flags)
```

This enables automatic schema generation without explicit configuration.

## Modern Python Type Hints

The project follows modern Python typing standards (Python 3.12+):

### 1. PEP 604: Union Type Syntax

Use `X | Y` instead of `Union[X, Y]`:

```python
# ✅ Modern (Python 3.10+)
age: int | str | None

# ❌ Deprecated
from typing import Union, Optional
age: Optional[Union[int, str]]
```

**Application in TypeBridge:**

```python
# Optional fields
class Person(Entity):
    name: Name = Flag(Key)
    age: Age | None = None  # PEP 604 syntax
```

### 2. PEP 695: Type Parameter Syntax

Use type parameter syntax for generics:

```python
# ✅ Modern (Python 3.12+)
class EntityManager[E: Entity]:
    def __init__(self, entity_class: type[E]):
        self.entity_class = entity_class

    def insert(self, entity: E) -> E:
        ...

# ❌ Old style (still works but verbose)
from typing import Generic, TypeVar
E = TypeVar("E", bound=Entity)
class EntityManager(Generic[E]):
    def __init__(self, entity_class: type[E]):
        self.entity_class = entity_class

    def insert(self, entity: E) -> E:
        ...
```

**Benefits:**

- Cleaner syntax
- Better IDE support
- Matches modern Python standards

### 3. No Linter Suppressions

Code should pass `ruff` and `pyright` without needing `# noqa` or `# type: ignore` comments:

```python
# ✅ CORRECT: No suppressions needed
def process_entity(entity: Entity) -> str:
    return entity.get_type_name()

# ❌ WRONG: Avoid suppressions
def process_entity(entity):  # type: ignore
    return entity.get_type_name()
```

**Exception:** Tests intentionally checking validation failures may show type warnings. These tests are in `tests/unit/type-check-except/` and excluded from type checking via `pyrightconfig.json`.

## Type Checking and Static Analysis

### @dataclass_transform Decorators

TypeBridge uses PEP-681 `@dataclass_transform` decorators on Entity and Relation classes to improve type checker support:

```python
from typing import dataclass_transform

@dataclass_transform(kw_only_default=True)
class Entity(BaseModel):
    """Base class for all entities."""
    ...
```

**Benefits:**

1. **Type checker recognition** of `Flag()` as a valid field default
2. **Automatic `__init__` signature inference** from class annotations
3. **Better IDE autocomplete** and type hints
4. **Keyword-only arguments enforced** (improved code clarity and safety)

### Type Checker Support

TypeBridge is fully compatible with:

- **Pyright**: Microsoft's static type checker (used in VS Code)
- **MyPy**: Optional, but TypeBridge is MyPy-compatible
- **Pydantic's type system**: Built on Pydantic v2

**Current status:**

- ✅ 0 type errors with Pyright
- ✅ 0 type warnings (except in type-check-except tests)
- ✅ Full type inference for managers and queries

### Type Checking Limitations

TypeBridge achieves 0 type errors with Pyright, but there are some edge cases:

#### 1. Optional Fields in Queries

When using field references with optional fields, Pyright may incorrectly infer the type:

```python
class Person(Entity):
    score: PersonScore | None = None  # Optional field

# Pyright may warn about optional field access
high_scorers = manager.filter(Person.score.gt(PersonScore(90)))  # May show warning
```

**Solution**: Use attribute class methods instead of field references for optional fields:

```python
# ✅ RECOMMENDED: Attribute class method (no warnings)
high_scorers = manager.filter(PersonScore.gt(PersonScore(90)))

# Also works, but may trigger type checker warnings
high_scorers = manager.filter(Person.score.gt(PersonScore(90)))
```

#### 2. Validation Tests

Tests that intentionally check Pydantic validation behavior use raw values and are excluded from type checking via `pyrightconfig.json`:

```json
{
  "exclude": ["tests/unit/type-check-except/**"]
}
```

These tests verify that runtime validation works correctly, even when type checkers would flag the code.

### Minimal `Any` Usage

The project minimizes `Any` usage for type safety:

**Where `Any` is used:**

1. **`Flag()` function**: Accepts `Any` for parameters to handle type aliases like `Key` and `Unique`

   ```python
   def Flag(*args: Any) -> AttributeFlags:
       """Create attribute flags from Key, Unique, Card arguments."""
       ...
   ```

2. **`Flag()` return type**: Returns `AttributeFlags` (used as field default)

   ```python
   class Person(Entity):
       name: Name = Flag(Key)  # Flag() returns AttributeFlags
   ```

3. **Pydantic core schema methods**: Use proper TypeVars (`StrValue`, `IntValue`, etc.)

   ```python
   @classmethod
   def __get_pydantic_core_schema__(
       cls, source_type: Any, handler: GetCoreSchemaHandler
   ) -> CoreSchema:
       ...
   ```

**Where `Any` is NOT used:**

- ✅ No other `Any` types in the core attribute system
- ✅ All managers are fully typed with generics
- ✅ All queries preserve type information
- ✅ All entity/relation operations are type-safe

## Keyword-Only Arguments

TypeBridge enforces keyword-only arguments for Entity and Relation constructors using `@dataclass_transform(kw_only_default=True)`.

### Why Keyword-Only?

1. **Clarity**: Explicit field names make code self-documenting
2. **Safety**: Type checkers catch argument order mistakes
3. **Maintainability**: Adding fields doesn't break existing code
4. **Prevention**: Eliminates entire class of positional argument bugs

### Usage Pattern

```python
from type_bridge import Entity, TypeFlags, String, Integer, Flag, Key

class Name(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None  # Optional field requires explicit = None

# ✅ CORRECT: Keyword arguments required
person = Person(name=Name("Alice"), age=Age(30))
person2 = Person(name=Name("Bob"))  # age is optional

# ❌ WRONG: Positional arguments not allowed
person = Person(Name("Alice"), Age(30))  # Type error!
```

### Optional Fields Require Explicit Defaults

Optional fields (marked with `| None`) **must** have an explicit `= None` default:

```python
# ✅ CORRECT: Explicit defaults for optional fields
class Person(Entity):
    name: Name = Flag(Key)          # Required field
    age: Age | None = None           # Optional with explicit = None
    email: Email | None = None       # Optional with explicit = None

# ❌ WRONG: Missing defaults on optional fields
class Person(Entity):
    name: Name = Flag(Key)
    age: Age | None                  # Type error: missing default!
    email: Email | None              # Type error: missing default!
```

**Why explicit `= None`?**

1. **Type checking**: Pyright needs explicit defaults to distinguish optional from required fields
2. **IDE support**: Autocomplete works better with explicit optionality
3. **Code clarity**: Makes intent obvious at a glance
4. **Runtime behavior**: Matches static type annotations exactly

### Implementation Details

The keyword-only enforcement is implemented via `@dataclass_transform`:

```python
@dataclass_transform(
    kw_only_default=True,
    field_specifiers=(Flag,)
)
class Entity(BaseModel):
    """Base class for all entities."""
    ...
```

This tells type checkers:

- All fields are keyword-only by default
- `Flag()` is recognized as a valid field specifier
- Constructor signature is inferred from class annotations

## Modular Architecture

The codebase follows a modular architecture pattern to improve maintainability and reduce file sizes:

### Models Module Structure

The `models/` module (previously a single 1500+ line file) is organized as:

```python
models/
├── __init__.py    # Public exports
├── base.py        # Base model functionality
├── entity.py      # Entity class
├── relation.py    # Relation class
├── role.py        # Role definitions
└── utils.py       # ModelAttrInfo and utilities
```

### CRUD Module Structure

The `crud/` module (previously a single 3000+ line file) is organized as:

```python
crud/
├── __init__.py       # Backward compatible exports
├── base.py           # Type variables (E, R)
├── utils.py          # Shared utilities
├── entity/           # Entity operations
│   ├── manager.py    # EntityManager
│   ├── query.py      # EntityQuery
│   └── group_by.py   # GroupByQuery
└── relation/         # Relation operations
    ├── manager.py    # RelationManager
    ├── query.py      # RelationQuery
    └── group_by.py   # RelationGroupByQuery
```

### Design Principles

1. **Single Responsibility**: Each module has a focused purpose
2. **Shared Utilities**: Common functions in `utils.py` to avoid duplication
3. **Backward Compatibility**: Top-level `__init__.py` maintains all public exports
4. **Clear Boundaries**: Entity and Relation operations are clearly separated
5. **Manageable Size**: Files are kept between 200-800 lines for maintainability

### Import Patterns

```python
# Public API
from type_bridge import TypeDBManager

# Or from crud module
from type_bridge.crud import TypeDBManager

# Shared utilities (internal use)
from type_bridge.crud.utils import format_value, is_multi_value_attribute
```

## Connection Architecture

TypeBridge provides a unified connection handling system for flexible transaction management.

### Connection Type

The `Connection` type alias allows managers to accept any connection type:

```python
from type_bridge.session import Connection, Database, Transaction, TransactionContext

# Type alias for flexible connection handling
Connection = Database | Transaction | TransactionContext

# Managers accept any Connection type
person_manager = Person.manager(db)         # Database
person_manager = Person.manager(tx)         # Transaction
person_manager = Person.manager(tx_ctx)     # TransactionContext
```

### TransactionContext

`TransactionContext` enables sharing transactions across multiple operations:

```python
from typedb.driver import TransactionType

# Create a shared transaction context
with db.transaction(TransactionType.WRITE) as tx:
    person_mgr = Person.manager(tx)     # reuses tx
    artifact_mgr = Artifact.manager(tx)  # same tx

    person_mgr.insert(alice)
    artifact_mgr.insert(artifact)
    # Both commit together on context exit
```

**Behavior:**

- Auto-commit on successful context exit (WRITE/SCHEMA transactions)
- Auto-rollback on exception
- READ transactions never commit (no writes)

### ConnectionExecutor

The internal `ConnectionExecutor` class handles transaction delegation:

```python
class ConnectionExecutor:
    """Unified query execution across connection types."""

    def __init__(self, connection: Connection):
        # Extracts database/transaction from connection

    def execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        # Uses existing transaction or creates new one

    @property
    def has_transaction(self) -> bool:
        # True if using an existing transaction

    @property
    def database(self) -> Database | None:
        # Returns database if available (for creating new transactions)
```

**Design principles:**

1. **Transparency**: CRUD operations work identically regardless of connection type
2. **Transaction reuse**: Existing transactions are never duplicated
3. **Auto-management**: Database connections create transactions as needed
4. **Atomic operations**: Bulk operations use single transactions

### Usage Patterns

```python
# Pattern 1: Simple operations (auto-managed transactions)
db = Database(address="localhost:1729", database="mydb")
Person.manager(db).insert(alice)  # Opens and commits its own transaction

# Pattern 2: Shared transaction (atomic multi-operation)
with db.transaction(TransactionType.WRITE) as tx:
    Person.manager(tx).insert(alice)
    Company.manager(tx).insert(techcorp)
    Employment.manager(tx).insert(employment)
    # All commit together

# Pattern 3: Bulk operations (single transaction internally)
Person.manager(db).insert_many(people)  # One transaction for all
Person.manager(db).update_many(people)  # One transaction for all
```

## Descriptor Contract

Every binding registers its models with the Rust core as *descriptors*
(`crates/orm/src/descriptor.rs`). The registry stores descriptors as-is — it
never resolves type inheritance — so each descriptor must be self-contained:
runtime query building and hydration consume exactly the lists the binding
provided.

### Inherited members flatten into subtypes

For an entity or relation **subtype**, `owned_attributes` re-lists inherited
attributes (parent declaration order first, then own). Relation `roles`
follow the same rule with one refinement: the list is the **effective role
set** —

- plain-inherited parent roles are flattened in (parent order first),
- own and specializing roles follow in declaration order,
- a parent role overridden via `relates child as parent` is **excluded**.

Both bindings (Python `_rust_runtime.relation_descriptor`, TypeScript
`Relation().descriptor()`) must emit byte-identical role lists; the
cross-language parity suite (`tests/integration/parity/`) enforces this for
parented and unparented types alike.

### Why the effective set (engine evidence)

Decided for #139 against TypeDB 3.11.5 (probe transcript:
`contribution relates contributor, relates work` /
`authoring sub contribution, relates author as contributor`):

| Probe | Engine verdict |
| --- | --- |
| `authoring` instance links a direct `contributor` player | REJECT (`INF11`: no compatible types for `links`) |
| `authoring` links `author` + plain-inherited `work` | ACCEPT |
| `match $r relates contributor` | rows: `contribution`, `authoring` (schema view keeps the edge) |
| `contribution` instance links `contributor` directly | ACCEPT (parent unaffected) |
| `match $r isa contribution, links (contributor: $x)` | rows include `authoring` (polymorphic read still works) |

An overridden parent role is *unplayable on subtype instances*, so listing it
(former TypeScript behavior) advertises a role the engine rejects; omitting
plain-inherited roles (former Python behavior) starves query building and
hydration of roles the engine accepts. The effective set is exactly what the
engine permits at instance level.

Schema-level introspection still sees inherited `relates` edges (third probe
row); the descriptor is an instance-facing contract, which is why playability
governs.

### Forward compatibility

Role specialization authoring (#140-A) extends `RoleDescriptor` with an
`overrides` link. The effective-set shape composes with it: the specializing
role carries `overrides: <parent role>`, from which the full schema picture
(including the replaced parent role) is reconstructable without re-listing
unplayable roles.

### Abstract roles (engine evidence)

Decided for #140-A against TypeDB 3.11.5 (probe shapes:
`interaction @abstract, relates participant @abstract` with
`collaboration sub interaction, relates collaborator as participant` and a
plain subtype `chat sub interaction`; plus a concrete relation
`meeting, relates attendee @abstract, relates room`):

| Probe | Engine verdict |
| --- | --- |
| `relates r @abstract` on an abstract relation | define ACCEPT |
| `relates r @abstract` on a concrete relation | define ACCEPT |
| Concrete relation instance links its own abstract role directly | REJECT (`INF11`) |
| Subtype that overrides the abstract role links it directly | REJECT (`INF11`, same as non-abstract override) |
| Subtype links the specializing role | ACCEPT |
| Plain-inheriting concrete subtype links the inherited abstract role | ACCEPT |
| `relates y as x @card(0..1)` (`as` before annotations) | define ACCEPT |
| `@abstract @card(...)` and `@card(...) @abstract` orders | both define ACCEPT |

Two consequences:

- **Abstractness gates direct play only at the declaring type's own scope.**
  A concrete subtype that plain-inherits an abstract role can play it, so
  inherited abstract roles **stay in the subtype's effective role set**. On
  the declaring relation itself the role is schema-present but unplayable on
  direct instances — the optional-role fetch partition already tolerates a
  permanently empty role.
- **Emission canon is free to pick one annotation order.** The generator
  emits `relates <name>[ as <parent>][ @abstract][ @card(...)]`; the parser
  accepts either annotation order, so round-trip stability comes from the
  emitter's fixed canon.

## Deprecated APIs

The following APIs are deprecated and should NOT be used:

### Removed Type Aliases

❌ **`Long`** - Renamed to `Integer` to match TypeDB 3.x

```python
# ❌ DEPRECATED
from type_bridge import Long
class Age(Long):
    pass

# ✅ USE INSTEAD
from type_bridge import Integer
class Age(Integer):
    pass
```

### Removed Cardinality Types

❌ **`Cardinal`** - Use `Flag(Card(...))` instead

```python
# ❌ DEPRECATED
from type_bridge import Cardinal
tags: Cardinal[2, None, Tag]

# ✅ USE INSTEAD
from type_bridge import Card, Flag
tags: list[Tag] = Flag(Card(min=2))
```

❌ **`Min[N, Type]`** - Use `list[Type] = Flag(Card(min=N))` instead

```python
# ❌ DEPRECATED
from type_bridge import Min
tags: Min[2, Tag]

# ✅ USE INSTEAD
from type_bridge import Card, Flag
tags: list[Tag] = Flag(Card(min=2))
```

❌ **`Max[N, Type]`** - Use `list[Type] = Flag(Card(max=N))` instead

```python
# ❌ DEPRECATED
from type_bridge import Max
tags: Max[5, Tag]

# ✅ USE INSTEAD
from type_bridge import Card, Flag
tags: list[Tag] = Flag(Card(max=5))
```

❌ **`Range[Min, Max, Type]`** - Use `list[Type] = Flag(Card(min, max))` instead

```python
# ❌ DEPRECATED
from type_bridge import Range
tags: Range[1, 5, Tag]

# ✅ USE INSTEAD
from type_bridge import Card, Flag
tags: list[Tag] = Flag(Card(1, 5))
```

### Removed Type Hint Aliases

❌ **`Optional[Type]`** - Use `Type | None` (PEP 604 syntax) instead

```python
# ❌ DEPRECATED
from typing import Optional
age: Optional[Age]

# ✅ USE INSTEAD (PEP 604)
age: Age | None = None
```

❌ **`Union[X, Y]`** - Use `X | Y` (PEP 604 syntax) instead

```python
# ❌ DEPRECATED
from typing import Union
result: Union[int, str]

# ✅ USE INSTEAD (PEP 604)
result: int | str
```

### Removed Flag Aliases

❌ **`EntityFlags`** - Use `TypeFlags` instead

```python
# ❌ DEPRECATED
from type_bridge import EntityFlags
class Person(Entity):
    flags = EntityFlags(name="person")

# ✅ USE INSTEAD
from type_bridge import TypeFlags
class Person(Entity):
    flags = TypeFlags(name="person")
```

❌ **`RelationFlags`** - Use `TypeFlags` instead

```python
# ❌ DEPRECATED
from type_bridge import RelationFlags
class Employment(Relation):
    flags = RelationFlags(name="employment")

# ✅ USE INSTEAD
from type_bridge import TypeFlags
class Employment(Relation):
    flags = TypeFlags(name="employment")
```

### Migration Guide

If you're updating code that uses deprecated APIs:

**Step 1: Update imports**

```python
# Before
from type_bridge import Long, Optional, EntityFlags, RelationFlags, Cardinal

# After
from type_bridge import Integer, TypeFlags, Card, Flag
```

**Step 2: Update type annotations**

```python
# Before
age: Optional[Age]
result: Union[int, str]

# After
age: Age | None = None
result: int | str
```

**Step 3: Update cardinality**

```python
# Before
tags: Cardinal[2, None, Tag]

# After
tags: list[Tag] = Flag(Card(min=2))
```

**Step 4: Update flags**

```python
# Before
flags = EntityFlags(name="person")
flags = RelationFlags(name="employment")

# After
flags = TypeFlags(name="person")
flags = TypeFlags(name="employment")
```

### Why These Changes?

These deprecations provide a cleaner, more consistent API following modern Python standards:

1. **PEP 604**: Native union syntax (`X | Y`) is now standard in Python 3.10+
2. **PEP 695**: Type parameter syntax is cleaner in Python 3.12+
3. **Unified API**: `TypeFlags` works for both entities and relations
4. **Explicit cardinality**: `Flag(Card(...))` is more explicit than type aliases
5. **TypeDB 3.x alignment**: `Integer` matches TypeDB's renamed `long` type

---

For API usage, see the [User Guide](../guide/index.md).

For development guidelines, see [setup.md](setup.md).
