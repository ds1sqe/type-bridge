# TypeBridge internals

TypeBridge 2.1 has one active authoring path and one semantic engine:

```text
Split-YAML workspace
  -> strict Rust schema resolution
  -> canonical declared schema + fingerprints
  -> generated Python / TypeScript / Rust projections
  -> projection registration and exact generated values
  -> Rust ORM/query/migration/provider execution
  -> typed hydrated results
```

Target-language classes do not define schema. Generation is an offline
projection of canonical workspace bytes.

## Authority and evidence

`typebridge.yaml` selects a closed schema-set root, compatibility profile,
migration directory, binding outputs, and environments. Resolution produces
canonical declared-schema evidence. Each generated package embeds:

- the declared schema and projection descriptor bytes;
- schema, projection, target, and package fingerprints;
- exact entity, relation, attribute, reference, field-token, and role-token
  identities;
- the generated manager and query facade for that projection.

Python and Node register those immutable bytes and exact emitted identities at
package import. Registration rejects forged classes, structural lookalikes,
changed fingerprints, duplicate/conflicting installations, and unbounded input.
Rust binds the same evidence through its generated `SchemaPackage`.

Generated integration uses a projection-owned nominal contract. It does not
route values through the retained V1 `TypeDBType` nominal boundary, and that
boundary must never be widened to `object`, `Any`, or structural detection.

## Rust ownership

The principal layers are:

| Layer | Responsibility |
| --- | --- |
| `contract` | Stable schema/query/runtime wire identities and limits |
| `schema` | Split-YAML resolution and projection facts |
| `schema-codegen` | Deterministic Python, TypeScript, and Rust emitters |
| `query` | Immutable query authority, validation, and lowering |
| `migration` / `schema-migration` | Canonical V2 planning and state |
| `orm` | Generated projection CRUD, query, hydration, and transactions |
| `typedb-runtime` | Retained TypeDB provider routing and lifecycle |
| `python` / `node` / `rust` | Language boundaries over the shared engine |

Bindings validate target-language types and snapshot hostile byte inputs before
crossing FFI. They do not reimplement schema meaning, query semantics, limits,
or provider selection.

## Generated managers

A generated model owns its manager entry point:

```python
ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
people = Person.manager(db).filter(age__gte=Age(18)).all()
```

The model/token identity selects an installed projection descriptor. The shared
dynamic manager is private execution machinery; it cannot register application
descriptors or accept arbitrary user classes.

Filter keys are resolved against exact projected fields. A complete field name
containing `__` is equality by default. When a field label collides with a
lookup suffix, append the explicit lookup, for example
`score__gte__eq=ScoreGte(8)`, to select equality on the `score__gte` field;
`score__gte=Score(8)` remains comparison on `score`.

All data operations revalidate exact wrapper, ownership, role-player, IID/key,
cardinality, and scalar constraints before lowering. Hydration restores the
exact generated concrete type and reference form.

## Immutable generated queries

For one model, the manager is the concise facade. Multi-model predicates and
result shapes use a package-owned query session:

```python
session = Person.query(db)
person = session.exact(Person)
employment = session.exact(Employment)
employee = employment.role(Employment.employee).connects(person)
rows = session.query(person, employment).where(employee).rows(limit=100)
```

Field and role tokens retain their owner and session. Query preparation lowers
the complete immutable graph once in Rust, authenticates the prepared request,
and applies the same result-shape validation for direct and remote execution.
Bindings expose language-native typed builders without embedding pre-authored
plan bytes.

The separately retained raw/V1 query facades are isolated compatibility
contracts. Generated V2 packages do not depend on them except where an
explicit raw query helper is documented.

## Transactions and lifecycle

Managers and query sessions accept either a database handle, which owns a
bounded operation transaction, or a caller-owned transaction for atomic
multi-operation work. Native database, transaction, prepared-request, and
reply handles are one-shot/lease-aware and reject use after close or consume.

Provider selection first obtains or accepts an exact server version, validates
the 3.11–3.12 support window, and then chooses band 8 or 9. Unknown and retired
versions fail before application data work. See [TypeDB integration](typedb.md).

## Migration and archive separation

Active change authority is the V2 workspace migration flow:

```text
migration make -> plan -> apply -> verify
```

Read-only readers remain for historical TOML, released Python/JSON migration
records, checksums, ledgers, snapshots, and metadata. They may verify, convert,
or adopt immutable history into a V2 workspace. They cannot create a new root
history, write historical snapshots, or become desired-schema authority.

Private archive modules use `_archive_…` names. Do not add a `_legacy` package
or expose archive implementation as an application API.

## Compatibility and release closure

The release graph is closed:

- TypeBridge product crates share the exact 2.1 version.
- TypeDB bands 8 and 9 are the only active providers.
- Python, npm, and Cargo archive validators inspect exact member sets and
  reject removed authoring/provider payloads.
- Native notices are generated from the packaged Cargo graph.
- Registry preflight distinguishes immutable pre-existing provider crates from
  new release keys and verifies official checksums.

The executable operation inventory and removal map are the audit authority for
1:1 parity. A deleted handwritten test family is acceptable only when every
operation maps to generated evidence or a separately retained query/archive
contract.

## Change checklist

When changing a shared operation:

1. Change the Rust contract/engine first.
2. Update each generated facade that advertises it.
3. Regenerate type/runtime acceptance fixtures.
4. Add hostile boundary cases and live materialization evidence.
5. Update the cross-language operation inventory.
6. Inspect release artifacts so private implementation does not become public.

See [Testing](testing.md), [typed-query contract](typed-query-contract.md), and
[Rust generated parity](rust-generated-parity.md) for executable evidence.
