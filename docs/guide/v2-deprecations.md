# V2 Deprecation Inventory

`type-bridge 2.0.0` ships the V2 schema, query, and migration stack
with Rust as the only semantic engine on every V2 path; each deprecated V1
facade keeps its released behavior and public contract until it is removed.
Its implementation may delegate to a parity-proven Rust V2 bridge, as the V1
`MatchRequest` path does, without scheduling the facade for removal. Every V1
surface stays fully operational throughout 2.0.x — and every surface without
a removal schedule throughout the whole 2.x line — unless an individual
migration scope explicitly completes the irreversible V2 adoption
cutover. That cutover closes only the adopted scope's legacy writer lane and
is not a package-wide removal. This page is the exact removal contract: a
surface absent from the "Scheduled for removal" list is not scheduled for
removal, and nothing is removed under a catch-all.

The removal schedule names a single release. Every scheduled removal lands
in the 2.1.0 minor release as a deliberate, exactly-enumerated exception to
ordinary major-version scheduling: active TypeDB 3.8/3.10 provider support
(the wheel then embeds only the band-8 and band-9 driver lines), direct TOML
desired-schema authoring, the V1 schema and model facades, and legacy
migration authoring. There is no 3.0.0 removal plan; a surface not scheduled
for 2.1.0 is not scheduled at all.

## Scheduled for removal in 2.1.0

### Provider and driver bands

- TypeDB 3.8 and TypeDB 3.10 provider/driver bands. TypeDB 3.11 support and
  its band-8 compatibility package remain; TypeDB 3.12.1 is the conformance
  baseline and the official upstream band-9 driver remains.

Starting with 2.1.0 the wheel embeds only the band-8 and band-9 driver
lines, so active 3.8/3.10 support ends in that minor release. Deployments
that must retain those server lines pin `type-bridge>=2,<2.1` or upgrade the
server to TypeDB 3.11 or 3.12 before moving past 2.0.x.

### TOML schema authoring

- Direct `schema.toml` desired-schema authoring.
- `generate_models(..., format="toml")`.
- Python `.toml` generator auto-routing.

TOML desired-schema authoring is scheduled for removal in the 2.1.0 minor
release. Deployments that still author TOML must either translate
to split YAML before upgrading past 2.0.x or pin `type-bridge>=2,<2.1`.

Both deprecated authoring routes — `format="toml"` and `.toml`
auto-routing — emit one standard Python `DeprecationWarning` per
`generate_models` call. Filter it with the ordinary
`warnings.filterwarnings("ignore", category=DeprecationWarning)` policy;
the warning does not change generation behavior.

Read-only TOML conversion is **not** removed: the public
`type_bridge_core.toml_to_typeql` converter and its frozen parser are
permanent so existing schemas can always be rendered for migration, and
calling `toml_to_typeql` directly emits no deprecation warning. No
automated TOML-to-YAML converter ships in 2.0.0.

Under the default warning policy, every successful 3.8/3.10 connection emits
exactly one compatibility notice and then continues normally. Strict warning
policies may suppress the public binding warning but cannot change the
connection outcome. An unknown-version connection emits the conservative
notice only when it actually negotiated the legacy band-7 fallback; missing
HTTP identity alone is not legacy evidence. Unknown band-8/band-9 and known
3.11/3.12 connections do not emit this deprecation code.

| Connection path | Server identity | Notice behavior |
| --- | --- | --- |
| External Python `typedb-driver` | Known 3.8/3.10 | One Python warning after the driver connects successfully. |
| Embedded Rust runtime, including Python and Node wrappers | Known 3.8/3.10 | One Rust tracing notice; the binding also emits its filterable public warning. |
| Embedded gRPC band-7 legacy fallback | Exact version unavailable | One conservative notice that does not claim a server version. |
| Embedded gRPC current band (8 or 9) | Exact version unavailable | No legacy-server notice. |
| Any path | Known 3.11/3.12 | No legacy-server notice, including a known 3.12 server that retains the compatible discovery connection. |

- Python exports `TypeDBServerDeprecationWarning`, a `FutureWarning` subclass.
  Filter it by class rather than matching prose:

  ```python
  import warnings

  from type_bridge import TypeDBServerDeprecationWarning

  warnings.filterwarnings("ignore", category=TypeDBServerDeprecationWarning)
  ```

- Node calls `process.emitWarning` with standard warning type
  `DeprecationWarning` and code
  `TYPE_BRIDGE_TYPEDB_LEGACY_SERVER`. Applications may inspect the `warning`
  event's `name` and `code`; Node's standard `--no-deprecation` policy also
  suppresses it.
- Rust emits one tracing event with code
  `TYPE_BRIDGE_TYPEDB_LEGACY_SERVER`.

The code and prose are owned by the Rust compatibility engine. Python aliases
the native code on its warning class, and the Node package checks its public
filter constant against the real addon's Rust export. The prose deliberately
contains no internal protocol-band number. These notices do not change
connection, query, or teardown behavior under any warning policy. Notice
lookup and delivery are best-effort: a Python filter that promotes
`TypeDBServerDeprecationWarning` to an error is caught after the delivery
attempt, a synchronous replacement of Node's `process.emitWarning` that throws
is caught, and Node `--throw-deprecation` suppresses this notice instead of
turning a successful connection into an exception. An exception raised later
by an application's asynchronous `warning` event listener remains
application-owned, as it does for every Node warning; `connect()` has already
returned and cannot contain it. The ordinary Python ignore filter and Node
`--no-deprecation` remain supported. Applications that must retain 3.8/3.10
support may pin
`type-bridge>=2,<2.1`.

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
Deployments that retain these facades past 2.0.x pin `type-bridge>=2,<2.1`.

### Legacy migration authoring

- Authoring new legacy root `NNNN_*.py` migrations and their sibling
  JSON files.
- The legacy files' role as active migration authority.

Legacy migration **reading** is not removed: readers, original checksum
verification, applied-ledger import, snapshots, historical
TypeDB-version metadata, and the legacy-frontier bridge all remain.
Before 2.1.0, an unadopted scope may keep using the released writer. Completing
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
removal. No V1 query surface is removed before a complete
V2 replacement exists for its full released algebra with a proven
result-, order-, and diagnostic-parity corpus, announced in a later
deprecation revision with its own notice period. The production Rust
`MatchRequest` bridge already lowers the complete released request algebra
through the shared V2 builder and same-snapshot model executor, retaining a
direct V1 fallback only when the exact representation exceeds V2's fixed
artifact-size envelope. That internal implementation change does not schedule
or justify removal of any V1 facade. Complete low-level V2 authoring is
available from `type_bridge.query_v2` and `@type-bridge/node/query-v2`, and the
model-oriented remote facade is available as `RemoteQuerySession` /
`RemoteQuery` under the typed subpaths. These additive replacements let an
application migrate one query at a time; their availability is not a removal
notice.

## Explicitly retained

These surfaces are not deprecated and carry no removal schedule:

- TypeQL import, inspection, generated migrations, and engine-boundary
  support.
- `type_bridge_core.toml_to_typeql` and its frozen TOML parser.
- Legacy Python/JSON migration readers and the legacy-frontier bridge.
- Frozen fixtures that prove conversion fidelity, canonical bytes,
  fingerprints, hostile-envelope rejection, V1 wire stability, or released
  compatibility. Only the former embedded `PLAN_B64` constants in the two V2
  binding execution smokes were retired after those smokes began authoring the
  equivalent plans through the public facades.
- The V2 `/v1` wire and document format versions
  (`typebridge.query-plan/v1`, the remote envelope formats, the
  migration manifest format): the `/v1` suffix names the wire revision
  and is unrelated to product-V1 compatibility.

## Migration paths

| Deprecated surface | Replacement | Converter |
| --- | --- | --- |
| `schema.toml` authoring | Split YAML schema documents | `toml_to_typeql` review output + manual YAML translation |
| Fused `Role[T]` | Split YAML + generated `relates()`/`plays()` | `type-bridge schema generate` projections |
| Python/Rust/Node V1 queries | Low-level V2 plans (`type_bridge.query_v2`, `@type-bridge/node/query-v2`) or model-oriented direct/remote typed sessions | Manual per-query rewrite (no automated converter yet; unrelated V1 queries stay operational) |
| Legacy `NNNN_*.py` migrations | Generated migration manifests | `migration adopt` (legacy-frontier bridge + ledger import) |
| TypeDB 3.8/3.10 bands | TypeDB 3.11 or 3.12 | Band upgrade before 2.1.0 |

Archive-only compatibility does not authorize new legacy authoring and does
not restore a second semantic engine. No archival reader is scheduled for
removal; any future removal requires a separately versioned,
fully enumerated contract and its own notice period, in addition to positive
bridge-adoption evidence.
