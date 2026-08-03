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
It defines and evolves schemas, generates application models, and provides
native Python, TypeScript/Node, and Rust data APIs. One Rust semantic engine
owns schema, query, migration, validation, and ORM behavior across every SDK
and the standalone query server.

## One system, several ways in

| Surface | Use it for | Distribution |
| --- | --- | --- |
| Python | Declarative Pydantic models, CRUD, queries, schema management | [`type-bridge`](https://pypi.org/project/type-bridge/) |
| TypeScript / Node | Branded models, typed managers, queries, native runtime | [`@type-bridge/node`](https://www.npmjs.com/package/@type-bridge/node) |
| Rust | Generated schema crates, async CRUD and immutable queries | [`type-bridge`](https://crates.io/crates/type-bridge) |
| CLI and generators | Split-YAML schemas, migrations, Python/Node/Rust projections | Included with the Python package |
| Server | Remote V2 query execution over the same contracts | `ghcr.io/ds1sqe/type-bridge-server` |

TypeBridge is designed around TypeDB rather than flattened into a relational
ORM shape:

- attributes remain independent types owned by entities and relations;
- roles, inheritance, cardinality, ordering, `@key`, and `@unique` stay typed;
- schema generation and migration use the same rules as runtime queries;
- direct and remote immutable queries preserve typed, owner-aware results;
- Python and Node are thin facades over the shared Rust runtime.

## Start with Python

Requires Python 3.12–3.14 and a supported TypeDB 3.x server.

```bash
pip install type-bridge
```

```python
from type_bridge import Database, Entity, Flag, Integer, Key, SchemaManager, String

class PersonId(String):
    pass

class Age(Integer):
    pass

class Person(Entity):
    person_id: PersonId = Flag(Key)
    age: Age | None = None

db = Database(address="localhost:1729", database="example")
db.connect()
db.create_database()

schema = SchemaManager(db)
schema.register(Person)
schema.sync_schema()

ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
people = Person.manager(db).filter(age__gte=18).all()
```

Continue with the [Python quick start](https://ds1sqe.github.io/type-bridge/getting-started/quickstart/),
then choose the [model](https://ds1sqe.github.io/type-bridge/guide/models/),
[data](https://ds1sqe.github.io/type-bridge/guide/data/), or
[schema](https://ds1sqe.github.io/type-bridge/guide/schema-workflows/)
workflow.

## TypeScript / Node

```bash
npm install @type-bridge/node
```

```ts
import { Entity, Key, attr, field } from "@type-bridge/node";

class PersonId extends attr.String("person-id") {}
class Person extends Entity("person", {
  personId: field(PersonId, Key),
}) {}

const ada = new Person({ personId: new PersonId("ada") });
```

See the [TypeScript/Node guide](https://ds1sqe.github.io/type-bridge/guide/typescript/)
for native targets, database lifecycle, managers, queries, and generation.

## Schema-first and Rust workflows

The V2 workspace makes a versioned Split-YAML schema the authority and projects
it into each configured SDK:

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
type-bridge --manifest typebridge.yaml migration make --name initial
type-bridge --manifest typebridge.yaml migration apply --environment development
```

The generated Rust SDK requires Rust 1.88+ and, starting with 2.0.1, resolves
from crates.io with `type-bridge = "2"`. TypeBridge 2.0.0 remains available
from its exact Git revision. Follow the
[Rust client guide](https://ds1sqe.github.io/type-bridge/guide/rust/) and
[Split-YAML reference](https://ds1sqe.github.io/type-bridge/guide/split-yaml-v1/)
for the reproducible setup.

## Documentation

- [Install a surface](https://ds1sqe.github.io/type-bridge/getting-started/installation/)
- [Choose an SDK](https://ds1sqe.github.io/type-bridge/guide/sdks/)
- [Model TypeDB data](https://ds1sqe.github.io/type-bridge/guide/models/)
- [Read and write data](https://ds1sqe.github.io/type-bridge/guide/data/)
- [Manage schemas and migrations](https://ds1sqe.github.io/type-bridge/guide/schema-workflows/)
- [Run the server](https://ds1sqe.github.io/type-bridge/guide/server-container/)
- [Upgrade to 2.0](https://ds1sqe.github.io/type-bridge/guide/upgrade-v2/)
- [Python API reference](https://ds1sqe.github.io/type-bridge/reference/)

TypeBridge 2.0.x supports TypeDB 3.8–3.12. Support for 3.8 and 3.10 is
deprecated and scheduled for removal in 2.1; consult the
[compatibility matrix](https://ds1sqe.github.io/type-bridge/development/typedb/#server-and-driver-compatibility)
and [deprecation inventory](https://ds1sqe.github.io/type-bridge/guide/v2-deprecations/)
before upgrading production deployments.

## Development

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync --extra dev
uv run pytest
./test.sh
./scripts/check.sh all
uv run --extra docs mkdocs build --strict
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for repository layout, test tiers, docs
architecture, and contribution checks.

## License

TypeBridge-authored code is MIT licensed. Native artifacts also include
third-party TypeDB components under Apache-2.0 and MPL-2.0; exact notices ship
with each artifact and are documented in
[`type-bridge-core/vendor/README.md`](type-bridge-core/vendor/README.md).
