# Split-YAML and Workspace V1 Reference

This page is the versioned authoring contract for the first TypeBridge V2
workspace. Its three wire discriminators are versioned independently:

- schema-set manifest: `typebridge.schema-set/v1`
- schema fragment: `typebridge.schema/v2`
- workspace manifest: `typebridge.workspace/v1`

Unknown keys, unsupported future shapes, duplicate set members, and duplicate
facts fail closed. Rust parses and validates these files directly; generated
TypeQL and language bindings are projections, not alternate schema authorities.

All three formats use the strict TypeBridge YAML subset: UTF-8, one document,
string mapping keys, no duplicate keys, and no tags, directives, anchors,
aliases, merge keys, environment interpolation, timestamps, or YAML 1.1
Boolean coercions. Document size, nesting, scalar length, and collection sizes
are bounded. Comments and exact source spans are retained, but comments do not
change declared schema meaning.

## Frozen contract

The following 24 decisions define this version. A later format may add syntax,
but these wires never parse a future key and ignore it.

| # | Surface | V1/V2 contract |
| --- | --- | --- |
| 1 | Fragment discriminator | Exactly `typebridge.schema/v2`. |
| 2 | `sub` | A supertype scalar or closed `{type, doc, meta}` mapping. |
| 3 | Fragment `sources` | Rejected. Only the schema-set manifest discovers fragments. |
| 4 | Sequence named facts | Labels and singleton expanded mappings are accepted. |
| 5 | Bare-null named body | Rejected; use an explicit `{}` body. |
| 6 | Fact-local extensions | Rejected and deferred to a later document format. |
| 7 | Schema extensions | Dot-namespaced capability ID with requirement-only `{required}`; no payload or data. |
| 8 | Structs | Flat, ordered fields with built-in value types; no nesting or user-defined field types. |
| 9 | Semantic profile | Exactly `typedb-3.11.5/v1` or `typedb-3.12.1/v1`. |
| 10 | Migration directory | A workspace path ending directly in `v2`, such as `migrations/v2`. |
| 11 | Schema lockfile key | Unsupported. Workspace V1 owns its fixed lock internally. |
| 12 | TypeDB version range | Unsupported. The exact semantic profile is authoritative. |
| 13 | Migration policies | Only `directory`, `app-label`, and optional `destructive`. |
| 14 | Binding mode | Fixed by target key; each binding body contains only `output`. |
| 15 | Binding customization | Rejected and deferred to a later workspace format. |
| 16 | Environment URI | One or more comma-separated `host:port` or `[IPv6]:port` endpoints. No scheme, userinfo, path, query, whitespace, empty member, missing port, port 0, or port above 65535. |
| 17 | Database | A nonempty string scalar. |
| 18 | Credentials | Separate `username: env:VAR` and `password: env:VAR` references. |
| 19 | Credential providers | Environment-variable references only. |
| 20 | Environment requirements | One flat, duplicate-free capability list. |
| 21 | Environment HTTP port | Optional canonical `u16` in `http-port`. |
| 22 | Top-level secrets | Namespaced `{env: VARIABLE}` slots, retained for explicit consumers. |
| 23 | Workspace extensions | Exact `{version}` handler requirements; no data payload. |
| 24 | Server authority output | Optional exact `artifacts.schema-authority.output`; one confined portable generated file whose path ends in lowercase `.json`. |

Workspace V1 additionally has the implemented `tls` and `tls-root-ca` keys.
They extend transport policy without changing any decision above.

## Files and ownership

```text
typebridge.yaml                         workspace and generation policy
schema/schema.yaml                     schema-set source discovery
schema/**/*.yaml                       portable schema fragments
migrations/v2/*.tbmigration.json       immutable transition authority
migrations/v2/*.typeql                 generated review rendering
generated/schema-authority.json        generated source-free server authority
environment variables                  deployment credentials
database ledger                        applied transition state
```

Paths in `typebridge.yaml` are relative to that manifest. Patterns in the
schema-set are relative to the schema-set manifest. Fragments cannot include
other fragments.

The server artifact is configured only through this exact shape:

```yaml
artifacts:
  schema-authority:
    output: generated/schema-authority.json
```

Its path is confined and cannot overlap schema sources, migrations, binding
outputs, or custom trust material. `schema generate` produces it from the same
captured workspace as every binding. The canonical JSON bytes are compiled
deployment evidence, not a file users maintain or feed back into schema
resolution.

## Schema-set manifest

The schema-set manifest contains only its discriminator and `sources`:

```yaml
format: typebridge.schema-set/v1
sources:
  - attributes/*.yaml
  - entities/**/*.yaml
  - relations/*.yaml
```

Each explicit path or glob must select at least one regular lowercase `.yaml`
file. `/` is the portable separator; `*` and `?` stay within one segment, and a
segment equal to `**` crosses segments. Absolute paths, `..`, backslashes,
classes, braces, escapes, duplicate selections, symlink escapes, and selection
of the manifest itself are errors. The final canonical relative paths are
NFC-normalized, collision-checked, and byte-sorted before loading.

The ordered `sources` pattern list controls discovery and is therefore
order-sensitive input. The discovered document set and normalized schema facts
are deterministic and set-like; source order never changes schema meaning.

## Schema fragments

Every fragment begins with:

```yaml
format: typebridge.schema/v2
```

The remaining root keys are closed: `capabilities`, `attributes`, `entities`,
`relations`, `plays`, `functions`, `structs`, and `extensions`. A type or
function has one primary declaration. `plays` is the only detached
cross-fragment statement family; it references declarations and never reopens
or creates them. Explicit capability requirements use one duplicate-free list:

```yaml
capabilities:
  required: [schema.roles, schema.annotations]
```

Named facts share a compact/expanded rule:

```text
NamedFacts<Body> = [label | {label: Body}, ...] | {label: Body, ...}
plays             = player -> relation -> NamedFacts<PlaysBody>
value             = value-type | {type: value-type, <ValueBody>}
sub               = supertype | {type: supertype, <SubBody>}
```

In a sibling mapping, every fact needs a mapping body. For example,
`owns: {name:}` is invalid and `owns: {name: {}}` is valid. Compact labels and
empty expanded bodies have the same declared meaning.

Annotations are typed keys on the exact fact they modify. YAML omits the
TypeQL `@` sigil. The supported keys for both accepted semantic profiles are:

| Subject | Keys |
| --- | --- |
| entity type | `abstract`, `doc`, `meta` |
| relation type | `abstract`, `doc`, `meta` |
| attribute type | `abstract`, `independent`, `doc`, `meta` |
| `sub` | `doc`, `meta` |
| attribute `value` | `regex`, `range`, `values` |
| `owns` | `unique`, `key`, `card`, `regex`, `range`, `values`, `doc`, `meta` |
| `relates` | `abstract`, `card`, `doc`, `meta` |
| `plays` | `card`, `doc`, `meta` |
| function | `doc`, `meta` |

Presence-only annotations accept only `true`. `card: N` means exact cardinality;
the structured form requires `min` and may omit `max` for an unbounded maximum.
`meta` is a string-keyed map of string values. Unknown or inapplicable keys are
errors.

Schema extensions declare capability requirements only:

```yaml
extensions:
  com.example.codegen:
    required: true
```

The ID uses the bounded lowercase dot-namespaced capability spelling.
`required` defaults to false. `payload`, `data`, and fact-local `extensions`
are invalid.

Flat built-in structs use ordered fields:

```yaml
structs:
  player-stats:
    fields:
      - name: wins
        type: integer
      - name: nickname
        type: string
        optional: true
```

Function parameters and returns are structured, while `body.typeql` retains
the provider function body as a typed literal:

```yaml
functions:
  top-scorer:
    parameters:
      - name: game
        type: game
    returns:
      stream: [player]
    body:
      typeql: |
        match
          ($game, $player) isa participation;
        return { $player };
```

Provider execution remains capability-gated. Recognizing a schema construct
does not claim that every configured provider can execute it.

## Workspace manifest

Workspace V1 has these closed top-level keys: `format`, `schema`,
`compatibility`, `migrations`, `bindings`, `secrets`, `extensions`, and
`environments`.

```yaml
format: typebridge.workspace/v1

schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: example-schema

compatibility:
  semantic-profile: typedb-3.12.1/v1
  require: [schema.doc-meta]

migrations:
  directory: migrations/v2
  app-label: example
  destructive: require-approval

bindings:
  python:
    output: generated/python
  typescript:
    output: generated/typescript
  rust:
    output: generated/rust

secrets:
  service.api-token:
    env: EXAMPLE_API_TOKEN

environments:
  development:
    database: example-development
    uri: localhost:1729,[::1]:1729
    http-port: '8000'
    tls: 'false'
    migrate: 'false'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
    requirements: [schema.doc-meta]
```

`schema.ownership` is exactly `exclusive`. `migrations.destructive` is either
`require-approval` or the tighter `reject`; a standing force allowance is
invalid. Binding output directories, the schema-set, the migration directory,
and the workspace manifest must be confined and non-overlapping.

Workspace extension entries contain only an exact version and require a local
registered handler:

```yaml
extensions:
  com.example.codegen:
    version: 1.0.0
```

The shipped CLI has no extension handlers, so the executable fixture below
intentionally omits this optional block.

### Environment addresses and transport

`uri` is an address list, despite its historical field name. Each member is
either a DNS-style ASCII host plus port or a bracketed IPv6 address plus port:

```text
localhost:1729
db-1.internal:1729,db-2.internal:1729
[2001:db8::1]:1729
db.internal:1729,[2001:db8::2]:1730
```

Ports are decimal `1..=65535`. Brackets are mandatory for IPv6. Credentials,
schemes, paths, query strings, fragments, whitespace, control characters,
empty list members, and malformed DNS labels are rejected before any secret
resolution or network I/O.

`tls` and `migrate` use exact string Booleans, `'true'` or `'false'`:

| `tls` | `tls-root-ca` | Result |
| --- | --- | --- |
| omitted or `'false'` | omitted | plaintext |
| `'true'` | omitted | TLS with operating-system roots |
| `'true'` | confined readable PEM path | TLS trusting that bundle |
| omitted or `'false'` | any path | configuration error |

A root-CA path never enables TLS implicitly. Offline workspace loading validates
the policy but does not resolve username/password values or contact TypeDB.

## Executable reference fixture

The checked-in fixture under
[`docs/fixtures/split-yaml-v1`](../fixtures/split-yaml-v1/typebridge.yaml)
contains the complete workspace shape above plus a schema-set and a fragment
that exercise compact and expanded facts, annotations, subtyping, role
specialization, and root `plays` declarations:

- [`typebridge.yaml`](../fixtures/split-yaml-v1/typebridge.yaml)
- [`schema/schema.yaml`](../fixtures/split-yaml-v1/schema/schema.yaml)
- [`schema/fixture.yaml`](../fixtures/split-yaml-v1/schema/fixture.yaml)

From that fixture directory, validate it without a TypeDB server:

```bash
type-bridge schema check
```

From a source checkout, the equivalent command is:

```bash
cargo run --manifest-path type-bridge-core/Cargo.toml \
  -p type-bridge-cli -- \
  --manifest docs/fixtures/split-yaml-v1/typebridge.yaml schema check
```

`schema check` is read-only and offline. `schema generate` writes every
configured binding plus the server authority artifact without contacting
TypeDB. Migration authoring and connected migration commands remain separate
explicit operations.
