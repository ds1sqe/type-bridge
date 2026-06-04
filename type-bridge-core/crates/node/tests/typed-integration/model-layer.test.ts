import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
  Entity,
  Key,
  Relation,
  attr,
  field,
  role,
  type DynamicEntityRow,
  type RuntimeAttributeValue,
} from "../../typescript/index.js";

type RuntimePackage = typeof import("../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-integration.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";

const suffix = `typed-model-${process.pid}-${Date.now()}`;
const personType = `${suffix}-person`;
const companyType = `${suffix}-company`;
const employmentType = `${suffix}-employment`;
const idAttr = `${suffix}-id`;
const nameAttr = `${suffix}-name`;
const sinceAttr = `${suffix}-since`;

class Id extends attr.String(idAttr) {}
class Name extends attr.String(nameAttr) {}
class Since extends attr.Date(sinceAttr) {}

class Person extends Entity(personType, {
  id: field(Id, Key),
  name: field(Name),
}) {}

class Company extends Entity(companyType, {
  id: field(Id, Key),
  name: field(Name),
}) {}

class Employment extends Relation(employmentType, {
  employee: role(Person, { cardinality: Card(1, 1) }),
  employer: role(Company, { cardinality: Card(1, 1) }),
  since: field(Since),
}) {}

describe("typed model layer integration", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());

  test("typed entity descriptors drive dynamic entity managers", () => {
    const company = new Company({
      id: new Id("company-1"),
      name: new Name("Typed Company"),
    });
    const manager = db.entityManager(Company.descriptor());

    const iid = manager.insert({
      id: typeBridge.string(company.id.value),
      name: typeBridge.string(company.name.value),
    });

    const row = manager.getByIid(iid);
    assert.ok(row !== null, "typed descriptor insert should create a row");
    assert.deepEqual(rowAttribute(row!, idAttr), { String: "company-1" });
    assert.deepEqual(rowAttribute(row!, nameAttr), { String: "Typed Company" });
  });

  test("typed relation descriptors drive dynamic relation managers", () => {
    const person = new Person({
      id: new Id("person-1"),
      name: new Name("Typed Person"),
    });
    const company = new Company({
      id: new Id("company-2"),
      name: new Name("Typed Employer"),
    });

    db.entityManager(Person.descriptor()).insert({
      id: typeBridge.string(person.id.value),
      name: typeBridge.string(person.name.value),
    });
    db.entityManager(Company.descriptor()).insert({
      id: typeBridge.string(company.id.value),
      name: typeBridge.string(company.name.value),
    });

    const manager = db.relationManager(Employment.descriptor());
    const iid = manager.insert(
      { since: typeBridge.date("2026-06-04") },
      [
        {
          role_name: "employee",
          player_type_name: Person.typeName,
          key_attr: idAttr,
          key_value: typeBridge.string(person.id.value),
        },
        {
          role_name: "employer",
          player_type_name: Company.typeName,
          key_attr: idAttr,
          key_value: typeBridge.string(company.id.value),
        },
      ],
    );

    assert.ok(iid.length > 0, "typed relation descriptor insert should return an IID");
    const rows = manager.get({ since: typeBridge.date("2026-06-04") });
    assert.equal(rows.length, 1);
    assert.deepEqual(rowAttribute(rows[0], sinceAttr), { Date: "2026-06-04" });
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
attribute ${nameAttr}, value string;
attribute ${sinceAttr}, value date;
entity ${personType}, owns ${idAttr} @key, owns ${nameAttr}, plays ${employmentType}:employee;
entity ${companyType}, owns ${idAttr} @key, owns ${nameAttr}, plays ${employmentType}:employer;
relation ${employmentType}, relates employee, relates employer, owns ${sinceAttr};
`;
}

function rowAttribute(row: DynamicEntityRow, attrName: string): RuntimeAttributeValue | undefined {
  return row.attributes.find(([name]) => name === attrName)?.[1];
}
