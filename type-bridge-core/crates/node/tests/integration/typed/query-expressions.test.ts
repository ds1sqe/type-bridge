import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import { Card, Entity, Key, Relation, attr, field, role } from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-query-expressions.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");

const suffix = `typed-query-${process.pid}-${Date.now()}`;

// Entity-query world.
const personType = `${suffix}-person`;
const idAttr = `${suffix}-id`;
const nameAttr = `${suffix}-name`;
const ageAttr = `${suffix}-age`;
const scoreAttr = `${suffix}-score`;

class Id extends attr.String(idAttr) {}
class PersonName extends attr.String(nameAttr) {}
class Age extends attr.Integer(ageAttr) {}
class Score extends attr.Double(scoreAttr) {}

class Person extends Entity(personType, {
  id: field(Id, Key),
  name: field(PersonName),
  age: field(Age),
  score: field(Score),
}) {}

// Relation-query world — fully separate types so entity queries above are not
// polluted by the relation fixtures' role players.
const relPersonType = `${suffix}-rel-person`;
const relCompanyType = `${suffix}-rel-company`;
const employmentType = `${suffix}-employment`;
const relIdAttr = `${suffix}-rel-id`;
const companyIdAttr = `${suffix}-company-id`;
const sinceAttr = `${suffix}-since`;

class RelId extends attr.String(relIdAttr) {}
class CompanyId extends attr.String(companyIdAttr) {}
class Since extends attr.Date(sinceAttr) {}

class RelPerson extends Entity(relPersonType, { id: field(RelId, Key) }) {}
class Company extends Entity(relCompanyType, { id: field(CompanyId, Key) }) {}
class Employment extends Relation(employmentType, {
  employee: role(RelPerson, { cardinality: Card(1, 1) }),
  employer: role(Company, { cardinality: Card(1, 1) }),
  since: field(Since),
}) {}

describe("typed entity query expressions", () => {
  const db = connectIntegration();
  defineSchema(db, entitySchemaTypeql());
  const manager = Person.manager(db);
  manager.insertMany([
    new Person({ id: new Id("alice"), name: new PersonName("Alice"), age: new Age(30n), score: new Score(95) }),
    new Person({ id: new Id("bob"), name: new PersonName("Bob"), age: new Age(40n), score: new Score(80) }),
    new Person({ id: new Id("carol"), name: new PersonName("Carol"), age: new Age(50n), score: new Score(90) }),
  ]);

  const names = (people: Person[]): string[] => people.map((p) => p.name.value).sort();

  test("comparison filter executes DB-side and hydrates typed instances", () => {
    const rows = manager.query().filter(Age.gte(new Age(40n))).all();
    assert.deepEqual(names(rows), ["Bob", "Carol"]);
    assert.ok(rows.every((p) => p instanceof Person && p._iid !== null));
  });

  test("string startsWith filter matches the anchored prefix", () => {
    assert.deepEqual(names(manager.query().filter(PersonName.startsWith("A")).all()), ["Alice"]);
  });

  test("and_ combines comparisons", () => {
    const rows = manager.query().filter(Age.gte(new Age(40n)).and_(Score.gte(new Score(90)))).all();
    assert.deepEqual(names(rows), ["Carol"]);
  });

  test("or_ executes through Rust, not a client scan", () => {
    const rows = manager
      .query()
      .filter(Age.eq(new Age(30n)).or_(Age.eq(new Age(50n))))
      .all();
    assert.deepEqual(names(rows), ["Alice", "Carol"]);
  });

  test("not_ excludes the matched row", () => {
    const rows = manager.query().filter(Age.eq(new Age(40n)).not_()).all();
    assert.deepEqual(names(rows), ["Alice", "Carol"]);
  });

  test("orderBy + offset + limit page DB-side", () => {
    const rows = manager.query().orderBy(Age.desc()).offset(1).limit(1).all();
    assert.equal(rows.length, 1);
    assert.equal(rows[0].name.value, "Bob");
  });

  test("first returns the leading row under a sort", () => {
    const first = manager.query().filter(Age.gte(new Age(40n))).orderBy(Age.asc()).first();
    assert.ok(first instanceof Person);
    assert.equal(first?.name.value, "Bob");
  });

  test("count and exists reflect the expression filter", () => {
    assert.equal(manager.query().filter(Score.gte(new Score(90))).count(), 2n);
    assert.equal(manager.query().filter(PersonName.eq(new PersonName("Nobody"))).exists(), false);
    assert.equal(manager.query().filter(PersonName.eq(new PersonName("Alice"))).exists(), true);
  });
});

describe("typed relation query expressions", () => {
  const db = connectIntegration();
  defineSchema(db, relationSchemaTypeql());
  const personManager = RelPerson.manager(db);
  const companyManager = Company.manager(db);
  const relationManager = Employment.manager(db);

  const alice = personManager.insert(new RelPerson({ id: new RelId("rel-alice") }));
  const acme = companyManager.insert(new Company({ id: new CompanyId("rel-acme") }));

  relationManager.insert(new Employment({ employee: alice, employer: acme, since: new Since("2026-01-01") }));
  relationManager.insert(new Employment({ employee: alice, employer: acme, since: new Since("2026-12-31") }));

  test("relation-owned attribute filter preserves role-player hydration", () => {
    const rows = relationManager.query().filter(Since.gte(new Since("2026-06-01"))).all();
    assert.equal(rows.length, 1);
    const [employment] = rows;
    assert.ok(employment instanceof Employment);
    assert.ok(employment._iid !== null);
    assert.equal(employment.since.value, "2026-12-31");
    assert.ok(employment.employee instanceof RelPerson);
    assert.ok(employment.employer instanceof Company);
    assert.equal(employment.employee.id.value, "rel-alice");
    assert.equal(employment.employer.id.value, "rel-acme");
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

function entitySchemaTypeql(): string {
  return `define
attribute ${idAttr}, value string;
attribute ${nameAttr}, value string;
attribute ${ageAttr}, value integer;
attribute ${scoreAttr}, value double;
entity ${personType}, owns ${idAttr} @key, owns ${nameAttr}, owns ${ageAttr}, owns ${scoreAttr};
`;
}

function relationSchemaTypeql(): string {
  return `define
attribute ${relIdAttr}, value string;
attribute ${companyIdAttr}, value string;
attribute ${sinceAttr}, value date;
entity ${relPersonType}, owns ${relIdAttr} @key, plays ${employmentType}:employee;
entity ${relCompanyType}, owns ${companyIdAttr} @key, plays ${employmentType}:employer;
relation ${employmentType}, relates employee, relates employer, owns ${sinceAttr};
`;
}
