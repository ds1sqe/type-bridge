# Code Generator

Generate TypeBridge Python models from TypeDB schema files (`.tql`).

## Overview

The generator eliminates manual synchronization between TypeDB schemas and Python code. Instead of writing both `.tql` and Python classes, you write the schema once in TypeQL and generate type-safe Python models.

```text
schema.tql  →  generator  →  attributes.py
                          →  entities.py
                          →  relations.py
                          →  __init__.py
```

## Quick Start

### CLI Usage

```bash
# Generate models from a schema file
python -m type_bridge.generator schema.tql -o ./myapp/models/

# With options
python -m type_bridge.generator schema.tql \
    --output ./myapp/models/ \
    --version 2.0.0 \
    --implicit-keys id
```

### Programmatic Usage

```python
from type_bridge.generator import generate_models

# From a file path
generate_models("schema.tql", "./myapp/models/")

# From schema text
schema = """
define
entity person, owns name @key;
attribute name, value string;
"""
generate_models(schema, "./myapp/models/")
```

## CLI Reference

```text
Usage: python -m type_bridge.generator [OPTIONS] SCHEMA

Arguments:
  SCHEMA  Path to the TypeDB schema file (.tql) [required]

Options:
  -o, --output PATH       Output directory for generated package [required]
  --version TEXT          Schema version string [default: 1.0.0]
  --no-copy-schema        Don't copy the schema file to the output directory
  --implicit-keys TEXT    Attribute names to treat as @key even if not marked
  --dto                   Generate Pydantic API DTOs (api_dto.py)
  --dto-config TEXT       Python module path to DTOConfig (e.g., myapp.config:dto_config)
  --help                  Show this message and exit
```

The `--output` directory is **required**. We recommend a dedicated directory like `./myapp/models/` or `./src/schema/` to keep generated code separate from hand-written code.

## Generated Package Structure

```text
myapp/models/
├── __init__.py      # Package exports, SCHEMA_VERSION, schema_text()
├── attributes.py    # Attribute class definitions
├── entities.py      # Entity class definitions
├── relations.py     # Relation class definitions
├── structs.py       # Struct definitions (if schema has structs)
├── functions.py     # Function metadata (if schema has functions)
├── registry.py      # Pre-computed schema metadata
├── api_dto.py       # Pydantic API DTOs (if --dto flag used)
└── schema.tql       # Copy of original schema (unless --no-copy-schema)
```

### Using the Generated Package

```python
from myapp.models import attributes, entities, relations
from myapp.models import SCHEMA_VERSION, schema_text

# Access generated classes
person = entities.Person(name=attributes.Name("Alice"))

# Get schema version
print(SCHEMA_VERSION)  # "1.0.0"

# Get original schema text
print(schema_text())
```

### Schema Registry

The generated `registry.py` provides pre-computed schema metadata for runtime queries without reflection:

```python
from myapp.models.registry import (
    get_entity_attributes,
    get_relation_attributes,
    get_role_players,
    ENTITY_ATTRIBUTES,
    RELATION_ATTRIBUTES,
    RELATION_ROLES,
)

# Get attributes owned by an entity type
attrs = get_entity_attributes("person")  # frozenset({"name", "age", "email"})

# Get attributes owned by a relation type
rel_attrs = get_relation_attributes("employment")  # frozenset({"start-date", "salary"})

# Get role player types for a relation role
players = get_role_players("employment", "employee")  # ("person",)
```

## Supported TypeQL Features

The generator supports the full TypeDB 3.0 schema syntax:

| Feature                     | Status |
| --------------------------- | ------ |
| Attributes with value types | ✓      |
| `@abstract` types           | ✓      |
| `@independent` attributes   | ✓      |
| `sub` inheritance           | ✓      |
| `@regex` constraints        | ✓      |
| `@values` constraints       | ✓      |
| `@range` constraints        | ✓      |
| `@key` / `@unique`          | ✓      |
| `@card` on owns             | ✓      |
| `@card` on plays            | ✓      |
| `@card` on relates          | ✓      |
| `@cascade` on owns          | ✓      |
| `@subkey` on owns           | ✓      |
| `@distinct` on relates      | ✓      |
| Role overrides (`as`)       | ✓      |
| Functions (`fun`)           | ✓      |
| Structs (`struct`)          | ✓      |
| `#` and `//` comments       | ✓      |

### Attributes

```typeql
// Basic attribute with value type
attribute name, value string;

// Abstract attribute (generates inheritance)
attribute id @abstract, value string;
attribute person-id sub id;

// Independent attribute (can exist without owner)
attribute language @independent, value string;

// With constraints
attribute email, value string @regex("^.*@.*$");
attribute status, value string @values("active", "inactive");

// Range constraints
attribute age, value integer @range(0..150);
attribute latitude, value double @range(-90.0..90.0);
attribute birth-date, value date @range(1900-01-01..2100-12-31);
attribute created-at, value datetime @range(1970-01-01T00:00:00..);  // Open-ended
```

**Generated Python:**

```python
class Name(String):
    flags = AttributeFlags(name="name")

class Id(String):
    flags = AttributeFlags(name="id")

class PersonId(Id):
    flags = AttributeFlags(name="person-id")

class Language(String):
    flags = AttributeFlags(name="language")
    independent: ClassVar[bool] = True

class Email(String):
    flags = AttributeFlags(name="email")
    regex: ClassVar[str] = r"^.*@.*$"

class Status(String):
    flags = AttributeFlags(name="status")
    allowed_values: ClassVar[tuple[str, ...]] = ("active", "inactive",)

class Age(Integer):
    flags = AttributeFlags(name="age")
    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")
```

### Entities

```typeql
// Basic entity
entity person,
    owns name @key,
    owns age,
    plays employment:employee;

// Abstract entity with inheritance
entity content @abstract,
    owns id @key;

entity post sub content,
    owns title,
    owns body;

// Cardinality constraints on owns
entity page,
    owns tag @card(0..10),
    owns name @card(1..3);

// Cardinality constraints on plays
entity user,
    plays friendship:friend @card(0..100),
    plays posting:author @card(0..);
```

**Generated Python:**

```python
class Person(Entity):
    flags = TypeFlags(name="person")
    plays: ClassVar[tuple[str, ...]] = ("employment:employee",)
    name: attributes.Name = Flag(Key)
    age: attributes.Age | None = None

class Content(Entity):
    flags = TypeFlags(name="content", abstract=True)
    id: attributes.Id = Flag(Key)

class Post(Content):
    flags = TypeFlags(name="post")
    title: attributes.Title | None = None
    body: attributes.Body | None = None

class Page(Entity):
    flags = TypeFlags(name="page")
    tag: list[attributes.Tag] = Flag(Card(0, 10))
    name: list[attributes.Name] = Flag(Card(1, 3))
```

### Relations

```typeql
// Basic relation
relation employment,
    relates employer,
    relates employee;

// Relation with inheritance and role override
relation contribution @abstract,
    relates contributor,
    relates work;

relation authoring sub contribution,
    relates author as contributor;  // Role override

// Relation with attributes
relation review,
    relates reviewer,
    relates reviewed,
    owns score @card(1),      // Required attribute
    owns timestamp;           // Optional (no @card = 0..1)

// Cardinality constraints on roles
relation social-relation @abstract,
    relates related @card(0..);

relation friendship sub social-relation,
    relates friend as related @card(0..1000);

relation parentship,
    relates parent @card(1..2),
    relates child @card(1..);
```

**Generated Python:**

```python
class Employment(Relation):
    flags = TypeFlags(name="employment")
    employer: Role[entities.Company] = Role("employer", entities.Company)
    employee: Role[entities.Person] = Role("employee", entities.Person)

class Friendship(SocialRelation):
    flags = TypeFlags(name="friendship")
    # Symmetric role - multiple friends allowed per friendship
    friend: Role[entities.Person] = Role("friend", entities.Person, cardinality=Card(0, 1000))

class Parentship(Relation):
    flags = TypeFlags(name="parentship")
    parent: Role[entities.Person] = Role("parent", entities.Person, cardinality=Card(1, 2))
    child: Role[entities.Person] = Role("child", entities.Person, cardinality=Card(1))

class Contribution(Relation):
    flags = TypeFlags(name="contribution", abstract=True)
    contributor: Role[entities.Contributor] = Role("contributor", entities.Contributor)
    work: Role[entities.Publication] = Role("work", entities.Publication)

class Authoring(Contribution):
    flags = TypeFlags(name="authoring")
    author: Role[entities.Contributor] = Role("author", entities.Contributor)

class Review(Relation):
    flags = TypeFlags(name="review")
    score: attributes.Score                      # Required (@card(1))
    timestamp: attributes.Timestamp | None = None  # Optional (no @card)
    reviewer: Role[entities.User] = Role("reviewer", entities.User)
    reviewed: Role[entities.Publication] = Role("reviewed", entities.Publication)
```

### Structs (TypeDB 3.0)

Structs are composite value types introduced in TypeDB 3.0. They are rendered as frozen dataclasses.

```typeql
struct person-name,
    value first-name string,
    value last-name string,
    value middle-name string?;  // Optional field

struct address,
    value street string,
    value city string,
    value postal-code string,
    value country string;
```

**Generated Python (`structs.py`):**

```python
from dataclasses import dataclass
from typing import ClassVar

@dataclass(frozen=True, slots=True)
class PersonName:
    """Struct for `person-name`."""
    first_name: str
    last_name: str
    middle_name: str | None = None

@dataclass(frozen=True, slots=True)
class Address:
    """Struct for `address`."""
    street: str
    city: str
    postal_code: str
    country: str
```

### Additional Annotations

#### `@cascade` - Cascading Deletes

When an entity is deleted, cascade delete to owned attributes:

```typeql
entity person,
    owns email @cascade,      // Delete emails when person is deleted
    owns name @key;
```

**Generated (metadata tracked on EntitySpec/RelationSpec):**

```python
# Accessible via schema registry
cascades: set[str] = {"email"}
```

#### `@subkey` - Composite Keys

Group attributes into composite keys:

```typeql
entity order-item,
    owns order-id @subkey(order),
    owns product-id @subkey(order),
    owns quantity;
```

**Generated (metadata tracked):**

```python
subkeys: dict[str, str] = {"order-id": "order", "product-id": "order"}
```

#### `@distinct` - Distinct Role Players

Ensure role players are distinct within a relation instance:

```typeql
relation friendship,
    relates friend @distinct @card(2);  // Can't be friends with yourself
```

**Generated (tracked on RoleSpec):**

```python
# Role has distinct=True
```

## Cardinality Mapping

The following cardinality rules apply to attributes on both **entities** and **relations**:

| TypeQL                         | Python Type                      | Default                   |
| ------------------------------ | -------------------------------- | ------------------------- |
| `@card(1)` or `@card(1..1)`    | `Type`                           | Required                  |
| `@card(0..1)` or no annotation | `Type \| None = None`            | Optional                  |
| `@card(0..)`                   | `list[Type] = Flag(Card(min=0))` | Optional list             |
| `@card(1..)`                   | `list[Type] = Flag(Card(min=1))` | Required list             |
| `@card(2..5)`                  | `list[Type] = Flag(Card(2, 5))`  | Bounded list              |
| `@key`                         | `Type = Flag(Key)`               | Key (implies required)    |
| `@unique`                      | `Type = Flag(Unique)`            | Unique (implies required) |

**Inheritance:** Child types inherit cardinality constraints from parent types. A child can override inherited constraints by redeclaring the attribute with a different `@card`.

## Comments

The parser supports both `#` (shell-style) and `//` (C-style) comments:

```typeql
# This is a shell-style comment
// This is a C-style comment
attribute name, value string;  // Inline comment
entity person, owns name;  # Also inline
```

### Comment Annotations

The generator supports special comment annotations for customizing output:

```typeql
# @prefix(PERSON_)
# Custom prefix for IDs
entity person,
    owns id @key;

# @internal
# This entity is for internal use
entity audit-log,
    owns timestamp;

# @tags(api, public)
entity user,
    owns username @key;
```

| Annotation            | Effect                               |
| --------------------- | ------------------------------------ |
| `# @prefix(XXX)`      | Adds `prefix: ClassVar[str] = "XXX"` |
| `# @internal`         | Sets `internal = True` on the spec   |
| `# @case(SNAKE_CASE)` | Uses specified case for type name    |
| `# @transform(xxx)`   | Adds `transform = "xxx"` attribute   |
| `# @tags(a, b, c)`    | Adds list annotation                 |
| `# Any other comment` | Becomes the class docstring          |

## Functions

TypeDB functions (`fun` declarations) are fully parsed and can be used for metadata. The generator extracts function signatures including parameters and return types.

### Supported Function Syntax

```typeql
// Stream return (single type)
fun user_phones($user: user) -> { phone }:
    match $user has phone $phone;
    return { $phone };

// Stream return (tuple)
fun all_users_and_phones() -> { user, phone, string }:
    match $user isa user, has phone $phone;
    return { $user, $phone, "value" };

// Single scalar return
fun add($x: integer, $y: integer) -> integer:
    match let $z = $x + $y;
    return first $z;

// Tuple return
fun divide($a: integer, $b: integer) -> integer, integer:
    match let $q = $a / $b;
    return first $q, $r;

// Bool return type
fun is_reachable($from: node, $to: node) -> bool:
    match ($from, $to) isa edge;
    return first true;

// Optional return type
fun any_place_with_optional_name() -> place, name?:
    match $p isa place;
    return first $p, $n;

// No parameters
fun mean_karma() -> double:
    match $user isa user, has karma $karma;
    return mean($karma);

// Aggregate returns
fun karma_sum_and_squares() -> double, double:
    match $karma isa karma;
    return sum($karma), sum($karma);
```

### Function Return Types

| TypeQL          | Parsed `return_type` |
| --------------- | -------------------- |
| `-> { type }`   | `"{ type }"`         |
| `-> { t1, t2 }` | `"{ t1, t2 }"`       |
| `-> type`       | `"type"`             |
| `-> t1, t2`     | `"t1, t2"`           |
| `-> t1, t2?`    | `"t1, t2?"`          |
| `-> bool`       | `"bool"`             |

## API Reference

### `generate_models()`

```python
def generate_models(
    schema: str | Path,
    output_dir: str | Path,
    *,
    implicit_key_attributes: Iterable[str] | None = None,
    schema_version: str = "1.0.0",
    copy_schema: bool = True,
    schema_path: str | Path | None = None,
    generate_dto: bool = False,
) -> None:
    """Generate TypeBridge models from a TypeDB schema.

    Args:
        schema: Path to .tql file or schema text content
        output_dir: Directory to write generated package
        implicit_key_attributes: Attribute names to treat as @key
        schema_version: Version string for SCHEMA_VERSION constant
        copy_schema: Whether to copy schema.tql to output directory
        schema_path: Custom path for the schema file (relative to output_dir)
        generate_dto: Whether to generate Pydantic API DTOs (api_dto.py)
    """
```

### `parse_tql_schema()`

```python
def parse_tql_schema(schema_content: str) -> ParsedSchema:
    """Parse a TypeDB schema into intermediate representation.

    Args:
        schema_content: TypeQL schema text

    Returns:
        ParsedSchema with attributes, entities, and relations
    """
```

### `ParsedSchema`

```python
@dataclass
class ParsedSchema:
    """Container for parsed schema components."""
    attributes: dict[str, AttributeSpec]
    entities: dict[str, EntitySpec]
    relations: dict[str, RelationSpec]
    functions: dict[str, FunctionSpec]
    structs: dict[str, StructSpec]

    def accumulate_inheritance(self) -> None:
        """Propagate owns/plays/keys down inheritance hierarchies."""
```

### `StructSpec`

```python
@dataclass
class StructSpec:
    """Struct definition extracted from a TypeDB schema."""
    name: str                           # e.g., "person-name"
    fields: list[StructFieldSpec]       # Struct fields
    docstring: str | None               # Optional docstring
    annotations: dict[str, Any]         # Custom annotations

@dataclass
class StructFieldSpec:
    """Field definition within a struct."""
    name: str        # e.g., "first-name"
    value_type: str  # e.g., "string"
    optional: bool   # Whether field is optional (?)
```

### `FunctionSpec`

```python
@dataclass
class FunctionSpec:
    """Function definition extracted from a TypeDB schema."""
    name: str                        # e.g., "calculate-age"
    parameters: list[ParameterSpec]  # Function parameters
    return_type: str                 # e.g., "{ person }" or "integer, integer"

@dataclass
class ParameterSpec:
    """Parameter definition for a TypeDB function."""
    name: str   # e.g., "birth-date"
    type: str   # e.g., "date" or "user"
```

## API DTOs (Pydantic)

The generator can optionally create Pydantic-based Data Transfer Objects for building REST APIs. Enable this with `--dto` on the CLI or `generate_dto=True` programmatically.

### Generated DTO Structure

When enabled, `api_dto.py` is generated with:

- **Base classes**: `BaseDTO`, `BaseDTOOut`, `BaseDTOCreate`, `BaseDTOPatch`
- **Entity DTOs**: `{Entity}Out`, `{Entity}Create`, `{Entity}Patch` for each non-abstract entity
- **Relation DTOs**: `{Relation}Out`, `{Relation}Create` for each non-abstract relation
- **Union types**: `EntityOut`, `EntityCreate`, `EntityPatch`, `RelationOut`, `RelationCreate`

### Example Usage

```bash
# Generate with Pydantic DTOs
python -m type_bridge.generator schema.tql -o ./myapp/models/ --dto
```

```python
# Programmatic generation
from type_bridge.generator import generate_models

generate_models("schema.tql", "./myapp/models/", generate_dto=True)
```

### Using Generated DTOs

```python
from myapp.models.api_dto import PersonOut, PersonCreate, EntityOut

# Create a new person via API
@app.post("/persons")
def create_person(data: PersonCreate) -> PersonOut:
    # data is validated by Pydantic
    person = Person(name=Name(data.name), age=Age(data.age))
    person_manager.insert(person)
    return PersonOut(iid=person.iid, name=data.name, age=data.age, type="person")

# Handle any entity type with discriminated union
@app.get("/entities/{id}")
def get_entity(id: str) -> EntityOut:
    # Returns the appropriate *Out type based on "type" field
    ...
```

### DTO Class Hierarchy

```text
BaseDTO
├── BaseDTOOut          # For entity responses (includes 'iid' field)
├── BaseDTOCreate       # For entity create payloads
├── BaseDTOPatch        # For entity partial updates (extra="forbid")
├── BaseRelationOut     # For relation responses
└── BaseRelationCreate  # For relation create payloads
```

For each entity `Foo` in your schema:

- `FooOut` - Response DTO with all attributes and a `type` discriminator
- `FooCreate` - Create payload DTO (required fields from `@key`, `@unique`, or `@card(1)`)
- `FooPatch` - Partial update DTO (all fields optional)

For each relation `Bar` in your schema:

- `BarOut` - Response DTO with role player IIDs and owned attributes
- `BarCreate` - Create payload with role player identifiers

### Features

- **Schema-driven**: All DTOs are generated directly from your TypeDB schema
- **Inheritance support**: DTO inheritance mirrors your schema's type hierarchy
- **Required field detection**: Uses `@key`, `@unique`, and `@card` annotations
- **Literal type discriminators**: Each DTO has a `type` field for union discrimination
- **Pydantic validation**: All DTOs use Pydantic for automatic validation
- **Discriminated unions**: `EntityOut`, `RelationCreate`, etc. use the `type` field

### DTOConfig - Custom Configuration

For advanced use cases, you can configure DTO generation with `DTOConfig`:

```python
from type_bridge.generator import (
    DTOConfig,
    BaseClassConfig,
    ValidatorConfig,
    FieldSyncConfig,
    generate_models,
)

config = DTOConfig(
    # Custom base classes for entity hierarchies
    base_classes=[
        BaseClassConfig(
            source_entity="artifact",         # Entities inheriting from 'artifact'
            base_name="BaseArtifact",         # Will use BaseArtifactOut, etc.
            inherited_attrs=["display_id", "name", "description", "status"],
            extra_fields={"version": "int | None = None"},  # Add computed fields
            field_syncs=[FieldSyncConfig("description", "content")],
        ),
    ],

    # Custom validators (generates Annotated types with AfterValidator)
    validators=[
        ValidatorConfig(name="DisplayId", pattern=r"^[A-Z]{1,10}-\d+$"),
    ],

    # Custom Python code injected after imports
    preamble='''
def _validate_node_id(value: str) -> str:
    """Accept display IDs, UUIDs, or TypeDB IIDs."""
    # Custom validation logic here
    return value

NodeId = Annotated[str, AfterValidator(_validate_node_id)]
''',

    # Customize union type names (default: "Entity", "Relation")
    entity_union_name="GraphNode",      # GraphNodeOut, GraphNodeCreate, etc.
    relation_union_name="GraphRelation",

    # Exclude internal entities from generation
    exclude_entities=["display_id_counter", "schema_status"],

    # Rename IID field (default: "iid", use "id" for conventional REST APIs)
    iid_field_name="id",

    # Skip generating relation Out classes (use with relation_preamble for custom output)
    skip_relation_output=True,

    # Custom base class for relation creates (must be defined in preamble/relation_preamble)
    relation_create_base_class="BaseRelationCreate",

    # Custom code for relation section (custom output classes, base classes)
    relation_preamble='''
class GraphEdgeOut(BaseDTO):
    """Custom edge output for graph API."""
    iid: str
    source: str
    target: str
    type: str

class BaseRelationCreate(BaseDTO):
    """Base for relation creates with generic source/target."""
    source_id: str
    target_id: str
''',
)

generate_models("schema.tql", "./models/", generate_dto=True, dto_config=config)
```

**CLI usage:**

```bash
# With a config module
python -m type_bridge.generator schema.tql -o ./models/ --dto --dto-config myapp.config:dto_config
```

The config module (`myapp/config.py`):

```python
from type_bridge.generator import DTOConfig, BaseClassConfig

dto_config = DTOConfig(
    base_classes=[
        BaseClassConfig(
            source_entity="artifact",
            base_name="BaseArtifact",
            inherited_attrs=["display_id", "name", "description"],
        ),
    ],
)
```

### DTOConfig Reference

| Option                       | Type                    | Default      | Description                                                 |
| ---------------------------- | ----------------------- | ------------ | ----------------------------------------------------------- |
| `base_classes`               | `list[BaseClassConfig]` | `[]`         | Custom base classes for entity hierarchies                  |
| `validators`                 | `list[ValidatorConfig]` | `[]`         | Custom validator types with regex patterns                  |
| `preamble`                   | `str \| None`           | `None`       | Python code injected after imports                          |
| `entity_union_name`          | `str`                   | `"Entity"`   | Name prefix for entity union types                          |
| `relation_union_name`        | `str`                   | `"Relation"` | Name prefix for relation union types                        |
| `exclude_entities`           | `list[str]`             | `[]`         | Entity names to exclude from generation                     |
| `iid_field_name`             | `str`                   | `"iid"`      | Field name for TypeDB IID (use `"id"` for REST conventions) |
| `skip_relation_output`       | `bool`                  | `False`      | Skip generating relation Out classes                        |
| `relation_create_base_class` | `str \| None`           | `None`       | Custom base class for relation Create DTOs                  |
| `relation_preamble`          | `str \| None`           | `None`       | Python code injected in relation section                    |

### BaseClassConfig Reference

| Option            | Type                    | Default  | Description                                            |
| ----------------- | ----------------------- | -------- | ------------------------------------------------------ |
| `source_entity`   | `str`                   | Required | Schema entity that triggers this base class            |
| `base_name`       | `str`                   | Required | Base class name prefix (e.g., `"BaseArtifact"`)        |
| `inherited_attrs` | `list[str]`             | `[]`     | Attributes defined in base class (skipped in children) |
| `extra_fields`    | `dict[str, str]`        | `{}`     | Additional fields as `{name: type_annotation}`         |
| `field_syncs`     | `list[FieldSyncConfig]` | `[]`     | Field sync validators for this base                    |
| `validators`      | `list[str]`             | `[]`     | Validator names to apply to fields                     |

## Best Practices

### 1. Keep Generated Code Separate

```text
myapp/
├── models/          # Generated (don't edit!)
│   ├── __init__.py
│   ├── attributes.py
│   ├── entities.py
│   └── relations.py
├── services/        # Hand-written business logic
└── schema.tql       # Source of truth
```

### 2. Regenerate After Schema Changes

```bash
# Add to your workflow
python -m type_bridge.generator schema.tql -o ./myapp/models/
```

### 3. Version Control the Schema, Not Generated Code

```gitignore
# .gitignore
myapp/models/  # Generated - regenerate from schema.tql
```

Or version control both for CI/CD verification:

```bash
# CI check: ensure generated code is up to date
python -m type_bridge.generator schema.tql -o ./myapp/models/
git diff --exit-code myapp/models/
```

### 4. Use `--implicit-keys` for Convention-Based Keys

If your schema uses `id` as a key by convention:

```bash
python -m type_bridge.generator schema.tql -o ./models/ --implicit-keys id
```

## See Also

- [API DTOs Documentation](dto.md) - Complete DTOConfig reference and examples
- [Entities Documentation](entities.md) - Entity inheritance and ownership
- [Relations Documentation](relations.md) - Relations, roles, and role players
- [Attributes Documentation](attributes.md) - Attribute types and constraints
- [Cardinality Documentation](cardinality.md) - Card API and Flag system
