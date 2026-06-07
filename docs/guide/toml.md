# TOML Schema DSL

Author TypeDB schemas in TOML instead of TypeQL.

## Overview

TypeBridge accepts schemas in two formats: TypeQL (`.tql`) and a TOML
DSL (`.toml`). Both formats feed the same parser and code generator — TOML
is an alternative authoring surface, not a separate pipeline.

The TOML DSL is useful when you prefer a data-file syntax over TypeQL prose,
want IDE tooling for schema keys, or are generating schema files
programmatically.

## Routing

When `generate_models` receives a path that ends in `.toml`, it
automatically routes the file through the TOML transpiler before passing the
result to the generator. No extra flags are required.

```python
from type_bridge.generator import generate_models

# .toml suffix routes automatically through the transpiler
generate_models("schema.toml", "out/models/")
```

If you hold TOML text in memory (not in a file), pass `format="toml"` to
force the TOML path:

```python
toml_text = """
[attributes.name]
value = "string"

[entities.person]
owns = ["name"]
"""

generate_models(toml_text, "out/models/", format="toml")
```

A `.tql` file or raw TypeQL text continues to work exactly as before.

## DSL Reference

### Attributes

```toml
[attributes.NAME]
value = "string"      # value type (see table below); required unless sub is set
sub   = "parent"      # inherit from a parent attribute instead of declaring a value type
abstract = true       # mark as abstract

# Annotation constraints (optional, combine as needed)
regex  = "^active|inactive$"
values = ["active", "inactive"]
range  = "0..150"
```

`value` and `sub` are mutually exclusive — an attribute either has a value
type or inherits from a parent, not both.

**Supported value types:**

| TOML value      | TypeDB type  |
| --------------- | ------------ |
| `"string"`      | `string`     |
| `"long"`        | `long`       |
| `"integer"`     | `integer`    |
| `"int"`         | `integer`    |
| `"double"`      | `double`     |
| `"boolean"`     | `boolean`    |
| `"bool"`        | `boolean`    |
| `"datetime"`    | `datetime`   |
| `"datetime-tz"` | `datetime-tz`|
| `"date"`        | `date`       |
| `"duration"`    | `duration`   |
| `"decimal"`     | `decimal`    |

**Example — annotation constraints:**

```toml
[attributes.status]
value  = "string"
values = ["active", "inactive", "archived"]

[attributes.age]
value = "integer"
range = "0..150"

[attributes.email]
value = "string"
regex = "^[^@]+@[^@]+\\.[^@]+$"

[attributes.person-id]
sub = "id"     # inherits from id; no value key
```

### Entities

```toml
[entities.NAME]
sub      = "parent"    # optional; inherit from another entity
abstract = true        # optional; mark as abstract
owns     = [...]       # list of owned attributes (see below)
plays    = [...]       # list of roles the entity plays (see below)
```

**`owns` entries** accept either a plain string (attribute name) or a table
with options:

```toml
owns = [
    "name",                                       # optional, no annotation
    { attribute = "email", key = true },          # @key
    { attribute = "tag",   unique = true },       # @unique
    { attribute = "score", card = "0..100" },     # @card(0..100)
]
```

**`plays` entries** are tables that name the relation and role:

```toml
plays = [
    { relation = "employment", role = "employee" },
    { relation = "friendship", role = "friend"   },
]
```

**Full entity example:**

```toml
[attributes.username]
value = "string"

[attributes.score]
value = "integer"
range = "0..10000"

[attributes.tag]
value  = "string"
values = ["beginner", "intermediate", "expert"]

[entities.user]
owns = [
    { attribute = "username", key = true },
    "score",
    { attribute = "tag", card = "0..5" },
]
plays = [{ relation = "membership", role = "member" }]
```

### Relations

```toml
[relations.NAME]
sub      = "parent"    # optional; inherit from another relation
abstract = true        # optional; mark as abstract
roles    = [...]       # list of role definitions (see below)
owns     = [...]       # list of owned attributes (same syntax as entities)
```

**`roles` entries** define the roles a relation relates. Each role is a table:

```toml
roles = [
    { name = "employer" },                        # no cardinality constraint
    { name = "employee", card = "1..3" },         # @card(1..3)
    { name = "author",   as = "contributor" },    # role override
]
```

`card` sets the `@card` annotation on the role. `as` sets a role override
(`as contributor`).

**Full relation example:**

```toml
[relations.membership]
roles = [
    { name = "member", card = "1.." },
    { name = "group",  card = "1..1" },
]
owns = ["joined-at"]
```

### Functions

```toml
[functions.NAME]
signature = "fun NAME($param: type) -> return-type"
body      = """
  match
    ...;
  return ...;"""
```

The `signature` string is the full TypeQL function signature, without a
trailing colon. The `body` string is verbatim TypeQL passed through to the
transpiler unchanged.

Stream return (curly-brace form):

```toml
[functions.top-scorer]
signature = "fun top-scorer($g: game) -> { player }"
body = """  match
    ($g, $p) isa participation;
  return { $p };"""
```

Scalar return:

```toml
[functions.max-score]
signature = "fun max-score($g: game) -> double"
body = """  match
    $g has score $s;
  return max($s);"""
```

### Structs

```toml
[structs.NAME]
fields = [
    { name = "field-name", type = "string" },
    { name = "optional-field", type = "integer", optional = true },
]
```

Each field entry must have `name` and `type`. Set `optional = true` for
nullable fields (generates `field: T | None = None` in Python).

```toml
[structs.player-stats]
fields = [
    { name = "wins",     type = "integer" },
    { name = "losses",   type = "integer" },
    { name = "nickname", type = "string",  optional = true },
]
```

## Complete Example

The following schema combines every TOML family:

```toml
# Attributes
[attributes.email]
value = "string"
regex = "^[^@]+@[^@]+\\.[^@]+$"

[attributes.username]
value = "string"

[attributes.score]
value    = "integer"
range    = "0..10000"

[attributes.tag]
value  = "string"
values = ["beginner", "intermediate", "expert"]

[attributes.created-at]
value = "datetime"

# Entities
[entities.user]
owns = [
    { attribute = "username",     key  = true },
    "email",
    "score",
    { attribute = "tag",          card = "0..5" },
]
plays = [{ relation = "membership", role = "member" }]

[entities.team]
owns = [
    { attribute = "username", key = true },
    "created-at",
]
plays = [{ relation = "membership", role = "group" }]

# Relations
[relations.membership]
roles = [
    { name = "member", card = "1.."  },
    { name = "group",  card = "1..1" },
]
owns = ["created-at"]

# Functions
[functions.team-members]
signature = "fun team-members($t: team) -> { user }"
body = """  match
    ($t, $u) isa membership;
  return { $u };"""

# Structs
[structs.user-profile]
fields = [
    { name = "display-name", type = "string" },
    { name = "bio",          type = "string", optional = true },
]
```

Generate models from this file:

```python
from type_bridge.generator import generate_models

generate_models("schema.toml", "out/models/")
```

Or from TOML text held in memory:

```python
generate_models(toml_text, "out/models/", format="toml")
```

## Error Reporting

A malformed TOML schema raises `ValueError` with a message that identifies
the offending field or type. Common errors:

| Condition | Example error |
| --------- | ------------- |
| Attribute has both `value` and `sub` | `"Attribute 'id': cannot set both value and sub"` |
| Unknown value type | `"Attribute 'score': unknown value type 'uint'"` |
| Attribute `sub` references an unknown parent | `"Attribute 'child-id': sub parent 'unknown' not defined"` |
| Role player references an unknown type | `"Relation 'review': role 'reviewer' player type not found"` |
| Struct with no fields | `"Struct 'empty-struct': fields list is empty"` |
| Malformed function body | `"Function 'my-fn': body does not contain a return statement"` |

## Limitations

The TOML DSL covers entities that own attributes and play roles in relations,
and relations that relate roles and own attributes. A few TypeQL constructs are
not yet expressible in the TOML front-end; schemas that use them should be
authored in `.tql` directly. Each is a known gap slated for a follow-up:

- **Relation-as-entity** — a relation that itself *plays* roles in other
  relations (common in deeply recursive or type-theoretic models). The TOML
  relation table has no `plays` field.
- **Abstract subtypes** — a type that is both `abstract` and `sub` another
  type. The emitter currently renders only one of the two on entities and
  relations, so mark a subtype abstract in `.tql`.
- **Per-`plays` cardinality** — a `@card(...)` annotation on an entity's
  `plays` entry. TOML `plays` entries carry only the relation and role.

## See Also

- [Code Generator](generator.md) — full `generate_models` API reference and CLI usage
- [Attributes](attributes.md) — attribute types and constraints
- [Entities](entities.md) — entity inheritance and ownership
- [Relations](relations.md) — relations, roles, and role players
