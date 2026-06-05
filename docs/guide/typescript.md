# TypeScript Typed Surface

A hard-typed TypeScript/Node API mirroring the Python ORM, with branded
attributes, typed managers, expression-form queries, inheritance, multi-value
attributes, and `toDict`/`fromDict` serialization.

## Overview

TypeBridge ships a Node package (`@type-bridge/node`) that is a **thin facade**
over the same shared Rust ORM runtime the Python package uses. The Rust workspace
owns all ORM mechanics — descriptor registration, query compilation, persistence,
hydration. TypeScript and Python are both marshalling-only facades over that one
engine, so a descriptor or a serialized value produced from TypeScript is
byte-identical to the one produced from Python.

The typed surface gives you compile-time safety over that runtime:

- **Branded attributes** — each attribute type is a distinct nominal type; you
  cannot pass a `Name` where an `Email` is expected, even though both wrap a
  string.
- **Typed models** — `Entity()` / `Relation()` build classes whose constructor,
  field reads, managers, and queries are all typed from the schema.
- **Typed managers** — CRUD and queries return your model instances, not loose
  rows.
- **Serialization** — `toDict()` / `fromDict()` convert between a typed instance
  and the canonical plain dict, the same shape Python `to_dict()` produces.

```ts
import { Entity, Key, attr, field } from "@type-bridge/node";

class Id extends attr.String("id") {}
class PersonName extends attr.String("name") {}

class Person extends Entity("person", {
  id: field(Id, Key),
  name: field(PersonName),
}) {}

const alice = new Person({ id: new Id("p1"), name: new PersonName("Alice") });
alice.name.value; // "Alice", typed as string
```

## Attributes

Declare an attribute type by extending one of the `attr` factory bases. The
argument is the TypeDB attribute type name.

```ts
import { attr } from "@type-bridge/node";

class Id extends attr.String("id") {}
class Age extends attr.Integer("age") {}        // TypeDB integer → JS bigint
class Score extends attr.Double("score") {}     // JS number
class Active extends attr.Boolean("active") {}  // JS boolean
class Birthday extends attr.Date("birthday") {}
class LoginAt extends attr.DateTime("login-at") {}
class SeenAt extends attr.DateTimeTZ("seen-at") {}
class Balance extends attr.Decimal("balance") {}
class Session extends attr.Duration("session") {}
```

Each class is **branded** (nominal typing): `Id` and `PersonName` are distinct
types even though both wrap `string`. Construct an instance with `new`, read the
plain value through `.value`:

```ts
const name = new PersonName("Alice");
name.value; // "Alice"
```

### The BigInt rule

TypeDB's `integer`/`long` is a 64-bit integer, which does not fit JavaScript's
`number` safely. The typed surface uses **`bigint` everywhere** for these
attributes:

```ts
class Age extends attr.Integer("age") {}

const age = new Age(40n);   // bigint literal
age.value;                  // 40n, typed as bigint
```

When you must cross from a JS `number`, convert explicitly and accept the lossy
boundary — the API never silently narrows a 64-bit value to a float.

## Models: entities and relations

`Entity(name, schema)` builds a typed entity base class. Extend it to declare
your model. `field(Attr, ...flags)` declares an owned attribute.

```ts
import { Entity, Key, Unique, attr, field } from "@type-bridge/node";

class Id extends attr.String("id") {}
class Email extends attr.String("email") {}
class Age extends attr.Integer("age") {}

class Person extends Entity("person", {
  id: field(Id, Key),
  email: field(Email, Unique),
  age: field(Age).optional(),
}) {}
```

- `field(Attr, Key)` / `field(Attr, Unique)` attach flags (see
  [Flags](#flags-key-unique-card-typeflags)).
- `field(Attr).optional()` makes the field optional — its read type becomes
  `Attr | undefined`, and the constructor accepts its absence.

Construct, then read fields with full typing:

```ts
const alice = new Person({
  id: new Id("p1"),
  email: new Email("alice@example.test"),
  age: new Age(37n),
});

alice.email.value; // "alice@example.test"
alice.age?.value;  // 37n | undefined (optional)
```

### Relations and roles

`Relation(name, schema)` builds a relation; the schema may contain `role(...)`
specs alongside `field(...)` attributes. A role names the player model(s) and its
cardinality.

```ts
import { Card, Entity, Relation, attr, field, role } from "@type-bridge/node";

class Since extends attr.Date("since") {}

class Company extends Entity("company", { id: field(Id, Key) }) {}

class Employment extends Relation("employment", {
  employee: role(Person, { cardinality: Card(1, 1) }),
  employer: role(Company, { cardinality: Card(1, 1) }),
  since: field(Since),
}) {}

const job = new Employment({
  employee: alice,
  employer: new Company({ id: new Id("acme") }),
  since: new Since("2024-05-01"),
});
```

A role may accept several player types and use a raw type-name string for a
player whose typed class is not declared yet:

```ts
evidence: role("person", EmailMessage, { cardinality: Card(0, 5) }),
```

## Flags: `Key`, `Unique`, `Card`, `TypeFlags`

```ts
import { Card, Key, TypeFlags, Unique, field } from "@type-bridge/node";

field(Id, Key);        // @key
field(Email, Unique);  // @unique
field(Tag).list(Card(0, 5)); // multi-value, see below
```

`TypeFlags` customizes the type itself — most often to declare an abstract
supertype:

```ts
import { Entity, TypeFlags, field } from "@type-bridge/node";

class Party extends Entity(TypeFlags({ name: "party", abstract: true }), {
  id: field(Id, Key),
  name: field(PersonName).optional(),
}) {}
```

## Inheritance

Pass a third `{ parent }` argument to inherit from another model. The child
re-lists the parent's attributes in its descriptor and exposes inherited fields
with the parent's brand.

```ts
class Person extends Entity(
  "person",
  {
    email: field(Email, Unique),
    age: field(Age).optional(),
  },
  { parent: Party },
) {}

const p = new Person({
  id: new Id("p1"),          // inherited from Party
  name: new PersonName("A"), // inherited optional
  email: new Email("a@b.test"),
});
```

## Multi-value attributes

`field(Attr).list(Card(min, max))` declares a list-valued field. The read type is
`Attr[]` (or `Attr[] | undefined` when the lower bound is 0).

```ts
class Tag extends attr.String("tag") {}

class Person extends Entity("person", {
  id: field(Id, Key),
  tags: field(Tag).list(Card(0, 5)),       // optional list
  required_tags: field(Tag).list(Card(1, 3)), // required list
}) {}

const p = new Person({
  id: new Id("p1"),
  required_tags: [new Tag("admin")],
});
p.tags;          // Tag[] | undefined
p.required_tags; // Tag[]
```

## Managers and CRUD

`Model.manager(db)` returns a typed CRUD manager over a `RustDatabase`
connection. Every method returns your model instances.

```ts
import { RustDatabase } from "@type-bridge/node";

const db = RustDatabase.connect("localhost:1730", "mydb", {
  username: "admin",
  password: "password",
});

const people = Person.manager(db);

const inserted = people.insert(alice);
people.insertMany([alice, bob]);

const one = people.getByIid(inserted._iid); // Person | null
const some = people.get({ id: new Id("p1") }); // Person[] (exact-match filter)
const all = people.all();                      // Person[]
const total = people.count();                  // bigint

people.update(inserted);
people.delete(inserted);
```

Every hydrated instance carries its TypeDB internal id on `_iid` and is a real
instance of your class (so it also carries `toDict()`).

## Queries and expressions

Build expression-form queries with `manager.query()`. Comparison operators are
static methods on the attribute class; the filter executes inside Rust, not as a
client-side scan.

```ts
// Comparison
people.query().filter(Age.gte(new Age(40n))).all();

// String anchored match
people.query().filter(PersonName.startsWith("A")).all();

// Boolean composition
people
  .query()
  .filter(Age.gte(new Age(40n)).and_(Score.gte(new Score(90))))
  .all();

people
  .query()
  .filter(Age.eq(new Age(30n)).or_(Age.eq(new Age(50n))))
  .all();
```

Aggregates and group-by use `agg` and the numeric aggregate methods:

```ts
import { agg } from "@type-bridge/node";

people.query().aggregate(agg.count(), Age.avg());
people.query().filter(Age.gte(new Age(40n))).aggregate(agg.count());
people.query().groupBy(Dept).aggregate(agg.count());
```

## Serialization: `toDict` / `fromDict`

`toDict()` serializes a typed instance to the **canonical plain dict** — plain
primitives keyed by field name, the same shape Python `to_dict()` produces and
the same shape that crosses the language boundary. `fromDict()` rebuilds a typed
instance from that dict.

```ts
const dict = alice.toDict();
// { id: "p1", email: "alice@example.test", age: 37n, tags: ["admin", "writer"] }

const restored = Person.fromDict(dict);
restored.email.value; // "alice@example.test"
```

Properties of the canonical dict:

- Each branded attribute is unwrapped to its plain primitive; list fields become
  plain arrays.
- Absent optional fields are **omitted** (not present as `undefined`), mirroring
  Python's `model_dump` + `_unwrap_value`.
- `integer`/`long` values are `bigint`; `decimal`/`date`/`datetime`/`datetime-tz`/
  `duration` are strings with the same encodings the cross-language parity oracle
  fixes (e.g. decimal without a `dec` suffix, datetime without trailing
  zero-nanoseconds).
- The return type is schema-derived (`InstanceDict<Schema>`) — never `any`.
  Indexing a non-schema key is a compile-time error.

`fromDict()` is brand- and required-field-safe: an unknown key throws
`TypedCodecError`, and a missing required field throws `TypeError`. On a relation,
`toDict()` emits attribute fields only — role players are excluded.

Use `toDict()` for HTTP responses, fixtures, or logs; use `fromDict()` to rebuild
an instance from a plain object without going through the database.

## Two settled differences from the Python API

The typed TypeScript surface deliberately differs from Python in exactly two
places, both forced by TypeScript's type system:

1. **Phantom-brand nominal typing.** Attribute classes carry a phantom brand so
   that `Id` and `Name` are distinct types even though both wrap `string`. Python
   relies on class identity at runtime; TypeScript needs the brand to make the
   distinction visible to the compiler.

2. **The runtime `field(Attr, Key)` token.** Flags are passed as runtime values
   (`Key`, `Unique`, `Card(...)`) to `field(...)`, rather than via Python's
   class-level metadata, because TypeScript has no decorator-free way to attach
   that information to a field at the type level.

## Generating models from a schema

The Node package ships a model generator — the TypeScript equivalent of the Python
`generate_models()`. Given a TypeDB schema in TypeQL, it writes a ready-to-use set
of typed TypeScript source files so you do not have to author attribute, entity, and
relation classes by hand.

```text
schema.tql  →  generateModels()  →  attributes.ts
                                 →  entities.ts
                                 →  relations.ts
                                 →  index.ts        (barrel re-export)
```

The four generated files mirror the Python generator's output (`.py` files plus
`__init__.py`). The barrel `index.ts` re-exports all generated classes so consumer
code can import from a single path.

### Architecture: one parser, two renderers

Parsing and inheritance resolution happen **once** in the shared Rust core. Each
language then runs its own renderer over that resolved schema. The Python generator
and the TypeScript generator both consume the same `TypeSchema` — they do not
re-implement parsing. This is why a model generated from TypeScript and the
corresponding Python model are structurally equivalent: the shared schema walk
produces identical descriptors in both renderers.

### Usage

`generateModels` and `parseSchema` are exported from `@type-bridge/node`.
The native module must be **injected** — obtain it from `loadNative()` and
pass it via the `native` option:

```ts
import { generateModels, loadNative } from "@type-bridge/node";
import { readFileSync } from "node:fs";

const tql = readFileSync("schema.tql", "utf8");
generateModels(tql, "src/models", { native: loadNative() });
```

`generateModels` writes `attributes.ts`, `entities.ts`, `relations.ts`, and
`index.ts` into the output directory. Each file imports only from
`@type-bridge/node`, so generated code has no transitive dependencies beyond
the package itself.

### API

```ts
generateModels(tql: string, outputDir: string, options: GenerateModelsOptions): void
```

| Parameter   | Type                    | Description                              |
| ----------- | ----------------------- | ---------------------------------------- |
| `tql`       | `string`                | TypeDB schema source (TypeQL text)       |
| `outputDir` | `string`                | Directory to write the four output files |
| `options`   | `GenerateModelsOptions` | Must include `native`; see below         |

`GenerateModelsOptions` is defined as:

```ts
type GenerateModelsOptions = { native: SchemaParserNative } & NamingOptions;
type NamingOptions = { implicitKeyAttributes?: string[] };
```

`SchemaParserNative` is the native module handle; `loadNative()` returns it.
`implicitKeyAttributes` lists attribute names that should be treated as `@key`
even when the schema does not annotate them — useful for convention-based key
fields such as `id`.

For the lower-level parse step, `parseSchema` is also exported:

```ts
parseSchema(tql: string, native: SchemaParserNative): TypeSchema
```

`TypeSchema` is the resolved schema value the Rust core produces. You rarely need
this directly; it is the intermediate form that `generateModels` consumes.

### Naming

Generated names are mechanical and match the Python generator exactly:

- Class names are **PascalCase** derived from the TypeDB type name:
  `parity-person` → `ParityPerson`, `company` → `Company`.
- Field and role keys are **snake\_case** derived from the attribute or role name:
  `parity-id` → `parity_id`, `parity-tag` → `parity_tag`.

Names are **not prettified** — no prefix-stripping, no pluralization. A generated
TypeScript model and its Python counterpart for the same schema produce
byte-identical descriptors; a cross-language parity test enforces this.

### Build integration

The generator writes files that import from the `@type-bridge/node` package
entrypoint, which resolves to the package's compiled runtime — so generated models
are runnable as-is once the package is installed.

## Packaging note

The published package ships the typed layer as **runnable JavaScript** plus type
declarations (`dist/index.js` + `dist/*.d.ts`) over the native module. The package
entrypoint exposes both the typed surface (`Entity`, `attr`, `field`, `Relation`,
`generateModels`, `parseSchema`, …) and the low-level facade; consumers `require`
or `import` it like any package. This guide documents the typed API surface, not
the build pipeline.
