---
name: type-bridge
description: Build, migrate, query, or operate TypeDB applications with TypeBridge across Python, TypeScript/Node, generated Rust, Split-YAML schema workspaces, immutable typed queries, and the TypeBridge server. Use when defining TypeDB models, implementing CRUD or queries, generating SDKs, managing schema migrations, selecting a TypeBridge language surface, or troubleshooting TypeBridge compatibility and runtime behavior.
---

# Use TypeBridge

Treat TypeBridge as a multi-language TypeDB application toolkit. One Rust
semantic engine owns schema, query, migration, validation, code generation,
ORM, and provider behavior. Python and Node expose language-native facades;
generated Rust applications and the query server consume the same contracts.

## Choose the surface

| Task | Surface | Read first |
| --- | --- | --- |
| Build a Python application | `type-bridge` | `getting-started/quickstart.md`, `guide/models.md`, `guide/data.md` |
| Build a Node application | `@type-bridge/node` | `guide/typescript.md` |
| Build a Rust application | generated schema crate + `type-bridge` | `guide/rust.md` |
| Own a canonical schema | Split-YAML workspace and CLI | `guide/schema-workflows.md`, `guide/split-yaml-v1.md` |
| Run remote queries | TypeBridge server | `guide/server-container.md`, `guide/typed-queries.md` |
| Upgrade an existing app | compatibility guides | `guide/upgrade-v2.md`, `guide/v2-deprecations.md` |

Resolve these paths relative to this file when the repository documentation is
available. Otherwise use <https://ds1sqe.github.io/type-bridge/>.

## Start every task

1. Determine whether the target is this repository or an application consuming
   TypeBridge.
2. Inspect the target's `pyproject.toml`, `package.json`, `Cargo.toml`,
   `typebridge.yaml`, and schema files as applicable. Do not assume the
   documentation branch matches the installed version.
3. Identify the language surface, TypeBridge version, TypeDB server version,
   and whether execution is direct or remote.
4. Identify the canonical V2 Split-YAML workspace. Treat existing TypeQL or
   released Python/Node declarations as migration input, never as a second
   active writer for the same scope.
5. Read only the relevant guide pages from the routing table before changing
   an API boundary.
6. Implement through the language facade and let the Rust engine own semantic
   validation.
7. Verify the focused behavior, then run the surface-level check.

When working in this repository, read `../DEVELOPMENT.md` first. Locate public
behavior in these ownership areas:

| Boundary | Source |
| --- | --- |
| Python facade | `../type_bridge/` |
| Shared engines and contracts | `../type-bridge-core/crates/` |
| TypeScript/Node facade | `../type-bridge-core/crates/node/` |
| Public Rust client | `../type-bridge-core/crates/rust/` |
| Schema generation | `../type-bridge-core/crates/schema-codegen/` |
| Tests and parity contracts | `../tests/` |

## Preserve the contracts

- Keep Rust as the only V2 semantic engine. Do not implement schema, query,
  migration, validation, or ORM rules independently in Python or TypeScript.
- Use Split-YAML as the sole active desired-schema authority. Read-only archive
  conversion and released query compatibility are not authoring paths.
- Treat generated Python, TypeScript, and Rust files as projections. Regenerate
  them after schema changes; do not edit them by hand.
- Sync or migrate the schema before inserting application data.
- Use attribute instances at typed model boundaries.
- Use JavaScript `bigint` for TypeDB integer values.
- Resolve Rust releases starting with 2.0.1 from crates.io. Resolve historical
  2.0.0 consumers from the exact release Git revision.
- Preserve exact compatibility, deprecation, remote-trust, and resource-limit
  behavior from the relevant guide.

## Model TypeDB semantics directly

- Define attributes as reusable independent types. Let entities and relations
  own them.
- Mark stable identity with Split-YAML `key: true` and non-key uniqueness with
  `unique: true`.
- Express optional and repeated ownership with explicit `card` bounds.
- Put `ordered` and `distinct` on the exact ownership or role edge that carries
  those semantics.
- Keep relates-side and plays-side cardinalities separate.
- Use abstract schema types and explicit `sub` edges for polymorphic contracts.
- Treat schema labels as authority; target-language names are generated
  projections.

Define relation roles and owned attributes in Split-YAML:

```yaml
format: typebridge.schema/v2
attributes:
  name: { value: string }
  age: { value: integer }
entities:
  person:
    owns:
      name: { key: true }
      age: { card: { min: 0, max: 1 } }
  company:
    owns:
      name: { key: true }
relations:
  employment:
    relates:
      employee: { card: 1 }
      employer: { card: 1 }
plays:
  person:
    employment: { employee: {} }
  company:
    employment: { employer: {} }
```

Read `guide/attributes.md`, `guide/entities.md`, `guide/relations.md`, and
`guide/cardinality.md` before implementing inheritance, overridden roles,
ordered values, schema metadata, or unusual cardinality.

## Python workflow

Install Python 3.12–3.14 support:

```bash
pip install type-bridge
```

Generate the Python package from the workspace, apply the canonical migration,
then use only generated models and tokens:

```python
from app_models import Age, Name, Person
from type_bridge import Database

db = Database(address="localhost:1729", database="example")
db.connect()
db.create_database()

ada = Person(name=Name("ada"), age=Age(36))
Person.manager(db).put(ada)
adults = Person.manager(db).filter(age__gte=Age(18)).all()
```

Use keyword arguments for generated entity and relation constructors. Change
labels, abstractness, ownership, roles, or cardinalities in Split-YAML and
regenerate; do not hand-edit emitted packages.

Use generated model managers for ordinary CRUD, filtering, ordering, grouping,
and transactions. Import the generated package's `QuerySession` for connected
multi-model selection, owner-aware fields and roles, named pages, counts,
existence checks, bounded reachability, or one-exchange remote execution.

## Choose the data operation

| Intent | Operation |
| --- | --- |
| Create and reject duplicates | `insert()` / `insert_many()` |
| Idempotently create by key | `put()` / `put_many()` |
| Persist a known keyed model | `update()` / `update_many()` |
| Read one model type | manager `get`, `filter`, `all`, `first`, `count` |
| Match connected model types | immutable `QuerySession` |
| Author binding-neutral V2 plans | `type_bridge.query_v2` or Node `query-v2` |
| Execute handcrafted TypeQL | raw query API, only when typed surfaces do not fit |

Require a key for `put()` and `update()`. For relation writes, prefer hydrated
role players carrying IIDs; otherwise provide key-complete stubs. Reject role
players that have neither identity form.

Reuse a caller-owned transaction for atomic multi-model work:

```python
from type_bridge import TransactionType

with db.transaction(TransactionType.WRITE) as tx:
    Person.manager(tx).put(person)
    Company.manager(tx).put(company)
    Employment.manager(tx).put(employment)
```

Do not use `sync_schema(force=True)` as conflict recovery without explicit
authorization for database recreation and data loss.

## Choose the query surface

- Use manager filters for a single root model, Django-style lookups,
  aggregation, ordering, pagination, and ordinary CRUD.
- Use the generated Python or TypeScript package's `QuerySession` for
  owner-aware, connected, multi-model matches. Create variables from one
  session and never mix handles or tokens between generated packages.
- Use `type_bridge.query_v2` or `@type-bridge/node/query-v2` for complete
  binding-neutral plan authoring. Let Rust create canonical bytes and
  fingerprints; never assemble mutable plan JSON in a facade.
- Use raw TypeQL only when the higher-level surfaces cannot express the
  operation, and retain parameterization and version gates.

Treat query construction as local and synchronous. Direct terminals perform
provider work. A remote terminal performs exactly one caller-owned exchange;
the client owns transport, authentication, retry policy, and capability trust.
Generated Python and TypeScript `RemoteQuerySession` constructors derive
authority from private package evidence. Supply advertisement bytes, the
one-exchange callback, and limits; never read an authority file or construct a
low-level `QueryV2Authority` for this normal path.

## TypeScript and Node workflow

```bash
npm install @type-bridge/node
```

```ts
import { Age, Name, Person, QuerySession } from "./generated/app-models/index.js";
import { RustDatabase } from "@type-bridge/node";

const db = RustDatabase.connect("localhost:1729", "example");
const ada = Person.create({ name: Name.create("ada"), age: Age.create(36n) });
Person.manager(db).put(ada);
const adults = Person.manager(db).filter({ age__gte: Age.create(18n) }).all();
const session = new QuerySession(db);
```

Import immutable model queries from the generated package and low-level V2 plan
authoring from `@type-bridge/node/query-v2`. Consult `guide/typescript.md` for
database lifecycle, integer `bigint` values, managers, and generation.

Use scheme-free `host:port` addresses by default. Keep URI scheme and
`tlsEnabled` consistent. Require `tlsEnabled: true` when supplying
`tlsRootCa`. Close `RustDatabase` handles when finished, but do not treat
synchronous `close()` as an out-of-band cancellation mechanism for a native
call occupying the event-loop thread. Use V2 deadlines for cancellable work.

## Canonical schema and generation workflow

For new multi-language systems:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
type-bridge --manifest typebridge.yaml migration make --name initial
type-bridge --manifest typebridge.yaml migration plan
type-bridge --manifest typebridge.yaml migration apply --environment development
```

Keep credentials in environment references, not committed workspace files.
Review a migration plan before applying it. Generation is offline and does not
change TypeDB.

Use this lifecycle:

1. Validate the schema set and workspace offline.
2. Generate every configured language package from that checked snapshot.
3. Create a named migration.
4. Review its TypeQL, plan, and destructive classifications.
5. Apply only in an environment whose policy permits migration.
6. Verify the resulting database and journal state.
7. Commit Split-YAML, `typebridge.yaml`, migrations, and generated packages
   together when the repository tracks generated outputs.

Treat `schema check`, migration planning, and generation as read-only with
respect to TypeDB. Only the explicit connected migration commands mutate the
managed schema. Split-YAML is the sole active authoring authority; historical
TOML is a read-only conversion input.

Configure every projection target in `typebridge.yaml`, then generate them from
the same checked workspace. No standalone JSON is required for generated
managers or package-owned query sessions.

If deploying the generic server, configure its authority artifact alongside the
bindings and commit it with the other generated outputs when the repository
tracks them:

```yaml
artifacts:
  schema-authority:
    output: generated/schema-authority.json
```

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

One generation snapshot embeds compiled authority into every configured
package and, when configured, writes the byte-equivalent server artifact. Its
canonical JSON is an internal, source-free deployment codec, not a
user-maintained schema input.

## Rust and server boundaries

Use Rust 1.88 or newer. Bind the generated schema package to the database
before using generated models. Follow `guide/rust.md` for the exact release
revision, dependency patch, connection, transaction, CRUD, and remote-query
forms.

Classify Rust SDK failures through `Error::category()`, `code()`, `path()`,
and `model_validation_phase()`; do not parse display messages. Preserve those
fields across direct and remote execution. In a caller-owned
`RemoteQueryTransport`, wrap transport failures with `Error::remote` and a
stable lowercase snake-case code.

Use the server container only with the configuration, TLS, generated
`schema_authority_file`, explicit `authority_mode`, resource limits, and
immutable digest described in `guide/server-container.md`. Scope and semantic
profile come from the verified artifact rather than duplicate server settings.
The client owns remote transport, authentication, retry, and
capability-advertisement trust.

## Diagnose failures by boundary

| Symptom | Check |
| --- | --- |
| Connect or protocol failure | TypeDB version, accepted driver band, address, credentials, TLS scheme/options |
| Feature rejected before I/O | Feature gate; `@doc`, `@meta`, ordered ownership, and given rows can require TypeDB 3.12 |
| Schema conflict | Existing types and migration history; do not jump to force recreation |
| Missing type during CRUD | Ensure the schema was synchronized or migrated before data operations |
| Relation player cannot be matched | Supply a hydrated IID or every key attribute |
| Node integer rejected | Use `bigint`, not JavaScript `number` |
| Generated model mismatch | Regenerate from the canonical schema and compare schema identity/fingerprint |
| Typed-query owner/session error | Recreate fields, roles, and variables from the same model owner and session |
| Remote reply rejected | Check embedded schema authority, capabilities, executor epoch, signature, deadline, and size limits |
| Closed-handle failure | Do not reuse a closed database or transaction; inspect lease ownership |

Read `development/typedb.md` before changing compatibility or provider
behavior. Read `development/typed-query-contract.md` before changing shared
typed-query semantics. Preserve structured diagnostics rather than replacing
them with facade-local generic errors.

## Verify changes

Select checks by changed surface:

| Change | Minimum verification |
| --- | --- |
| Python facade or models | focused `uv run pytest …`, then `./scripts/check.sh python` |
| Node facade or declarations | focused npm test/typecheck, then `./scripts/check.sh node` |
| Rust engine or SDK | focused Cargo test, then `./scripts/check.sh rust` |
| Schema generation | target acceptance test plus Python/TypeScript/Rust projection checks |
| Documentation or skill | `uv run --extra docs mkdocs build --strict` and skill validation |
| Cross-surface semantics | parity/contract tests plus `./test.sh` when live TypeDB behavior changes |

Use `./test.sh` for the full isolated source-tree suite with TypeDB. Exact
wheel, npm tarball, native-platform, container, and publication acceptance
remains workflow-only.

Before reporting completion:

- confirm generated files and documentation match the implemented surface;
- confirm no compatibility or deprecation promise was broadened accidentally;
- report which source and live tiers ran;
- distinguish local source checks from workflow-only artifact acceptance.
