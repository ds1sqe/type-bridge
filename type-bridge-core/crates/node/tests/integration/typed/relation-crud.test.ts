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
  buildRolePlayers,
  field,
  role,
} from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-relation-crud.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";

const suffix = `typed-relation-${process.pid}-${Date.now()}`;
const personType = `${suffix}-person`;
const companyType = `${suffix}-company`;
const emailType = `${suffix}-email`;
const employmentType = `${suffix}-employment`;
const personIdAttr = `${suffix}-person-id`;
const personNameAttr = `${suffix}-person-name`;
const companyIdAttr = `${suffix}-company-id`;
const companyNameAttr = `${suffix}-company-name`;
const emailIdAttr = `${suffix}-email-id`;
const subjectAttr = `${suffix}-subject`;
const sinceAttr = `${suffix}-since`;
const confidenceAttr = `${suffix}-confidence`;

class PersonId extends attr.String(personIdAttr) {}
class PersonName extends attr.String(personNameAttr) {}
class CompanyId extends attr.String(companyIdAttr) {}
class CompanyName extends attr.String(companyNameAttr) {}
class EmailId extends attr.String(emailIdAttr) {}
class Subject extends attr.String(subjectAttr) {}
class Since extends attr.Date(sinceAttr) {}
class Confidence extends attr.Integer(confidenceAttr) {}

class Person extends Entity(personType, {
  id: field(PersonId, Key),
  name: field(PersonName),
}) {}

class Company extends Entity(companyType, {
  id: field(CompanyId, Key),
  name: field(CompanyName),
}) {}

class Email extends Entity(emailType, {
  id: field(EmailId, Key),
  subject: field(Subject),
}) {}

class Employment extends Relation(employmentType, {
  employee: role(Person, { cardinality: Card(1, 1) }),
  employer: role(Company, { cardinality: Card(1, 1) }),
  evidence: role(Email, { cardinality: Card(1) }),
  since: field(Since),
  confidence: field(Confidence),
}) {}

describe("typed relation manager CRUD", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());

  test("insert by key fallback, read hydration, repeated role merge, and delete", () => {
    const personManager = Person.manager(db);
    const companyManager = Company.manager(db);
    const emailManager = Email.manager(db);
    const relationManager = Employment.manager(db);

    const alice = personManager.insert(
      new Person({ id: new PersonId("person-1"), name: new PersonName("Alice") }),
    );
    const acme = companyManager.insert(
      new Company({ id: new CompanyId("company-1"), name: new CompanyName("Acme") }),
    );
    const offer = emailManager.insert(
      new Email({ id: new EmailId("email-1"), subject: new Subject("Offer") }),
    );
    const contract = emailManager.insert(
      new Email({ id: new EmailId("email-2"), subject: new Subject("Contract") }),
    );

    const keyOnlyAlice = new Person({
      id: new PersonId(alice.id.value),
      name: new PersonName(alice.name.value),
    });
    const keyOnlyAcme = new Company({
      id: new CompanyId(acme.id.value),
      name: new CompanyName(acme.name.value),
    });
    const keyOnlyOffer = new Email({
      id: new EmailId(offer.id.value),
      subject: new Subject(offer.subject.value),
    });
    const roleInputs = buildRolePlayers(
      new Employment({
        employee: keyOnlyAlice,
        employer: keyOnlyAcme,
        evidence: [keyOnlyOffer, contract],
        since: new Since("2026-06-04"),
        confidence: new Confidence(87n),
      }),
      Employment.schema,
    );
    assert.equal(roleInputs[0].key_attr, personIdAttr);
    assert.equal(roleInputs[1].key_attr, companyIdAttr);
    assert.equal(roleInputs[2].key_attr, emailIdAttr);
    assert.equal(roleInputs[3].iid, contract._iid);

    const relation = new Employment({
      employee: keyOnlyAlice,
      employer: keyOnlyAcme,
      evidence: [keyOnlyOffer, contract],
      since: new Since("2026-06-04"),
      confidence: new Confidence(87n),
    });
    relationManager.insert(relation);
    assert.ok(relation._iid !== null);

    const hydrated = relationManager.getByIid(relation._iid);
    assert.ok(hydrated instanceof Employment);
    assert.equal(hydrated._iid, relation._iid);
    assert.ok(hydrated.employee instanceof Person);
    assert.ok(hydrated.employer instanceof Company);
    assert.equal(hydrated.employee.id.value, "person-1");
    assert.equal(hydrated.employer.id.value, "company-1");
    assert.ok(Array.isArray(hydrated.evidence));
    assert.equal(hydrated.evidence.length, 2);
    assert.ok(hydrated.evidence.every((item) => item instanceof Email));
    assert.deepEqual(
      hydrated.evidence.map((item) => item.id.value).sort(),
      ["email-1", "email-2"],
    );
    assert.equal(hydrated.since.value, "2026-06-04");
    assert.equal(hydrated.confidence.value, 87n);

    relationManager.delete(hydrated);
    assert.equal(relationManager.getByIid(relation._iid), null);
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
attribute ${personIdAttr}, value string;
attribute ${personNameAttr}, value string;
attribute ${companyIdAttr}, value string;
attribute ${companyNameAttr}, value string;
attribute ${emailIdAttr}, value string;
attribute ${subjectAttr}, value string;
attribute ${sinceAttr}, value date;
attribute ${confidenceAttr}, value integer;
entity ${personType}, owns ${personIdAttr} @key, owns ${personNameAttr}, plays ${employmentType}:employee;
entity ${companyType}, owns ${companyIdAttr} @key, owns ${companyNameAttr}, plays ${employmentType}:employer;
entity ${emailType}, owns ${emailIdAttr} @key, owns ${subjectAttr}, plays ${employmentType}:evidence;
relation ${employmentType}, relates employee, relates employer, relates evidence @card(0..), owns ${sinceAttr}, owns ${confidenceAttr};
`;
}
