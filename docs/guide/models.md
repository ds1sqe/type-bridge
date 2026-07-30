# Model TypeDB data

TypeBridge models TypeDB concepts directly: attributes are independent types,
entities and relations own attributes, relations declare roles, and player
types play those roles.

## Build a model

1. [Define attribute types](attributes.md) and their value domains.
2. [Define entities](entities.md), ownership, and inheritance.
3. [Define relations and roles](relations.md).
4. Express [cardinality, keys, uniqueness, ordering, and metadata](cardinality.md).
5. Use [abstract types](abstract-types.md) for polymorphic contracts.
6. Apply [validation and serialization](validation.md) at the application
   boundary.

Python models are Pydantic classes. The TypeScript facade uses branded
attribute and model constructors. Schema-first workspaces can generate both
surfaces plus a Rust schema crate from the same authority.

## Application projections

- [API DTOs](dto.md) generate create, patch, and output shapes without exposing
  persistence-only fields.
- [Code generation](generator.md) projects TypeQL or canonical workspace
  schemas into application-owned model packages.
- [TypeScript/Node](typescript.md) explains branded attribute values and the
  JavaScript `bigint` boundary.
- [Rust](rust.md) explains generated create/model types and schema binding.

Once the model is defined, continue with [read and write workflows](data.md).
