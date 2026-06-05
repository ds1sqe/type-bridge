import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import { Entity, Key, agg, attr, field } from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-query-aggregates.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";

const suffix = `typed-agg-${process.pid}-${Date.now()}`;
const personType = `${suffix}-person`;
const idAttr = `${suffix}-id`;
const ageAttr = `${suffix}-age`;
const deptAttr = `${suffix}-dept`;

class Id extends attr.String(idAttr) {}
class Age extends attr.Integer(ageAttr) {}
class Dept extends attr.String(deptAttr) {}

class Person extends Entity(personType, {
  id: field(Id, Key),
  age: field(Age),
  dept: field(Dept),
}) {}

describe("typed aggregate and group-by queries", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());
  const manager = Person.manager(db);
  manager.insertMany([
    new Person({ id: new Id("a"), age: new Age(30n), dept: new Dept("eng") }),
    new Person({ id: new Id("b"), age: new Age(40n), dept: new Dept("eng") }),
    new Person({ id: new Id("c"), age: new Age(50n), dept: new Dept("sales") }),
  ]);

  test("count and avg normalize to user-facing result keys", () => {
    const summary = manager.query().aggregate(agg.count(), Age.avg());
    assert.equal(summary.count, 3);
    assert.equal(summary[`avg_${ageAttr}`], 40);
  });

  test("aggregate honors the expression filter", () => {
    const summary = manager.query().filter(Age.gte(new Age(40n))).aggregate(agg.count());
    assert.equal(summary.count, 2);
  });

  test("median and std reduce numeric attributes DB-side", () => {
    const summary = manager.query().aggregate(Age.median(), Age.std());
    assert.equal(summary[`median_${ageAttr}`], 40);
    assert.ok(Math.abs((summary[`std_${ageAttr}`] as number) - 10) < 1e-6);
  });

  test("count over an empty match set normalizes to zero", () => {
    const summary = manager.query().filter(Age.gt(new Age(100n))).aggregate(agg.count());
    assert.equal(summary.count, 0);
  });

  test("groupBy returns one normalized row per group", () => {
    const groups = manager.query().groupBy(Dept).aggregate(agg.count());
    const byDept = new Map(groups.map((g) => [g[deptAttr] as string, g.count]));
    assert.equal(byDept.get("eng"), 2);
    assert.equal(byDept.get("sales"), 1);
    assert.equal(groups.length, 2);
  });
});

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
  return `define
attribute ${idAttr}, value string;
attribute ${ageAttr}, value integer;
attribute ${deptAttr}, value string;
entity ${personType}, owns ${idAttr} @key, owns ${ageAttr}, owns ${deptAttr};
`;
}
