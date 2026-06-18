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

The package entry is `index.js`, with declarations emitted to
`dist/index.d.ts`.

For local smoke runs after `cargo build -p type-bridge-node`, copy the built
library to a `.node` file or point the loader at one:

```bash
TYPE_BRIDGE_NODE_NATIVE_PATH=/path/to/type_bridge_node.node npm run smoke:package
```

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
    },
    {
      field_name: "age",
      attr_name: "age",
      value_type: "long",
      annotations: [],
      is_optional: true,
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

## Descriptor Shape

Descriptors use the canonical Rust serde shape:

- Entity descriptor: `type_name`, `is_abstract`, `parent_type`, and
  `owned_attributes`.
- Relation descriptor: entity fields plus `roles`.
- Owned attributes: `field_name` is the JavaScript-facing field key,
  `attr_name` is the TypeDB attribute label, `value_type` is one TypeDB
  primitive value type, `annotations` carries `Key`, `Unique`, or `Card`, and
  `is_optional` mirrors the model field optionality.
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

The facade does not expose a TypeScript query compiler or a direct TypeDB
driver. Use `npm run scope:probe` to verify the Node crate stays inside that
boundary.
