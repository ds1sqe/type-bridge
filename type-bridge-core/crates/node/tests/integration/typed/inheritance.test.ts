import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Entity,
  Key,
  TypeFlags,
  attr,
  field,
} from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-inheritance.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");

// Per-run suffix ensures fixture types never collide with other test runs or
// existing schema types (plan requirement: per-test unique type-name suffix).
const suffix = `typed-inh-${process.pid}-${Date.now()}`;

// Parent attribute names.
const partyIdAttr = `${suffix}-party-id`;
const partyNameAttr = `${suffix}-party-name`;

// Child-local attribute name.
const personEmailAttr = `${suffix}-person-email`;

// Type names.
const partyType = `${suffix}-party`;
const personType = `${suffix}-person`;

// ---------------------------------------------------------------------------
// Typed model declarations
// ---------------------------------------------------------------------------

class PartyId extends attr.String(partyIdAttr) {}
class PartyName extends attr.String(partyNameAttr) {}
class PersonEmail extends attr.String(personEmailAttr) {}

// Abstract parent — no parent itself, is_abstract: true.
class Party extends Entity(TypeFlags({ name: partyType, abstract: true }), {
  id: field(PartyId, Key),
  name: field(PartyName),
}) {}

// Concrete child — inherits id + name from Party and adds email.
// The merged schema is { id, name, email }.
class Person extends Entity(
  personType,
  {
    email: field(PersonEmail),
  },
  { parent: Party },
) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("typed entity manager CRUD over an inherited model", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());
  const manager = Person.manager(db);

  test("insert assigns _iid and both inherited and child-local fields are accessible", () => {
    const original = new Person({
      id: new PartyId("person-insert-1"),
      name: new PartyName("Alice"),
      email: new PersonEmail("alice@example.com"),
    });

    assert.equal(original._iid, null, "pre-insert: _iid must be null");

    const inserted = manager.insert(original);
    assert.equal(inserted, original, "insert returns the same instance");
    assert.ok(inserted._iid !== null, "insert must set _iid");

    // Inherited fields read with parent attribute brands.
    assert.ok(inserted.id instanceof PartyId, "id must carry PartyId brand");
    assert.equal(inserted.id.value, "person-insert-1");
    assert.ok(inserted.name instanceof PartyName, "name must carry PartyName brand");
    assert.equal(inserted.name.value, "Alice");

    // Child-local field.
    assert.ok(inserted.email instanceof PersonEmail, "email must carry PersonEmail brand");
    assert.equal(inserted.email.value, "alice@example.com");
  });

  test("getByIid round-trips inherited and child fields with brands and _iid preserved", () => {
    const inserted = manager.insert(
      new Person({
        id: new PartyId("person-get-1"),
        name: new PartyName("Bob"),
        email: new PersonEmail("bob@example.com"),
      }),
    );
    assert.ok(inserted._iid !== null);

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof Person, "hydrated must be a Person instance");
    assert.equal(hydrated._iid, inserted._iid, "_iid must be preserved");

    // Inherited fields hydrate with correct brands.
    assert.ok(hydrated.id instanceof PartyId, "inherited id must carry PartyId brand");
    assert.equal(hydrated.id.value, "person-get-1");
    assert.ok(hydrated.name instanceof PartyName, "inherited name must carry PartyName brand");
    assert.equal(hydrated.name.value, "Bob");

    // Child-local field.
    assert.ok(hydrated.email instanceof PersonEmail, "email must carry PersonEmail brand");
    assert.equal(hydrated.email.value, "bob@example.com");
  });

  test("update does not throw and re-read returns the entity", () => {
    const inserted = manager.insert(
      new Person({
        id: new PartyId("person-update-1"),
        name: new PartyName("Carol"),
        email: new PersonEmail("carol@example.com"),
      }),
    );
    assert.ok(inserted._iid !== null);

    // update() with the same instance is a valid round-trip: it replaces the
    // owned attributes (inherited + child) with the same values.
    manager.update(inserted);

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof Person);
    assert.equal(hydrated.name.value, "Carol", "inherited name must survive update");
    assert.equal(hydrated.email.value, "carol@example.com", "child-local email must survive update");
  });

  test("delete removes the entity; getByIid returns null afterward", () => {
    const inserted = manager.insert(
      new Person({
        id: new PartyId("person-delete-1"),
        name: new PartyName("Dave"),
        email: new PersonEmail("dave@example.com"),
      }),
    );
    assert.ok(inserted._iid !== null);

    manager.delete(inserted);
    assert.equal(
      manager.getByIid(inserted._iid),
      null,
      "entity must be gone after delete",
    );
  });

  test("scalar-only (non-inherited) model still hydrates correctly (Plan 08 regression guard)", () => {
    // After the hydration regrouping fix, confirm that a scalar-only entity
    // (no list fields, no parent) still hydrates single values without corruption.
    // This guards against the fix accidentally breaking the common case.
    const scalarIdAttr = `${suffix}-scalar-id`;
    const scalarTypeName = `${suffix}-scalar`;

    class ScalarId extends attr.String(scalarIdAttr) {}
    class Scalar extends Entity(scalarTypeName, { id: field(ScalarId, Key) }) {}

    const tx = db.transaction("schema");
    try {
      tx.query(`define attribute ${scalarIdAttr}, value string; entity ${scalarTypeName}, owns ${scalarIdAttr} @key;`);
      tx.commit();
    } catch (err) {
      tx.close();
      throw err;
    }

    const scalarManager = Scalar.manager(db);
    const s = scalarManager.insert(new Scalar({ id: new ScalarId("scalar-regression-1") }));
    assert.ok(s._iid !== null, "scalar insert must set _iid");

    const h = scalarManager.getByIid(s._iid);
    assert.ok(h instanceof Scalar, "hydrated instance must be a Scalar");
    assert.ok(h.id instanceof ScalarId, "scalar id must carry ScalarId brand");
    assert.equal(h.id.value, "scalar-regression-1", "scalar value must round-trip unchanged");
  });
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function connectIntegration() {
  typeBridge.ensureDatabase(address, database, { username, password, httpPort });
  return typeBridge.RustDatabase.connect(address, database, { username, password, httpPort });
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
  // TypeDB 3.x syntax: @abstract on the parent, sub <parent> on the child.
  return `define
attribute ${partyIdAttr}, value string;
attribute ${partyNameAttr}, value string;
attribute ${personEmailAttr}, value string;
entity ${partyType} @abstract, owns ${partyIdAttr} @key, owns ${partyNameAttr};
entity ${personType} sub ${partyType}, owns ${personEmailAttr};
`;
}
