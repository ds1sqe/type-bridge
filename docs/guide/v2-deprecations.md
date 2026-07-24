# V2 Deprecation Inventory

`type-bridge 2.0.0` ships the V2 schema, query, and migration stack
with Rust as the only semantic engine on every V2 path; each deprecated V1
facade keeps its released engine until it is removed. Every V1 surface
stays fully operational throughout every 2.x release unless an individual
migration scope explicitly completes the irreversible V2 adoption cutover.
That cutover closes only the adopted scope's legacy writer lane and is not a
package-wide removal. This page is the exact removal contract: a surface absent from the
"Scheduled for removal" list is not scheduled for removal, and nothing
is removed under a catch-all.

## Scheduled for removal in 3.0.0

### Provider and driver bands

- TypeDB 3.8 and TypeDB 3.10 provider/driver bands. TypeDB 3.11 support and
  its band-8 compatibility package remain; TypeDB 3.12.1 is the conformance
  baseline and the official upstream band-9 driver remains.

### TOML schema authoring

- Direct `schema.toml` desired-schema authoring.
- `generate_models(..., format="toml")`.
- Python `.toml` generator auto-routing.

Read-only TOML conversion is **not** removed: the public
`type_bridge_core.toml_to_typeql` converter and its frozen parser are
permanent so existing schemas can always be rendered for migration. No
automated TOML-to-YAML converter ships in 2.0.0.

### V1 schema and model facades

- `type_bridge_core.TypeSchema` and
  `type_bridge_core_lib::schema::TypeSchema`.
- `type_bridge.SchemaInfo`, `type_bridge.migration.SchemaInfo`, and
  `type_bridge_orm::SchemaInfo`.
- `type_bridge.SchemaManager`, `type_bridge.migration.SchemaManager`, and
  `type_bridge_orm::SchemaManager`.
- The fused declaration class exported as `type_bridge.Role` and
  `type_bridge.models.Role` when used as `Role[T]` (replaced by split YAML
  with generated `relates()`/`plays()` projections).

This list deliberately does **not** schedule `TypeDBType`, `Entity`,
`Relation`, attribute/flag declarations, model registry or scanner symbols,
CRUD managers/queries/hooks/exceptions, generated model projections, or the
V2 `RoleRef`. Removing any of those would require a later inventory that names
the fully qualified public symbols and gives them a new notice period.

### Legacy migration authoring

- Authoring new legacy root `NNNN_*.py` migrations and their sibling
  JSON files.
- The legacy files' role as active migration authority.

Legacy migration **reading** is not removed: readers, original checksum
verification, applied-ledger import, snapshots, historical
TypeDB-version metadata, and the legacy-frontier bridge all remain.
Before 3.0, an unadopted scope may keep using the released writer. Completing
`migration adopt` is a deliberate, per-scope, irreversible opt-in that writes
the ledger cutover marker and closes that scope's writer lane immediately;
quiescence and revocation of the old writer credential are required during the
cutover. Released 1.5.x binaries, including the old `SchemaManager` and
`SimpleMigrationManager`, do not understand the managed anchor and must be
treated as fence-unaware. The current 2.x facades reject an exact anchor-bound
cutover before mutation, but that protection does not make old credentials safe
to retain.

## Deprecated without a removal schedule

### Legacy migration convenience manager

- `type_bridge.MigrationManager` and
  `type_bridge.migration.SimpleMigrationManager` (aliases of the same
  released class).

These convenience aliases remain operational on unadopted scopes throughout
2.x. Their warning names the supported replacements but does not announce a
removal version. The current 2.x implementation rejects their writes after an
exact per-scope V2 cutover; released 1.5.x copies remain fence-unaware and are
excluded through credential revocation during adoption.

### V1 query facades

- The Python V1 `Query` and `QueryBuilder`.
- The Node single-model `TypedQuery<T, Row>` and
  `TypedGroupByQuery<Row>`.
- The Rust V1 `MatchRequest` and its entity, relation, and group-by
  query facades.

These facades are deprecated in intent but are **not** scheduled for
removal in `3.0.0`. No V1 query surface is removed before a complete
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
| TypeDB 3.8/3.10 bands | TypeDB 3.11 or 3.12 | Band upgrade before 3.0.0 |

Archive-only compatibility does not authorize new legacy authoring and does
not restore a second semantic engine. No archival reader is scheduled for
removal in `3.0.0`; any future removal requires a separately versioned,
fully enumerated contract and its own notice period, in addition to positive
bridge-adoption evidence.
