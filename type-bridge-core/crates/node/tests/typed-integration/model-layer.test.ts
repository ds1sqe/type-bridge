import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
  Entity,
  Key,
  Relation,
  Unique,
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
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");

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

  test("typed registry schemaInfo preserves attribute type annotations in live define", () => {
    const annotatedType = `${suffix}-annotated`;
    const rootAttr = `${suffix}-root-code`;
    const codeAttr = `${suffix}-code`;
    const stateAttr = `${suffix}-state`;

    class AnnotatedRootCode extends attr.String(rootAttr, { abstract: true }) {}
    class AnnotatedCode extends attr.String(codeAttr, {
      parent: AnnotatedRootCode,
      regex: "^[A-Z]{2}$",
    }) {}
    class AnnotatedState extends attr.String(stateAttr, {
      values: ["open", "closed"],
    }) {}
    class AnnotatedEntity extends Entity(annotatedType, {
      code: field(AnnotatedCode, Key),
      state: field(AnnotatedState),
    }) {}

    const descriptor = AnnotatedEntity.descriptor();
    const registry = new typeBridge.DescriptorRegistry();
    registry.registerEntity(descriptor);
    const typeql = typeBridge.generateDefineBlock(registry.schemaInfo());

    assert.ok(
      typeql.includes(`attribute ${rootAttr} @abstract, value string;`),
      "unowned abstract parent attribute must be emitted",
    );
    assert.ok(
      typeql.includes(`attribute ${codeAttr} sub ${rootAttr}, value string @regex("^[A-Z]{2}$");`),
      "child attribute regex and parent must be emitted",
    );
    assert.ok(
      typeql.includes(`attribute ${stateAttr}, value string @values("open", "closed");`),
      "allowed values must be emitted",
    );

    defineSchema(db, typeql);
    const manager = db.entityManager(descriptor);
    manager.insert({
      code: typeBridge.string("AB"),
      state: typeBridge.string("open"),
    });

    assert.throws(() =>
      manager.insert({
        code: typeBridge.string("bad"),
        state: typeBridge.string("open"),
      }),
    );
    assert.throws(() =>
      manager.insert({
        code: typeBridge.string("CD"),
        state: typeBridge.string("stale"),
      }),
    );
  });

  test("typed registry preserves optional and required unique cardinalities", () => {
    const probeType = `${suffix}-unique-probe`;
    const emailAttr = `${suffix}-email`;
    const handleAttr = `${suffix}-handle`;
    const aliasAttr = `${suffix}-alias`;

    class Email extends attr.String(emailAttr) {}
    class Handle extends attr.String(handleAttr) {}
    class Alias extends attr.String(aliasAttr) {}
    class UniqueProbe extends Entity(probeType, {
      email: field(Email, Unique).optional(),
      handle: field(Handle, Unique),
      aliases: field(Alias, Unique).list(Card(0, 3)),
    }) {}

    const registry = new typeBridge.DescriptorRegistry();
    registry.registerEntity(UniqueProbe.descriptor());
    const typeql = typeBridge.generateDefineBlock(registry.schemaInfo());

    assert.ok(typeql.includes(`owns ${emailAttr} @unique @card(0..1)`));
    assert.ok(typeql.includes(`owns ${handleAttr} @unique @card(1..1)`));
    assert.ok(typeql.includes(`owns ${aliasAttr} @unique @card(0..3)`));
    defineSchema(db, typeql);
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
attribute ${sinceAttr}, value date;
entity ${personType}, owns ${idAttr} @key, owns ${nameAttr}, plays ${employmentType}:employee;
entity ${companyType}, owns ${idAttr} @key, owns ${nameAttr}, plays ${employmentType}:employer;
relation ${employmentType}, relates employee, relates employer, owns ${sinceAttr};
`;
}

function rowAttribute(row: DynamicEntityRow, attrName: string): RuntimeAttributeValue | undefined {
  return row.attributes.find(([name]) => name === attrName)?.[1];
}
