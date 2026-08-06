# V2.1 cutover inventory

TypeBridge 2.1 completes the generated-only cutover. Split YAML is the sole
active schema and model authoring authority, and `type-bridge schema generate`
emits the Python, TypeScript/Node, and Rust application bindings. Generated
packages use a projection-owned runtime contract; they do not inherit from or
reconstruct handwritten schema classes.

The cutover is operation-preserving. The machine-checked parity inventory at
`tests/fixtures/generated-only-operation-parity-inventory.json` binds every
retained operation to generated-package evidence in each binding. The
source-to-successor map at
`tests/fixtures/handwritten-operation-removal-map.json` accounts for the
handwritten tests that were replaced.

## Removed in 2.1

### Handwritten schema and model authoring

Python no longer exports handwritten `Attribute`, `Entity`, `Relation`,
`Role`, `TypeDBType`, flag/cardinality, descriptor, registry, scanner, or
manager declaration APIs. The defining `type_bridge.attribute`,
`type_bridge.fields`, and `type_bridge.models` barrels have no authoring
exports. The original `TypeSchema`, `SchemaInfo`, and `SchemaManager` public
facades are also absent.

Node no longer exports descriptor factories, handwritten entity/relation
builders, dynamic schema registries, or their declaration types. Rust no
longer exposes the handwritten ORM traits, derive macros, descriptor registry,
schema manager, `include_schema!`, or programmatic TypeQL model generator.

The replacement is always canonical generation:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

Application values remain concise and strongly typed. For example, generated
Python bindings retain the single-type manager journey:

```python
ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
people = Person.manager(db).filter(age__gte=18).all()
```

Generated Node and Rust packages expose equivalent model-owned managers and
query terminals.

### Retired provider support

TypeDB 3.8 and 3.10 support and their native provider packages are absent.
TypeBridge 2.1 supports TypeDB 3.11 and 3.12 through bands 8 and 9. An older
known server or driver fails the version gate before data work. No compatibility
warning class, Node warning code, or Rust warning event is retained in 2.1.

Applications that cannot upgrade their TypeDB server must remain on a 2.0.x
TypeBridge release. Read the current support matrix in
[TypeDB integration](../development/typedb.md#server-and-driver-compatibility).

### TOML and programmatic generator authoring

Direct `schema.toml` desired-schema routing,
`generate_models(..., format="toml")`, `.toml` generator auto-routing, and the
old programmatic TypeQL-to-model generator are absent. Split YAML replaces all
of them.

Read-only TOML conversion remains available through
`type_bridge_core.toml_to_typeql`. Its frozen parser renders existing TOML for
review and migration but cannot make TOML active schema authority.

### Archived migration authoring

TypeBridge no longer creates new root `NNNN_*.py`/JSON migration histories or
treats them as active authority. Canonical V2 `migration make`, `plan`,
`apply`, `verify`, and `adopt` remain the writer path.

Readers for frozen histories, original checksum verification, applied-ledger
import, snapshots, metadata, and the one-way frontier bridge remain read-only.
They can recover and adopt old history; they cannot reopen its writer lane.

## Retained query contracts

The low-level Query V2 APIs and generated model-oriented direct and remote
facades remain public. The following V1 query contracts also remain because
they have no removal schedule:

- Python `Query` and `QueryBuilder`;
- Node `TypedQuery<T, Row>` and `TypedGroupByQuery<Row>`;
- Rust `MatchRequest` and its entity, relation, and group-by query facades.

They may delegate to the shared Rust semantic engine, but they are not schema
authoring authority and are not part of this removal.

## Explicitly retained archival and wire surfaces

- `type_bridge_core.toml_to_typeql` and its frozen TOML parser;
- read-only archived migration loading, verification, adoption, snapshots,
  metadata, and ledger import;
- frozen compatibility fixtures required to prove recovery;
- Query V2 wire identifiers ending in `/v1`, where `/v1` identifies the wire
  format revision rather than a handwritten product API.

Archive-only compatibility does not authorize new authoring. Any future
removal requires a separately enumerated contract and acceptance evidence.
