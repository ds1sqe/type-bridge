# @type-bridge/node

Node.js NAPI facade for the shared type-bridge Rust ORM runtime.

This package is a boundary layer only. Descriptor validation, query
construction, CRUD execution, hydration, and transaction behavior live in
`type-bridge-orm`; the Node package loads the native module, marshals
JavaScript values, and exposes TypeScript-friendly wrappers.

## Build

```bash
npm run build
```

The package entry is `dist/index.js`, with declarations emitted to
`dist/index.d.ts`. The additive typed-query subpath resolves to
`dist/typed/index.js` and `dist/typed/index.d.ts`.

For local smoke runs after `cargo build -p type-bridge-node`, copy the built
library to a `.node` file or point the loader at one:

```bash
TYPE_BRIDGE_NODE_NATIVE_PATH=/path/to/type_bridge_node.node npm run smoke:package
```

## Prebuilt Native Targets

The published npm tarball contains all native modules selected by the package
loader:

- Linux glibc on x64 and arm64
- macOS on x64 and arm64
- Windows on x64 and arm64

Release CI builds each module on its matching native GitHub-hosted runner,
combines the six immutable outputs into one tarball, and executes that tarball
on every target. Node 18 is the supported runtime floor; the full platform
matrix runs on Node 20, with an additional Node 18 acceptance run on Linux x64.
Linux musl and processor architectures outside this list are not prebuilt.

## Public Surface

```ts
import {
  DescriptorRegistry,
  Marshalling,
  RustDatabase,
  long,
  string,
} from "@type-bridge/node";

const registry = new DescriptorRegistry();
const person = registry.registerEntity({
  type_name: "person",
  is_abstract: false,
  parent_type: null,
  owned_attributes: [
    {
      field_name: "name",
      attr_name: "person-name",
      value_type: "string",
      annotations: ["Key"],
      is_optional: false,
      is_ordered: false,
    },
    {
      field_name: "age",
      attr_name: "age",
      value_type: "long",
      annotations: [],
      is_optional: true,
      is_ordered: false,
    },
  ],
});

const attrs = { name: string("Alice"), age: long(42n) };
new Marshalling().entityAttributes(person, attrs);

const db = RustDatabase.connect("127.0.0.1:1729", "example", {
  httpPort: 8000,
  // Optional: skip HTTP probing when only gRPC is reachable.
  serverVersion: "3.11.5",
});
const manager = db.entityManager(person);
manager.insert(attrs);
```

## Immutable Typed Queries

The additive `./typed` subpath builds owner-aware matches over the same model
constructors. It does not replace the package-root managers or raw query
surface:

```ts
import { Entity, Key, attr, field } from "@type-bridge/node";
import { QuerySession, references } from "@type-bridge/node/typed";

class PersonName extends attr.String("person-name") {}
class Person extends Entity("person", {
  name: field(PersonName, Key),
}) {}

const session = new QuerySession(db);
const person = session.var(Person);
const personRefs = references(Person);

const page = session.query(person).pageBy(person, {
  limit: 25,
  orderBy: [person.field(personRefs.fields.name).asc()],
  includeTotal: true,
});
```

Variables, bound fields and roles, predicates, orders, and queries retain
opaque native handles. `references(Model)` returns frozen, owner-branded
JavaScript reference tokens; resolving one against a variable creates the
native field or role handle. The same immutable `Query` supports exact or
subtype matching, 1–16 selected models, hidden graph witnesses,
named/collected pages, distinct-root counts, and existence checks. Subtype
queries must register constructors that JavaScript cannot discover with
`session.registerModels(...)`. See the
[typed-query guide](https://ds1sqe.github.io/type-bridge/guide/typed-queries/)
for the complete contract and transaction rules.

## Descriptor Shape

Descriptors use the canonical Rust serde shape:

- Entity descriptor: `type_name`, `is_abstract`, `parent_type`, and
  `owned_attributes`.
- Relation descriptor: entity fields plus `roles`.
- Owned attributes: `field_name` is the JavaScript-facing field key,
  `attr_name` is the TypeDB attribute label, `value_type` is one TypeDB
  primitive value type, `annotations` carries `Key`, `Unique`, or `Card`, and
  `is_optional` mirrors the model field optionality. `is_ordered` declares
  ordered multi-valued storage and is required even when `false`.
- Relation roles: `role_name`, accepted `player_type_names`, and optional
  `[min, max]` cardinality.

## BigInt Rule

TypeDB `long` maps to Rust `i64`, so JavaScript `number` is not accepted by
`long()`. Use `long(9223372036854775807n)`.

`longFromNumberUnsafe()` exists only for explicit caller-owned conversions when
the value is known to fit safely enough for that call site.

Runtime rows return `Long` values as decimal strings so no precision is lost
while crossing JSON.

## Supported Runtime Operations

The current facade exposes descriptor registration, value marshalling, database
and transaction handles, entity and relation managers, insert/put/update/get,
batch insert/put, count, aggregate, group-by aggregate, relation role-player
filters, and IID deletes.

The facade does not expose a direct TypeDB driver or accept caller-provided
TypeQL for the immutable typed path. Its native Rust compiler and validator own
the semantic plan and execution boundary. Use `npm run scope:probe` to verify
the Node crate stays inside that boundary.
