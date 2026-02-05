# API DTOs (Pydantic)

Generate Pydantic Data Transfer Objects for building REST APIs from your TypeDB schema.

## Overview

The DTO generator creates Pydantic models that mirror your TypeDB schema, providing:

- **Type-safe API contracts** - Request/response validation via Pydantic
- **Schema-driven generation** - DTOs stay in sync with your database schema
- **Discriminated unions** - Handle polymorphic entities with `type` field discrimination
- **Customizable output** - Configure base classes, validators, field names, and more

## Quick Start

```bash
# Generate with DTOs
python -m type_bridge.generator schema.tql -o ./myapp/models/ --dto
```

```python
# Programmatic
from type_bridge.generator import generate_models

generate_models("schema.tql", "./myapp/models/", generate_dto=True)
```

This generates `api_dto.py` alongside your other model files.

## Generated Structure

For a schema with entities and relations, the generator creates:

```python
# Base classes
class BaseDTO(BaseModel): ...
class BaseDTOOut(BaseDTO): ...      # Has 'iid' field (TypeDB internal ID)
class BaseDTOCreate(BaseDTO): ...
class BaseDTOPatch(BaseDTO): ...    # extra="forbid"
class BaseRelationOut(BaseDTO): ... # Has 'iid' and 'type'
class BaseRelationCreate(BaseDTO): ...

# Entity DTOs (for each non-abstract entity)
class PersonOut(BaseDTOOut): ...
class PersonCreate(BaseDTOCreate): ...
class PersonPatch(BaseDTOPatch): ...

# Relation DTOs (for each non-abstract relation)
class FriendshipOut(BaseRelationOut):
    friend_iid: Optional[str]  # Role player's TypeDB IID (for reading)
    ...
class FriendshipCreate(BaseRelationCreate):
    friend_id: str             # Role player identifier (for resolving)
    ...

# Union types (discriminated by 'type' field)
EntityOut = Annotated[Union[PersonOut, ...], Field(discriminator="type")]
EntityCreate = Annotated[Union[PersonCreate, ...], Field(discriminator="type")]
EntityPatch = Annotated[Union[PersonPatch, ...], Field(discriminator="type")]
RelationOut = Annotated[Union[FriendshipOut, ...], Field(discriminator="type")]
RelationCreate = Annotated[Union[FriendshipCreate, ...], Field(discriminator="type")]
```

## Usage Example

```python
from myapp.models.api_dto import PersonOut, PersonCreate, EntityOut

# FastAPI endpoint
@app.post("/persons", response_model=PersonOut)
def create_person(data: PersonCreate) -> PersonOut:
    person = Person(name=Name(data.name), age=Age(data.age))
    person_manager.insert(person)
    return PersonOut(
        iid=person.iid,
        name=data.name,
        age=data.age,
        type="person"
    )

# Polymorphic endpoint using discriminated union
@app.get("/entities/{id}", response_model=EntityOut)
def get_entity(id: str) -> EntityOut:
    # Returns PersonOut, CompanyOut, etc. based on actual type
    ...
```

## DTOConfig - Customization

For advanced use cases, configure generation with `DTOConfig`:

```python
from type_bridge.generator import (
    DTOConfig,
    BaseClassConfig,
    ValidatorConfig,
    FieldSyncConfig,
    generate_models,
)

config = DTOConfig(
    # ... options ...
)

generate_models("schema.tql", "./models/", generate_dto=True, dto_config=config)
```

### CLI with Config

```bash
python -m type_bridge.generator schema.tql -o ./models/ --dto --dto-config myapp.config:dto_config
```

Where `myapp/config.py` contains:

```python
from type_bridge.generator import DTOConfig

dto_config = DTOConfig(
    # ... your configuration ...
)
```

## Configuration Options

### DTOConfig Reference

| Option                       | Type                          | Default      | Description                                  |
| ---------------------------- | ----------------------------- | ------------ | -------------------------------------------- |
| `base_classes`               | `list[BaseClassConfig]`       | `[]`         | Custom base classes for entity hierarchies   |
| `validators`                 | `list[ValidatorConfig]`       | `[]`         | Custom validator types with regex patterns   |
| `preamble`                   | `str \| None`                 | `None`       | Python code injected after imports           |
| `entity_union_name`          | `str`                         | `"Entity"`   | Name prefix for entity union types           |
| `relation_union_name`        | `str`                         | `"Relation"` | Name prefix for relation union types         |
| `exclude_entities`           | `list[str]`                   | `[]`         | Entity names to exclude from generation      |
| `iid_field_name`             | `str`                         | `"iid"`      | Field name for TypeDB IID                    |
| `skip_relation_output`       | `bool`                        | `False`      | Skip generating relation Out classes         |
| `relation_create_base_class` | `str \| None`                 | `None`       | Custom base class for relation Create DTOs   |
| `relation_preamble`          | `str \| None`                 | `None`       | Python code injected in relation section     |
| `composite_entities`         | `list[CompositeEntityConfig]` | `[]`         | Composite (flat/merged) DTOs configuration   |
| `strict_out_models`          | `bool`                        | `False`      | Required fields are non-Optional in Out DTOs |

### BaseClassConfig Reference

| Option            | Type                    | Default  | Description                                      |
| ----------------- | ----------------------- | -------- | ------------------------------------------------ |
| `source_entity`   | `str`                   | Required | Schema entity that triggers this base class      |
| `base_name`       | `str`                   | Required | Base class name prefix (e.g., `"BaseArtifact"`)  |
| `inherited_attrs` | `list[str]`             | `[]`     | Attributes defined in base (skipped in children) |
| `extra_fields`    | `dict[str, str]`        | `{}`     | Additional fields as `{name: type_annotation}`   |
| `field_syncs`     | `list[FieldSyncConfig]` | `[]`     | Field sync validators for this base              |

### ValidatorConfig Reference

| Option    | Type          | Default  | Description                               |
| --------- | ------------- | -------- | ----------------------------------------- |
| `name`    | `str`         | Required | Validator type name (e.g., `"DisplayId"`) |
| `pattern` | `str \| None` | `None`   | Regex pattern for validation              |

### FieldSyncConfig Reference

| Option    | Type  | Description       |
| --------- | ----- | ----------------- |
| `field_a` | `str` | First field name  |
| `field_b` | `str` | Second field name |

When one field is set but not the other, the value is copied automatically.

### CompositeEntityConfig Reference

Composite entities create a "flat" DTO that merges multiple entity types into one class. Useful for polymorphic APIs where one endpoint handles all node types.

| Option                    | Type                         | Default  | Description                                         |
| ------------------------- | ---------------------------- | -------- | --------------------------------------------------- |
| `name`                    | `str`                        | Required | Name prefix (e.g., `"GraphNode"` → `GraphNodeOut`)  |
| `base_entity`             | `str \| None`                | `None`   | Entity name - include all inheriting entities       |
| `include_entities`        | `list[str]`                  | `[]`     | Explicit list of entities to include                |
| `exclude_entities`        | `list[str]`                  | `[]`     | Entities to exclude from the composite              |
| `common_fields`           | `list[CompositeFieldConfig]` | `[]`     | Explicitly configured common fields                 |
| `field_syncs`             | `list[FieldSyncConfig]`      | `[]`     | Field sync validators for the composite             |
| `extra_fields`            | `dict[str, str]`             | `{}`     | Additional fields as `{name: type_annotation}`      |
| `id_field_name`           | `str`                        | `"id"`   | Name of the ID field                                |
| `type_field_name`         | `str`                        | `"type"` | Name of the type discriminator field                |
| `type_enum_from_registry` | `bool`                       | `True`   | Generate `Literal` type enum from included entities |

Use `base_entity` OR `include_entities`, not both. When using `base_entity`, all non-abstract entities inheriting from it are included.

### CompositeFieldConfig Reference

| Option            | Type          | Default  | Description                                     |
| ----------------- | ------------- | -------- | ----------------------------------------------- |
| `name`            | `str`         | Required | Python field name (snake_case)                  |
| `type_annotation` | `str`         | Required | Python type annotation (e.g., `"str"`, `"int"`) |
| `default`         | `str \| None` | `None`   | Default value (e.g., `"None"`, `"''"`)          |
| `description`     | `str \| None` | `None`   | Optional field description                      |

If `default` is `None`, the field is required in Create DTOs.

## Common Patterns

### Composite DTOs for Polymorphic APIs

When your API needs to handle multiple entity types in one endpoint, use composite DTOs:

```python
from type_bridge.generator import (
    DTOConfig,
    CompositeEntityConfig,
    CompositeFieldConfig,
    FieldSyncConfig,
)

config = DTOConfig(
    composite_entities=[
        CompositeEntityConfig(
            name="GraphNode",
            base_entity="artifact",  # Include all entities inheriting from artifact
            exclude_entities=["internal_utility"],  # Exclude specific types
            common_fields=[
                CompositeFieldConfig("name", "str"),
                CompositeFieldConfig("description", "str", default="''"),
                CompositeFieldConfig("status", "str", default="'proposed'"),
            ],
            field_syncs=[FieldSyncConfig("description", "content")],
            extra_fields={"version": "int | None = None"},
        ),
    ]
)
```

This generates:

```python
# Type enum (from included entities)
GraphNodeType = Literal["task", "epic", "story", ...]

class GraphNodeOut(BaseDTOOut):
    type: GraphNodeType
    name: Optional[str] = None
    description: Optional[str] = None
    status: Optional[str] = None
    version: int | None = None
    # ... merged attributes from all included entities

class GraphNodeCreate(BaseDTOCreate):
    type: GraphNodeType
    name: str  # Required (no default)
    description: Optional[str] = ''
    status: Optional[str] = 'proposed'
    version: int | None = None

class GraphNodePatch(BaseDTOPatch):
    name: Optional[str] = None
    description: Optional[str] = None
    status: Optional[str] = None
    version: Optional[int] = None
```

Composite DTOs are generated **alongside** per-entity DTOs, giving you both:

- Per-entity DTOs (`TaskOut`, `EpicOut`) for type-specific endpoints
- Composite DTOs (`GraphNodeOut`) for polymorphic endpoints

### Relation Role Field Naming

For relation DTOs, role player fields use different suffixes depending on the DTO type:

- **Out models**: `{role}_iid` - Returns the TypeDB internal ID (IID) of the role player
- **Create models**: `{role}_id` - Accepts any identifier you can resolve to a role player

This distinction is intentional:

- **Reading**: When returning relation data, the IID uniquely identifies the connected entity
- **Creating**: When creating relations, you provide an ID (which could be a display ID, UUID, or IID) that your application resolves to the actual entity

```python
# Out: returns the actual TypeDB IIDs
class EmploymentOut(BaseRelationOut):
    employee_iid: Optional[str]  # e.g., "0x826e80018000000000000001"
    company_iid: Optional[str]

# Create: accepts identifiers your app can resolve
class EmploymentCreate(BaseRelationCreate):
    employee_id: str  # e.g., "EMP-123" or a UUID
    company_id: str
```

### Excluding Internal Entities

Exclude utility entities that shouldn't appear in your API:

```python
config = DTOConfig(
    exclude_entities=["display_id_counter", "schema_status", "audit_log"],
)
```

### Strict Out Models

By default, all fields in `Out` DTOs are `Optional` for safety. Enable `strict_out_models` to make required fields (those with `@key` or `min>=1` cardinality) non-optional:

```python
config = DTOConfig(strict_out_models=True)
```

This generates:

```python
# Without strict_out_models (default):
class PersonOut(BaseDTOOut):
    name: Optional[str] = None  # Even @key is Optional

# With strict_out_models=True:
class PersonOut(BaseDTOOut):
    name: str  # @key field is required
    nickname: Optional[str] = None  # Optional fields stay Optional
```

Use this when you want your API contracts to reflect the actual database guarantees.

### Renaming IID Field

Use conventional `id` instead of TypeDB's `iid`:

```python
config = DTOConfig(
    iid_field_name="id",  # BaseDTOOut will have 'id: str' instead of 'iid: str'
)
```

### Custom Entity Hierarchy Base Classes

When your schema has abstract entities (like `artifact`) with common attributes:

```python
config = DTOConfig(
    base_classes=[
        BaseClassConfig(
            source_entity="artifact",
            base_name="BaseArtifact",
            inherited_attrs=["display_id", "name", "description", "status", "created_at"],
            extra_fields={"version": "int | None = None"},
            field_syncs=[FieldSyncConfig("description", "content")],
        ),
    ],
)
```

This generates:

- `BaseArtifactOut`, `BaseArtifactCreate`, `BaseArtifactPatch` with common fields
- Child entities (e.g., `TaskOut`) inherit from `BaseArtifactOut` instead of `BaseDTOOut`
- Child DTOs only include their own attributes, not inherited ones

### Custom Validators

Add validated types for specific field formats:

```python
config = DTOConfig(
    validators=[
        ValidatorConfig(name="DisplayId", pattern=r"^[A-Z]{1,5}-\d+$"),
        ValidatorConfig(name="Email", pattern=r"^[^@]+@[^@]+\.[^@]+$"),
    ],
)
```

Generates:

```python
_DISPLAYID_PATTERN = re.compile(r"^[A-Z]{1,5}-\d+$")

def _validate_displayid(value: str) -> str:
    if not _DISPLAYID_PATTERN.match(value):
        raise ValueError(f"'{value}' is not a valid DisplayId format.")
    return value

DisplayId = Annotated[str, AfterValidator(_validate_displayid)]
```

### Complex Validators via Preamble

For validators with complex logic (not just regex):

```python
config = DTOConfig(
    preamble='''
_DISPLAY_ID_PATTERN = re.compile(r"^[A-Z]{1,5}-\\d+$")
_UUID_PATTERN = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.I)
_IID_PATTERN = re.compile(r"^0x[0-9a-f]{20,24}$", re.I)

def _validate_node_id(value: str) -> str:
    """Accept display IDs, UUIDs, or TypeDB IIDs."""
    if _DISPLAY_ID_PATTERN.match(value) or _UUID_PATTERN.match(value) or _IID_PATTERN.match(value):
        return value
    raise ValueError(f"'{value}' is not a valid node ID format.")

NodeId = Annotated[str, AfterValidator(_validate_node_id)]
''',
)
```

### Custom Union Names

Rename union types for your API conventions.

**Note:** The generator validates that entity/relation names don't clash with union type names. If you have an entity named `entity` (which generates class `Entity`), the default `entity_union_name="Entity"` will raise an error:

```text
ValueError: Entity 'entity' generates class name 'Entity' which clashes
with entity_union_name='Entity'. Use a different entity_union_name in
DTOConfig to avoid this conflict.
```

Simply choose a different union name:

```python
config = DTOConfig(
    entity_union_name="GraphNode",      # GraphNodeOut, GraphNodeCreate, GraphNodePatch
    relation_union_name="GraphRelation", # GraphRelationOut, GraphRelationCreate
)
```

### Custom Relation Structure

For APIs that use generic `source_id`/`target_id` instead of role-specific fields:

```python
config = DTOConfig(
    skip_relation_output=True,  # Don't generate FriendshipOut, etc.
    relation_create_base_class="BaseRelationCreate",  # Defined in preamble
    relation_preamble='''
class GraphEdgeOut(BaseDTO):
    """Custom edge output for graph API."""
    iid: str = Field(description="Relation IID")
    source: str = Field(description="Source node IID")
    target: str = Field(description="Target node IID")
    source_display_id: str
    target_display_id: str
    type: str
    role_from: str
    role_to: str

class BaseRelationCreate(BaseDTO):
    """Base for relation creates with generic source/target."""
    source_id: NodeId
    target_id: NodeId
    context_id: NodeId | None = None
''',
)
```

With this config:

- Relation Out classes are skipped (you use `GraphEdgeOut`)
- Relation Create classes inherit from your `BaseRelationCreate`
- Role-specific fields (`friend_id`, etc.) are not generated
- Only owned attributes are schema-driven

## Full Example

A complete configuration for a graph-based API:

```python
from type_bridge.generator import (
    DTOConfig,
    BaseClassConfig,
    FieldSyncConfig,
    ValidatorConfig,
    generate_models,
)

config = DTOConfig(
    # Exclude internal entities
    exclude_entities=["display_id_counter", "schema_status"],

    # Use conventional 'id' field
    iid_field_name="id",

    # Custom union names
    entity_union_name="GraphNode",
    relation_union_name="GraphRelation",

    # Custom validators
    validators=[
        ValidatorConfig(name="DisplayId", pattern=r"^[A-Z]{1,5}-\d+$"),
    ],

    # Complex validators
    preamble='''
_UUID_PATTERN = re.compile(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.I)
_IID_PATTERN = re.compile(r"^0x[0-9a-f]{20,24}$", re.I)

def _validate_node_id(value: str) -> str:
    if _DISPLAYID_PATTERN.match(value) or _UUID_PATTERN.match(value) or _IID_PATTERN.match(value):
        return value
    raise ValueError(f"'{value}' is not a valid node ID.")

NodeId = Annotated[str, AfterValidator(_validate_node_id)]
''',

    # Custom base class for artifacts
    base_classes=[
        BaseClassConfig(
            source_entity="artifact",
            base_name="BaseArtifact",
            inherited_attrs=["display_id", "name", "description", "status", "priority", "created_at", "updated_at"],
            extra_fields={"version": "int | None = None", "content": "str | None = None"},
            field_syncs=[FieldSyncConfig("description", "content")],
        ),
    ],

    # Custom relation structure
    skip_relation_output=True,
    relation_create_base_class="BaseRelationCreate",
    relation_preamble='''
class GraphEdgeOut(BaseDTO):
    """API response for graph edges."""
    iid: str = Field(description="Relation IID")
    source: str = Field(description="Source node IID")
    target: str = Field(description="Target node IID")
    source_display_id: str
    target_display_id: str
    type: str
    role_from: str
    role_to: str

class BaseRelationCreate(BaseDTO):
    """Base for creating graph edges."""
    source_id: NodeId
    target_id: NodeId
    context_id: NodeId | None = None
''',
)

generate_models("schema.tql", "./models/", generate_dto=True, dto_config=config)
```

## See Also

- [Code Generator](generator.md) - Full generator documentation
- [Entities](entities.md) - Entity inheritance and ownership
- [Relations](relations.md) - Relations, roles, and role players
