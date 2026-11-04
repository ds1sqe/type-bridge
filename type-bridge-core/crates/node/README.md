# @type-bridge/node

TypeScript and Node SDK for the TypeBridge shared Rust semantic engine.

This package is the runtime boundary used by bindings generated from a split
schema YAML. CRUD, hydration, and query semantics live in the shared Rust
engine; application model constructors, fields, roles, and managers come only
from the generated package.

## Build

```bash
npm run build
```

The package entry is `dist/public.js`, with declarations emitted to
`dist/public.d.ts`. Generated packages consume the exported
`./runtime-projection` subpath. The separately retained low-level Query V2 plan
builder is exported from the package root.

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
import { Database } from "@type-bridge/node";
import { Age, Person, PersonId } from "./generated/index.js";

const db = Database.connect("127.0.0.1:1729", "example", {
  httpPort: 8000,
  serverVersion: "3.14.0",
});
const ada = Person.create({
  personId: PersonId.create("ada"),
  age: Age.create(36n),
});
Person.manager(db).put(ada);
const adults = Person.manager(db)
  .filter({ age__gte: Age.create(18n) })
  .all();
```

There is no runtime API for registering handwritten entity or relation
descriptors. Change the schema YAML and regenerate the application package when
the model changes.

Scheme-free `host:port` addresses are recommended. Released matching URI forms
remain accepted: `http://host:port` with disabled TLS and
`https://host:port` with enabled TLS. The upstream driver rejects a URI scheme
that contradicts `tlsEnabled`. TLS is opt-in through `tlsEnabled`; with no
`tlsRootCa`, an enabled connection uses native trust roots, while a custom PEM
bundle requires both options explicitly:

```ts
const secureDb = Database.connect("db.example.com:1729", "example", {
  tlsEnabled: true,
  tlsRootCa: "/run/secrets/type-db-root.pem",
});
```

`tlsRootCa` with an omitted or false `tlsEnabled` is a configuration error and
never enables TLS implicitly.

### Connection close and cancellation

`db.close()` synchronously marks the connection terminal and dispatches the
upstream driver's shutdown request; once dispatched, the connection admits no
new work. The unmodified upstream 3.12.1 driver releases its final callback
worker when the last native driver lease drops, so a retained closed handle is
not a synchronous worker-join boundary. Close is also not an out-of-band
cancellation channel for the synchronous CRUD APIs. While one of those native
calls occupies the JavaScript event-loop thread, JavaScript cannot dispatch
`db.close()` concurrently to interrupt it. Generated query terminals return
Promises and run off that thread; cancel them through the session's
`QueryCancellation` and bound them with its `timeoutMilliseconds`.

For V2 work that must remain cancellable, use the Promise-returning
`queryV2ExecuteLocal(...)` surface and supply its `deadlineMs` argument. Remote
V2 requests likewise carry the bounded `QueryV2RemoteLimits.deadlineMs` expiry.
Do not rely on a later synchronous `db.close()` call as the timeout mechanism.

## Immutable Typed Queries

Schema generation emits immutable owner-aware query classes beside each
application's generated models and tokens. The base package supplies the
connection and verified runtime projection; it does not register handwritten
model descriptors:

```ts
import { Person, QuerySession } from "./generated/app-models/index.js";
import { Database } from "@type-bridge/node";

const db = Database.connect("localhost:1729", "example");
const session = new QuerySession(db);
const person = session.exact(Person);

const page = await session.query(person).pageBy(person, {
  limit: 25n,
  orderBy: [person.field(Person.name).asc()],
  includeTotal: true,
});
```

The session runs on a direct `Database` or transaction, or on a proxy
connection, with the same queries and results. Composition is synchronous;
every terminal returns a `Promise`. Variables, generated field and role tokens,
predicates, orders, and queries retain opaque native handles. The same immutable `Query` supports exact or
subtype matching, 1–16 selected models, hidden graph witnesses,
named/collected pages, distinct-root counts, and existence checks. Constructors
and subtype closures come only from the installed generated projection. See the
[typed-query guide](https://ds1sqe.github.io/type-bridge/guide/typed-queries/)
for the complete contract and transaction rules.

## Native plans and proxy execution

The package root exposes `PlanBuilder` and `Plan`. Builder calls forward
to Rust; build with the generated schema authority and stage the result with
`db.stagedTransaction()` or the proxy connection's staged transaction.

A generated session reaches a TypeBridge server through a proxy connection
passed to the same `QuerySession`. Each awaited terminal performs exactly one
exchange, and the verified authority is embedded in the generated package:

```ts
import {
  ExchangeConnection,
  Person,
  QuerySession,
  connectProxy,
} from "./generated/app-models/index.js";

// The built-in HTTP route; `headers`, `tls`, and `advertisement` are options.
const remote = new QuerySession(await connectProxy("https://typebridge.example"));

// Or a host-owned exchange: one request in, the exact reply bytes out.
const hosted = new QuerySession(new ExchangeConnection(advertisementBytes, exchange));

const person = remote.exact(Person);
const rows = await remote.query(person).rows({ limit: 50n });
```

Generated sessions never read a schema-authority file or accept a low-level
`PlanAuthority` argument. The generated package also re-exports
`ProxyConnection`, `ProxyConnectionOptions`, and `ProxyTlsMode` from
`@type-bridge/node/runtime-projection`.

Native plans use the authenticated V3 staged protocol. The capability
advertisement is a trust input: retrieve it over authenticated TLS for the
intended executor or pin its bytes or fingerprint out of band.
See the [3.0 upgrade guide](https://ds1sqe.github.io/type-bridge/guide/upgrade-3.0/)
for replacements for the removed V1/V2 entry points.

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
