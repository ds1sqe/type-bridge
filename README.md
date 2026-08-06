<p align="center">
  <img src="https://raw.githubusercontent.com/ds1sqe/type-bridge/master/docs/assets/typebridge-hero.svg" alt="TypeBridge connects typed Python, TypeScript, and Rust applications to TypeDB through one semantic engine." width="100%">
</p>

<p align="center">
  <a href="https://github.com/ds1sqe/type-bridge/actions/workflows/ci.yml"><img src="https://github.com/ds1sqe/type-bridge/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://pypi.org/project/type-bridge/"><img src="https://img.shields.io/pypi/v/type-bridge.svg" alt="PyPI"></a>
  <a href="https://www.npmjs.com/package/@type-bridge/node"><img src="https://img.shields.io/npm/v/%40type-bridge%2Fnode.svg" alt="npm"></a>
  <a href="https://crates.io/crates/type-bridge"><img src="https://img.shields.io/crates/v/type-bridge.svg" alt="crates.io"></a>
  <a href="https://ds1sqe.github.io/type-bridge/"><img src="https://img.shields.io/badge/docs-TypeBridge-67e8f9" alt="Documentation"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-8b5cf6.svg" alt="MIT license"></a>
</p>

TypeBridge is a typed application toolkit for [TypeDB](https://typedb.com/).
A versioned Split-YAML workspace is the schema authority. TypeBridge validates
that workspace and generates Python, TypeScript/Node, and Rust bindings that
share one Rust-owned schema, query, migration, validation, and ORM engine.

## One system, three generated SDKs

| Surface | Generated application API | Distribution |
| --- | --- | --- |
| Python | Value classes, model managers, transactions, direct/remote queries | [`type-bridge`](https://pypi.org/project/type-bridge/) |
| TypeScript / Node | Branded values, model managers, native and remote queries | [`@type-bridge/node`](https://www.npmjs.com/package/@type-bridge/node) |
| Rust | Schema-bound create/model types, async CRUD and immutable queries | [`type-bridge`](https://crates.io/crates/type-bridge) |
| CLI | Split-YAML checks, migrations, and all three projections | Included with the Python package |
| Server | Remote V2 query execution over the same generated contract | `ghcr.io/ds1sqe/type-bridge-server` |

TypeBridge preserves TypeDB concepts directly: independent attributes,
entities, relations, roles, inheritance, cardinality, ordering, keys, and
uniqueness remain typed from schema through hydrated results.

## Start from Split-YAML

Install the CLI and Python runtime:

```bash
pip install type-bridge
```

Create `typebridge.yaml`, a schema-set manifest, and one or more schema
fragments. Configure the generated package output in the workspace, then run:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml migration make --name initial
type-bridge --manifest typebridge.yaml migration apply --environment development
type-bridge --manifest typebridge.yaml schema generate
```

The generated Python package owns the concise single-type manager:

```python
from app_models import Age, Person, PersonId
from type_bridge import Database

db = Database(address="localhost:1729", database="example")
db.connect()

ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
people = Person.manager(db).filter(age__gte=18).all()
```

Use its package-local query session when a query spans types:

```python
session = Person.query(db)
person = session.exact(Person)
adults = (
    session.query(person)
    .where(person.field(Person.age).gte(Age(18)))
    .rows(limit=100)
)
```

Continue with the [Python quick start](https://ds1sqe.github.io/type-bridge/getting-started/quickstart/),
[Split-YAML reference](https://ds1sqe.github.io/type-bridge/guide/split-yaml-v1/),
and [data guide](https://ds1sqe.github.io/type-bridge/guide/data/).

## TypeScript / Node

```bash
npm install @type-bridge/node
```

Import application values from the generated package and connection primitives
from `@type-bridge/node`:

```ts
import { RustDatabase } from "@type-bridge/node";
import { Age, Person, PersonId } from "./generated/models/index.js";

const db = RustDatabase.connect("localhost:1729", "example", {
  username: "admin",
  password: "password",
});
const ada = Person.create({
  personId: PersonId.create("ada"),
  age: Age.create(36n),
});
Person.manager(db).put(ada);
const people = Person.manager(db).filter({ age__gte: Age.create(18n) }).all();
```

See the [TypeScript/Node guide](https://ds1sqe.github.io/type-bridge/guide/typescript/)
for generated managers, transactions, and direct/remote queries.

## Rust

The same workspace emits a schema-bound Rust crate. The generated SDK uses
Rust 1.88+ and the crates.io `type-bridge` runtime:

```toml
[dependencies]
type-bridge = "2"
```

Follow the [Rust client guide](https://ds1sqe.github.io/type-bridge/guide/rust/)
for generation, direct execution, transactions, and remote queries.

## Documentation

- [Install a surface](https://ds1sqe.github.io/type-bridge/getting-started/installation/)
- [Model data in Split-YAML](https://ds1sqe.github.io/type-bridge/guide/models/)
- [Read and write generated models](https://ds1sqe.github.io/type-bridge/guide/data/)
- [Manage schemas and migrations](https://ds1sqe.github.io/type-bridge/guide/schema-workflows/)
- [Run the query server](https://ds1sqe.github.io/type-bridge/guide/server-container/)
- [Compatibility and removals](https://ds1sqe.github.io/type-bridge/guide/v2-deprecations/)

## Development

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync --extra dev
uv run pytest
./test.sh
./scripts/check.sh all
uv run --extra docs mkdocs build --strict
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for repository boundaries and verification.

## License

TypeBridge-authored code is MIT licensed. Native artifacts also include
third-party TypeDB components under Apache-2.0 and MPL-2.0; exact notices ship
with each artifact.
