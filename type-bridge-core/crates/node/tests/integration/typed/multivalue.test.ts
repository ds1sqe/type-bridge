import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
  Entity,
  Key,
  TypeFlags,
  attr,
  field,
} from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-multivalue.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";

// Per-run suffix isolates fixture types.
const suffix = `typed-mv-${process.pid}-${Date.now()}`;

// Parent attributes.
const partyIdAttr = `${suffix}-party-id`;
const partyNameAttr = `${suffix}-party-name`;

// Child-local attributes.
const personEmailAttr = `${suffix}-person-email`;
const personTagAttr = `${suffix}-person-tag`;

// Type names.
const partyType = `${suffix}-party`;
const personType = `${suffix}-person`;

// ---------------------------------------------------------------------------
// Typed model declarations
// ---------------------------------------------------------------------------

class PartyId extends attr.String(partyIdAttr) {}
class PartyName extends attr.String(partyNameAttr) {}
class PersonEmail extends attr.String(personEmailAttr) {}
class PersonTag extends attr.String(personTagAttr) {}

// Abstract parent — mirrors parity-party structure.
class Party extends Entity(TypeFlags({ name: partyType, abstract: true }), {
  id: field(PartyId, Key),
  name: field(PartyName).optional(),
}) {}

// Concrete child — inherits id + name, adds email (scalar) and tags (list).
// Card(0, 5): min=0 (optional), max=5.
class Person extends Entity(
  personType,
  {
    email: field(PersonEmail),
    tags: field(PersonTag).list(Card(0, 5)),
  },
  { parent: Party },
) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("typed multi-value (list) attribute CRUD", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());
  const manager = Person.manager(db);

  test("insert with 3 tags; read back yields Tag[] with set-semantics equality", () => {
    const inserted = manager.insert(
      new Person({
        id: new PartyId("mv-person-1"),
        email: new PersonEmail("p1@example.com"),
        tags: [new PersonTag("typescript"), new PersonTag("typedb"), new PersonTag("rust")],
      }),
    );
    assert.ok(inserted._iid !== null, "insert must set _iid");

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof Person, "hydrated must be a Person instance");
    assert.equal(hydrated._iid, inserted._iid, "_iid must be preserved");

    // Inherited scalar fields.
    assert.ok(hydrated.id instanceof PartyId);
    assert.equal(hydrated.id.value, "mv-person-1");
    assert.ok(hydrated.email instanceof PersonEmail);
    assert.equal(hydrated.email.value, "p1@example.com");

    // Multi-value list attribute: round-trips as PersonTag[].
    // TypeDB list attributes have SET semantics — the returned order is
    // unspecified. Assert equality as a sorted set, not an ordered array.
    assert.ok(Array.isArray(hydrated.tags), "tags must hydrate as an array");
    const tags = hydrated.tags as PersonTag[];
    assert.equal(tags.length, 3, "all 3 tags must round-trip");
    assert.ok(tags.every((t) => t instanceof PersonTag), "each element must carry PersonTag brand");
    const tagValues = tags.map((t) => t.value).sort();
    assert.deepEqual(tagValues, ["rust", "typedb", "typescript"], "tag values must match (sorted)");
  });

  test("insert without tags (absent optional list); read back yields undefined", () => {
    // Rule: an absent optional list field (Card(0,N)) reads back as `undefined`
    // when TypeDB returns zero tuples for that attribute name. The regrouping
    // logic only creates the array key when at least one tuple is present; if no
    // [attr_name, value] pair exists for the list attribute, the field is absent
    // from the hydrated object, so the constructor leaves it as `undefined`.
    const inserted = manager.insert(
      new Person({
        id: new PartyId("mv-person-2"),
        email: new PersonEmail("p2@example.com"),
        // tags omitted — optional list, Card(0,5).
      }),
    );
    assert.ok(inserted._iid !== null);

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof Person);
    assert.equal(hydrated.tags, undefined, "absent optional list must read back as undefined");
  });

  test("insert with empty tags array; read back yields undefined (no wire tuples emitted)", () => {
    // An empty Attr[] lowers to an empty JS array []. The NAPI insert parser
    // receives an empty array per field and emits zero `has` statements. TypeDB
    // therefore stores no attribute values for this field, so the read-back
    // returns no tuples — hydration never creates the array key, and the field
    // is undefined on the constructed instance. This is the same documented
    // rule as the absent case: zero wire tuples → undefined.
    const inserted = manager.insert(
      new Person({
        id: new PartyId("mv-person-3"),
        email: new PersonEmail("p3@example.com"),
        tags: [],
      }),
    );
    assert.ok(inserted._iid !== null);

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof Person);
    assert.equal(
      hydrated.tags,
      undefined,
      "empty-array insert emits no wire tuples; read-back must be undefined",
    );
  });

  test("scalar field alongside list field hydrates without corruption (Plan 08 regression guard)", () => {
    // Confirm the regrouping logic does not affect scalar fields that share the
    // same entity. The inherited `id` and `email` must each hydrate as a single
    // Attribute instance, not wrapped in an array.
    const inserted = manager.insert(
      new Person({
        id: new PartyId("mv-regression-1"),
        email: new PersonEmail("regression@example.com"),
        tags: [new PersonTag("guard")],
      }),
    );
    assert.ok(inserted._iid !== null);

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof Person);

    // Scalar fields must NOT be arrays.
    assert.ok(!Array.isArray(hydrated.id), "scalar id must not be an array");
    assert.ok(!Array.isArray(hydrated.email), "scalar email must not be an array");

    // List field must be an array.
    assert.ok(Array.isArray(hydrated.tags), "list tags must be an array");
    const tags = hydrated.tags as PersonTag[];
    assert.equal(tags.length, 1);
    assert.equal(tags[0].value, "guard");
  });
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function connectIntegration() {
  typeBridge.ensureDatabase(address, database, { username, password });
  return typeBridge.RustDatabase.connect(address, database, { username, password });
}

function defineSchema(db: ReturnType<typeof connectIntegration>, typeql: string): void {
  const tx = db.transaction("schema");
  try {
    tx.query(typeql);
    tx.commit();
  } catch (err) {
    tx.close();
    throw err;
  }
}

function schemaTypeql(): string {
  // TypeDB 3.x: @abstract on the parent, sub <parent> on the child.
  // @card(0..5) on the tag attribute in the child entity grants multi-value
  // ownership. The parent entity owns id @key and name (optional scalar).
  return `define
attribute ${partyIdAttr}, value string;
attribute ${partyNameAttr}, value string;
attribute ${personEmailAttr}, value string;
attribute ${personTagAttr}, value string;
entity ${partyType} @abstract, owns ${partyIdAttr} @key, owns ${partyNameAttr};
entity ${personType} sub ${partyType}, owns ${personEmailAttr}, owns ${personTagAttr} @card(0..5);
`;
}
