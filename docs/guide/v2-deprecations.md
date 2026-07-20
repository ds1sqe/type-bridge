# V2 Deprecation Inventory

`type-bridge 2.0.0` ships the V2 schema, query, and migration stack
with Rust as the only semantic engine on every V2 path; deprecated V1
facades keep their released engines until they are removed. Every V1
surface stays fully operational throughout every `2.0.x` release. This
page is the exact removal contract: a surface absent from the
"Scheduled for removal" list is not scheduled for removal, and nothing
is removed under a catch-all.

## Scheduled for removal in 2.1.0

### Provider and driver bands

- TypeDB 3.8 and TypeDB 3.10 provider/driver bands. TypeDB 3.12 is the
  supported baseline; the band-9 driver remains.

### TOML schema authoring

- Direct `schema.toml` desired-schema authoring.
- `generate_models(..., format="toml")`.
- Python `.toml` generator auto-routing.

Read-only TOML conversion is **not** removed: the public
`type_bridge_core.toml_to_typeql` converter and its frozen parser are
permanent so existing schemas can always be rendered for migration. No
automated TOML-to-YAML converter ships in 2.0.0.

### V1 schema and model facades

- The V1 `TypeSchema`, `SchemaInfo`, and `SchemaManager` facades.
- Model discovery and model-descriptor construction.
- The fused `Role[T]` declaration form (replaced by split YAML with
  generated `relates()`/`plays()` projections).
- Legacy CRUD declaration facades.

Generated model projections and the V2 `RoleRef` are **not** removed.

### Legacy migration authoring

- Authoring new legacy root `NNNN_*.py` migrations and their sibling
  JSON files.
- The legacy files' role as active migration authority.

Legacy migration **reading** is not removed: readers, original checksum
verification, applied-ledger import, snapshots, historical
TypeDB-version metadata, and the legacy-frontier bridge all remain.

## Deprecated without a removal schedule

### V1 query facades

- The Python V1 `Query` and `QueryBuilder`.
- The Node single-model `TypedQuery<T, Row>` and
  `TypedGroupByQuery<Row>`.
- The Rust V1 `MatchRequest` and its entity, relation, and group-by
  query facades.

These facades are deprecated in intent but are **not** scheduled for
removal in `2.1.0`. No V1 query surface is removed before a complete
V2 replacement exists for its full released algebra with a proven
result-, order-, and diagnostic-parity corpus, announced in a later
deprecation revision with its own notice period. The internal
`MatchRequest`-to-V2 adapter is an incomplete experiment, is not wired
into any execution path, and does not justify any removal.

## Explicitly retained

These surfaces are not deprecated and carry no removal schedule:

- TypeQL import, inspection, generated migrations, and engine-boundary
  support.
- `type_bridge_core.toml_to_typeql` and its frozen TOML parser.
- Legacy Python/JSON migration readers and the legacy-frontier bridge.
- Frozen fixtures that prove conversion fidelity.
- The V2 `/v1` wire and document format versions
  (`typebridge.query-plan/v1`, the remote envelope formats, the
  migration manifest format): the `/v1` suffix names the wire revision
  and is unrelated to product-V1 compatibility.

## Migration paths

| Deprecated surface | Replacement | Converter |
| --- | --- | --- |
| `schema.toml` authoring | Split YAML schema documents | `toml_to_typeql` review output + manual YAML translation |
| Fused `Role[T]` | Split YAML + generated `relates()`/`plays()` | `type-bridge schema generate` projections |
| Python/Rust/Node V1 queries | V2 query plans (prepared, capability-gated) | Manual per-query rewrite (no automated converter yet) |
| Legacy `NNNN_*.py` migrations | Generated migration manifests | `migration adopt` (legacy-frontier bridge + ledger import) |
| TypeDB 3.8/3.10 bands | TypeDB 3.12 baseline | Band upgrade before 2.1.0 |

Archive-only compatibility does not authorize new legacy authoring and
does not restore a second semantic engine. Archival readers are removed
only after positive bridge-adoption evidence, never merely for being
unused during `2.0.x`.
