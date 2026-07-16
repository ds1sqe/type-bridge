# V2 Deprecation Inventory

`type-bridge 2.0.0` ships the complete V2 schema, query, and migration
stack with Rust as the only semantic engine. Every V1 surface listed
below stays fully operational throughout every `2.0.x` release and is
**scheduled for removal in `2.1.0`**. This page is the exact removal
contract: a surface absent from the removal list is not scheduled for
removal, and nothing is removed under a catch-all.

## Scheduled for removal in 2.1.0

### Provider and driver bands

- TypeDB 3.8 and TypeDB 3.10 provider/driver bands. TypeDB 3.12 is the
  supported baseline; the band-9 driver remains.

### TOML schema authoring

- Direct `schema.toml` desired-schema authoring.
- `generate_models(..., format="toml")`.
- Python `.toml` generator auto-routing.
- The public TOML transpiler entry point.

Read-only TOML conversion is **not** removed: the frozen parser and the
one-way TOML-to-YAML converter remain so existing schemas can migrate.

### V1 schema and model facades

- The V1 `TypeSchema`, `SchemaInfo`, and `SchemaManager` facades.
- Model discovery and model-descriptor construction.
- The fused `Role[T]` declaration form (replaced by split YAML with
  generated `relates()`/`plays()` projections).
- Legacy CRUD declaration facades.

Generated model projections and the V2 `RoleRef` are **not** removed.

### V1 query facades

- The Python V1 `Query` and `QueryBuilder`.
- The Node single-model `TypedQuery<T, Row>` and
  `TypedGroupByQuery<Row>`.
- The Rust V1 `MatchRequest` and its entity, relation, and group-by
  query facades.

Each is removed only where its complete V2 replacement exists; the V1
adapter that lowers `MatchRequest` onto V2 plans (with the proven
result-parity corpus) is the documented migration path.

### Legacy migration authoring

- Authoring new legacy root `NNNN_*.py` migrations and their sibling
  JSON files.
- The legacy files' role as active migration authority.

Legacy migration **reading** is not removed: readers, original checksum
verification, applied-ledger import, snapshots, historical
TypeDB-version metadata, and the legacy-frontier bridge all remain.

## Explicitly retained

These surfaces are not deprecated and carry no removal schedule:

- TypeQL import, inspection, generated migrations, and engine-boundary
  support.
- Read-only TOML/schema converters and their frozen parser.
- Legacy Python/JSON migration readers and the legacy-frontier bridge.
- Frozen fixtures that prove conversion fidelity.
- The V2 `/v1` wire and document format versions
  (`typebridge.query-plan/v1`, the remote envelope formats, the
  migration manifest format): the `/v1` suffix names the wire revision
  and is unrelated to product-V1 compatibility.

## Migration paths

| Deprecated surface | Replacement | Converter |
| --- | --- | --- |
| `schema.toml` authoring | Split YAML schema documents | Read-only TOML-to-YAML converter |
| Fused `Role[T]` | Split YAML + generated `relates()`/`plays()` | Schema converter output |
| Python/Rust/Node V1 queries | V2 query plans (prepared, capability-gated) | V1 `MatchRequest` adapter onto V2 plans |
| Legacy `NNNN_*.py` migrations | Generated migration manifests | Legacy-frontier bridge + ledger import |
| TypeDB 3.8/3.10 bands | TypeDB 3.12 baseline | Band upgrade before 2.1.0 |

Archive-only compatibility does not authorize new legacy authoring and
does not restore a second semantic engine. Archival readers are removed
only after positive bridge-adoption evidence, never merely for being
unused during `2.0.x`.
