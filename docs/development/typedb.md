# TypeDB integration

## Server and driver compatibility

TypeBridge 2.1 supports TypeDB 3.11.x–3.12.x. The live release matrix uses
3.11.5 and 3.12.1, and 3.12.1 is the V2 conformance baseline. Generated Python,
TypeScript/Node, and Rust applications share this exact window.

Generated live acceptance executes the same application operation set on both
verified tags. Its 3.11 fixture omits only plays-side `@doc` annotations, which
are unavailable before 3.12; an offline source guard pins that as the complete
difference from the 3.12 fixture.

| Dimension | Supported value |
| --- | --- |
| TypeDB servers | 3.11.x–3.12.x |
| Verified tags | 3.11.5, 3.12.1 |
| Generated CRUD/query acceptance | 3.11.5 and 3.12.1 |
| Connected V2 migration execution | exactly 3.12.1 |
| Protocol providers | band 8 and band 9 |
| V2 conformance baseline | 3.12.1 |
| CPython | 3.12–3.14 |
| Node | 18+; primary lane 20 |
| Public Rust SDK | Rust 1.88+ |

Versions outside the window fail before TypeBridge opens an application data
transaction.

The wider window applies to generated application operations and query
execution. `migration apply`, `migration verify`, and `migration adopt` have a
narrower authority contract: both the workspace semantic profile and the
negotiated server must be exactly 3.12.1. They reject before database creation
or migration state work otherwise. Offline `schema check`, `schema generate`,
`migration make`, and `migration plan` remain available for either supported
semantic profile.

### Protocol providers

A TypeDB driver speaks one protocol band. Server acceptance is asymmetric:

| Provider | Driver line | Accepted servers |
| --- | --- | --- |
| band 8 | 3.11 | 3.11, 3.12 |
| band 9 | 3.12 | 3.12 |

The native runtime embeds the source-unmodified namespaced 3.11.5 provider and
uses the official 3.12.1 driver. Band 8 is the safe unknown-server discovery
path; a confirmed 3.12 server upgrades to band 9. No application-side provider
selection is required.

The Python `typedb-driver` extra is independent of generated ORM execution. It
exists for direct driver calls. CPython 3.14 uses driver 3.12.1 and therefore
requires a 3.12 server for those direct calls; the embedded TypeBridge runtime
still supports both server lines.

### Version discovery

By default, `Database.connect()` probes `GET /v1/version` on the TypeDB HTTP
port (default `8000`) and then opens the matching native provider. Configure the
HTTP port when it is remapped:

```python
from type_bridge import Database

db = Database(
    address="localhost:1729",
    database="application",
    http_port=9000,
)
db.connect()
```

For gRPC-only deployments, supply an exact supported server version:

```python
db = Database(
    address="localhost:1729",
    database="application",
    server_version="3.11.5",
)
db.connect()
```

An explicit version skips HTTP discovery, validates the semantic version, and
selects the matching provider. Raw band numbers, malformed versions, and
unsupported versions are rejected before provider work.

Node exposes the same options:

```ts
import { RustDatabase } from "@type-bridge/node";

const db = RustDatabase.connect("localhost:1729", "application", {
  httpPort: 9000,
  serverVersion: "3.12.1",
});
```

### Feature gates

The support window determines whether TypeBridge can connect; individual
features may have a higher minimum inside that window.

| Feature | Minimum server | Behavior on 3.11 |
| --- | --- | --- |
| `@doc` / `@meta` schema annotations | 3.12.0 | rejected with a versioned client diagnostic before migration/data work |
| `given` rows | 3.12.0 | generated queries use validated bounded literal lowering |

Feature gates live in the Rust version authority and are consumed by all
bindings. A too-old server produces the same actionable TypeBridge diagnostic
instead of a server-side syntax failure.

## Lifecycle

Binding-owned database handles close the embedded driver explicitly. Leases
held by active transactions keep their provider alive until terminal cleanup;
once explicit close begins, new operations fail and in-flight work is
cancelled. Close is idempotent.

Node synchronous native work occupies the JavaScript event-loop thread, so
`RustDatabase.close()` cannot interrupt a synchronous call out of band.
Cancellable Node V2 work uses Promise-returning execution plus a deadline. The
explicit shutdown behavior tracks the upstream 3.12.1 lifecycle issue in #196
and does not patch the packaged TypeDB driver.

## TypeDB concepts preserved by generated projections

Split-YAML maps directly to TypeDB concepts:

- attributes declare scalar domains, inheritance, independence, and value
  constraints;
- entities and relations declare ownership and cardinality;
- relations declare roles and role cardinality;
- `plays` declares the exact entity/relation player contract;
- subtype facts preserve inherited ownership and roles;
- keys, uniqueness, ordering, distinctness, documentation, and metadata remain
  explicit schema facts.

The generator projects those facts into exact wrapper/model/reference and
field/role-token types. It does not infer schema from application classes.

Example Split-YAML:

```yaml
format: typebridge.schema/v2

attributes:
  person-id: {value: string}
  age:
    value:
      type: integer
      range: {min: 0, max: 150}

entities:
  person:
    owns:
      person-id: {key: true}
      age: {card: {min: 0, max: 1}}
```

The generated Python projection is then data-only:

```python
from app_models import Age, Person, PersonId

ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
```

## TypeQL lowering rules

The Rust engine owns TypeQL emission and hydration. Binding code must not build
query strings by interpolating generated values.

Important invariants:

- every schema/data statement uses exact resolved labels;
- string, regex, temporal, decimal, duration, and binary values use canonical
  escaping/encoding;
- optional ownership absence is distinct from scalar null (TypeDB has no null
  attribute instance);
- multivalue ownership, repeated role players, and missing fields retain their
  declared shape;
- updates and deletes identify instances by IID or complete generated key
  evidence;
- query variables, fields, and roles retain owner/session identity;
- selected result shape is validated before hydration.

Raw TypeQL remains available through separately retained query/execution
facades, but it does not become model authority and cannot install a generated
projection.

## Transactions

Generated managers accept either a database or an existing transaction:

```python
with db.transaction("write") as tx:
    Person.manager(tx).put(ada)
    Employment.manager(tx).insert(employment)
```

A database argument creates an operation-owned transaction. A transaction
argument keeps the full sequence atomic. Python direct-driver transactions are
separate from TypeBridge's Rust-owned handles.

## TLS and credentials

Workspace credentials are environment references, never committed literals.
TLS configuration is validated as one closed transport contract: system roots,
custom roots, domain override, and insecure/plaintext choices cannot be mixed
contradictorily. The same normalized contract is exercised across Python,
Node, Rust, CLI, and server paths.

## Testing provider behavior

The full suite manages an isolated 3.12.1 server by default:

```bash
./test.sh
```

Use a retained existing server deliberately:

```bash
USE_DOCKER=false TYPEDB_HTTP_PORT=9000 ./test.sh --no-isolated
```

Focused version/provider tests live in `crates/core`, `crates/typedb-runtime`,
binding session tests, generated live acceptance, and the release identity
validators. Positive live support covers only 3.11/3.12; older versions appear
only as rejection evidence.

## Maintainer checklist

When changing TypeDB integration:

1. Update the Rust version/feature authority first.
2. Preserve fail-closed discovery before data work.
3. Exercise both retained providers and every binding that advertises the path.
4. Regenerate native notices when dependencies change.
5. Validate exact wheel, npm, Cargo, and container artifacts.
6. Keep the public support table, workflow matrix, and runtime diagnostics
   coherent.
