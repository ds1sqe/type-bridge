# Model TypeDB data

Split-YAML is the schema and model-authoring authority. Generated Python,
TypeScript/Node, and Rust packages project that authority into idiomatic value,
entity, relation, reference, field-token, and role-token types.

## Build a model

1. Declare [attribute types](attributes.md) and scalar constraints.
2. Declare [entities](entities.md), ownership, keys, and inheritance.
3. Declare [relations](relations.md), roles, and allowed players.
4. Express [cardinality, uniqueness, ordering, and metadata](cardinality.md).
5. Use [abstract types](abstract-types.md) for polymorphic query boundaries.
6. Run `type-bridge schema check`, create/review a migration, then run
   `type-bridge schema generate`.

Generated files are projections, not alternate schema authorities. Do not edit
or subclass their package-internal bases. Change Split-YAML and regenerate.

## Application values

Each generated attribute class wraps the target-language scalar for one schema
attribute. Generated entity and relation constructors accept only the exact
projected fields and role players declared by the workspace. Reference types
carry an IID plus the key material needed by relation writes.

The generated packages deliberately differ at language boundaries: Python uses
keyword constructors, TypeScript uses `create(...)` and `bigint` for integer
values, and Rust separates create values from hydrated models. They preserve the
same labels, cardinalities, roles, keys, and query outcomes.

## Continue

- [Generated CRUD and transactions](crud.md)
- [Immutable generated queries](typed-queries.md)
- [Workspace generation](generator.md)
- [Split-YAML reference](split-yaml-v1.md)
