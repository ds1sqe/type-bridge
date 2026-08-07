# Upgrading to 2.1

TypeBridge 2.1 makes Split YAML and generated bindings the only active schema
and model authoring path. It preserves application operations through generated
Python, TypeScript/Node, and Rust packages, removes the handwritten declaration
surface, and narrows TypeDB support to 3.11 and 3.12.

The release date is not a compatibility deadline. Upgrade when the repository
and your application have passed the generated-package gates below.

## Before changing the dependency

1. Upgrade application-query targets to TypeDB 3.11 or 3.12. A target on which
   TypeBridge will apply, verify, or adopt V2 migrations must be exactly
   TypeDB 3.12.1, the migration and conformance baseline.
2. Express the desired schema as a `typebridge.yaml` workspace plus
   `typebridge.schema-set/v1` and `typebridge.schema/v2` documents.
3. Generate clean Python, TypeScript/Node, and/or Rust packages.
4. Move application imports to those generated packages and run the same CRUD,
   query, transaction, hook, and remote journeys.
5. Adopt any archived root migration history into the canonical V2 chain.

Applications that still require TypeDB 3.8/3.10 or handwritten declaration
classes must remain on `type-bridge>=2,<2.1` until those prerequisites change.

## Generate the application bindings

Validate and generate from the workspace root:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

Generated packages contain their canonical projection evidence and runtime
contract. Do not copy descriptor JSON, rebuild schema meaning from target
language classes, or subclass a compatibility model.

The generated Python single-type path remains concise:

```python
from generated_app import Age, Person, PersonId

ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
people = Person.manager(db).filter(age__gte=18).all()
```

Generated Node and Rust models expose the corresponding model-owned managers,
transactions, filters, reducers, and local/remote query terminals. Interior
double underscores in generated field names remain filterable; an explicit
lookup suffix is resolved without making the field itself unreachable.

See [Schema generation](generator.md), [CRUD](crud.md), and
[Immutable typed queries](typed-queries.md) for binding-specific examples.

## Convert prior schema authority

### TOML

Use `type_bridge_core.toml_to_typeql` to render an existing TOML schema for
review, then translate the reviewed meaning into Split YAML. The converter and
its frozen parser are read-only. Direct `.toml` generation routing and
`generate_models(..., format="toml")` are absent in 2.1.

### Handwritten declarations

Run the 2.0.x application while translating its schema facts into Split YAML,
then compare the generated 2.1 application outcomes before switching. There is
no Python-, Node-, or Rust-declaration-to-YAML writer in 2.1.

The repository parity authority is
`tests/fixtures/generated-only-operation-parity-inventory.json`; it records
which generated acceptance proves each retained operation. Handwritten tests
that were replaced are accounted for by
`tests/fixtures/handwritten-operation-removal-map.json`.

## Adopt archived migration history

Archived root Python/JSON migrations remain readable but cannot be authored or
used as new active authority. Materialize the old history as bounded regular
files, generate its trusted sidecar under the old environment if required, and
adopt it once:

```bash
type-bridge --manifest typebridge.yaml migration adopt \
  --environment production \
  --archive-directory path/to/archived-history
```

Treat adoption as a writer cutover. Quiesce the old migrator and revoke its
writer credential before adoption. The native adopter verifies dependencies,
original checksums, snapshots, metadata, and the applied ledger, then records
the one-way frontier. It never re-executes archived callbacks such as
`RunPython`.

Continue only with canonical migration commands:

```bash
type-bridge --manifest typebridge.yaml migration make --name next-change
type-bridge --manifest typebridge.yaml migration plan
type-bridge --manifest typebridge.yaml migration apply --environment production
type-bridge --manifest typebridge.yaml migration verify --environment production
```

Read-only recovery, ledger import, and replay remain available after the
cutover; they cannot reopen the archived writer lane.

## Retained query facades

The Python `Query`/`QueryBuilder`, Node `TypedQuery`/`TypedGroupByQuery`, and
Rust `MatchRequest` facades have no removal schedule. They remain available for
raw or compatibility queries and are not schema authoring authority.

New generated code can use model-owned direct queries or prepare complete
Query V2 plans through `type_bridge.query_v2`,
`@type-bridge/node/query-v2`, or the Rust SDK. Local and one-exchange remote
paths share canonical plans, validation, result ordering, structured
diagnostics, and concrete-subtype hydration.

## TLS and exact server identity

Plaintext remains the default. A workspace environment can enable TLS and an
optional workspace-confined root:

```yaml
environments:
  production:
    database: example
    uri: typedb.example.internal:1729
    tls: 'true'
    tls-root-ca: certs/production-root.pem
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
```

For gRPC-only deployments, supply an exact retained `server_version` (3.11.x
or 3.12.x). TypeBridge validates it before constructing the corresponding
driver. A root CA never enables TLS implicitly, and transport failures never
retry over plaintext.

## Acceptance checklist

- Split YAML passes `schema check` and regenerates clean packages
  deterministically.
- Generated applications pass the same supported operation journeys as the
  former 2.0.x application.
- No application imports `type_bridge.models`, handwritten Node descriptors,
  or Rust ORM derive/schema-authoring APIs.
- Generated application operations pass on TypeDB 3.11 and/or 3.12; connected
  migration apply/verify/adopt pass on exactly 3.12.1 and reject other versions
  before database mutation.
- Archived migration adoption and a subsequent canonical V2 migration replay
  from empty.
- Wheel, npm package, generated Rust crate, CLI, and server candidates contain
  no removed authoring or provider payload.

The exact removed and retained surfaces are listed in
[V2.1 cutover inventory](v2-deprecations.md).
