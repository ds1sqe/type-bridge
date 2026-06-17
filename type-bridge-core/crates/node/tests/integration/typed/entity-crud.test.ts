import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import { Entity, Key, attr, field } from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-entity-crud.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");

const suffix = `typed-entity-${process.pid}-${Date.now()}`;
const allType = `${suffix}-all`;
const idAttr = `${suffix}-id`;
const nameAttr = `${suffix}-name`;
const ageAttr = `${suffix}-age`;
const scoreAttr = `${suffix}-score`;
const activeAttr = `${suffix}-active`;
const birthdayAttr = `${suffix}-birthday`;
const loginAtAttr = `${suffix}-login-at`;
const seenAtAttr = `${suffix}-seen-at`;
const balanceAttr = `${suffix}-balance`;
const sessionLengthAttr = `${suffix}-session-length`;

class Id extends attr.String(idAttr) {}
class Name extends attr.String(nameAttr) {}
class Age extends attr.Integer(ageAttr) {}
class Score extends attr.Double(scoreAttr) {}
class Active extends attr.Boolean(activeAttr) {}
class Birthday extends attr.Date(birthdayAttr) {}
class LoginAt extends attr.DateTime(loginAtAttr) {}
class SeenAt extends attr.DateTimeTZ(seenAtAttr) {}
class Balance extends attr.Decimal(balanceAttr) {}
class SessionLength extends attr.Duration(sessionLengthAttr) {}

class AllPrimitive extends Entity(allType, {
  id: field(Id, Key),
  name: field(Name),
  age: field(Age),
  score: field(Score),
  active: field(Active),
  birthday: field(Birthday),
  loginAt: field(LoginAt),
  seenAt: field(SeenAt),
  balance: field(Balance),
  sessionLength: field(SessionLength),
}) {}

describe("typed entity manager CRUD", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());

  test("insert, getByIid, get, all, count, and delete hydrate typed instances", () => {
    const manager = AllPrimitive.manager(db);
    const original = new AllPrimitive({
      id: new Id("entity-1"),
      name: new Name("Alice"),
      age: new Age(9223372036854775807n),
      score: new Score(91.25),
      active: new Active(true),
      birthday: new Birthday("1990-01-02"),
      loginAt: new LoginAt("2026-05-27T10:30:00"),
      seenAt: new SeenAt("2026-05-27T10:30:00+00:00"),
      balance: new Balance("1234.56"),
      sessionLength: new SessionLength("PT2H30M"),
    });

    const inserted = manager.insert(original);
    assert.equal(inserted, original);
    assert.ok(inserted._iid !== null);

    const hydrated = manager.getByIid(inserted._iid);
    assert.ok(hydrated instanceof AllPrimitive);
    assert.equal(hydrated._iid, inserted._iid);
    assert.equal(hydrated.id.value, original.id.value);
    assert.equal(hydrated.name.value, original.name.value);
    assert.equal(hydrated.age.value, original.age.value);
    assert.equal(hydrated.score.value, original.score.value);
    assert.equal(hydrated.active.value, original.active.value);
    assert.equal(hydrated.birthday.value, original.birthday.value);
    assert.ok(hydrated.loginAt.value.startsWith("2026-05-27T10:30:00"));
    assert.ok(hydrated.seenAt.value.startsWith("2026-05-27T10:30:00"));
    assert.equal(Number.parseFloat(hydrated.balance.value), 1234.56);
    assert.equal(hydrated.sessionLength.value, original.sessionLength.value);

    const filtered = manager.get({ id: new Id("entity-1") });
    assert.equal(filtered.length, 1);
    assert.ok(filtered[0].name instanceof Name);
    assert.equal(filtered[0]._iid, inserted._iid);

    assert.ok(manager.all().some((item) => item._iid === inserted._iid));
    assert.ok(manager.count() >= 1n);

    manager.update(inserted);
    manager.delete(inserted);
    assert.equal(manager.getByIid(inserted._iid), null);
  });
});

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
  return `define
attribute ${idAttr}, value string;
attribute ${nameAttr}, value string;
attribute ${ageAttr}, value integer;
attribute ${scoreAttr}, value double;
attribute ${activeAttr}, value boolean;
attribute ${birthdayAttr}, value date;
attribute ${loginAtAttr}, value datetime;
attribute ${seenAtAttr}, value datetime-tz;
attribute ${balanceAttr}, value decimal;
attribute ${sessionLengthAttr}, value duration;
entity ${allType}, owns ${idAttr} @key, owns ${nameAttr}, owns ${ageAttr}, owns ${scoreAttr}, owns ${activeAttr}, owns ${birthdayAttr}, owns ${loginAtAttr}, owns ${seenAtAttr}, owns ${balanceAttr}, owns ${sessionLengthAttr};
`;
}
