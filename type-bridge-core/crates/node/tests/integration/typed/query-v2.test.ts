import assert from "node:assert/strict";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import type { ManagerConnection } from "../../../typescript/index.js";

type RuntimePackage = typeof import("../../../typescript/index.js");
type TypedRuntimePackage = typeof import("../../../typescript/typed/index.js");

const requirePackage = createRequire(path.join(process.cwd(), "typed-query-v2-live.cjs"));
const typeBridge = requirePackage("@type-bridge/node") as RuntimePackage;
const typedBridge = requirePackage("@type-bridge/node/typed") as TypedRuntimePackage;

const {
  Card,
  Entity,
  Key,
  Relation,
  attr,
  field,
  role,
} = typeBridge;
const { QuerySession, references } = typedBridge;

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");

const suffix = `typed-query-v2-${process.pid}-${Date.now()}`;
const partyType = `${suffix}-party`;
const employeeType = `${suffix}-employee`;
const contractorType = `${suffix}-contractor`;
const companyType = `${suffix}-company`;
const employmentType = `${suffix}-employment`;
const partyIdAttr = `${suffix}-party-id`;
const partyNameAttr = `${suffix}-party-name`;
const rankAttr = `${suffix}-rank`;
const specialtyAttr = `${suffix}-specialty`;
const companyIdAttr = `${suffix}-company-id`;
const companyNameAttr = `${suffix}-company-name`;
const employmentCodeAttr = `${suffix}-employment-code`;

class PartyId extends attr.String(partyIdAttr) {}
class PartyName extends attr.String(partyNameAttr) {}
class Rank extends attr.Integer(rankAttr) {}
class Specialty extends attr.String(specialtyAttr) {}
class CompanyId extends attr.String(companyIdAttr) {}
class CompanyName extends attr.String(companyNameAttr) {}
class EmploymentCode extends attr.String(employmentCodeAttr) {}

class Party extends Entity(partyType, {
  id: field(PartyId, Key),
  name: field(PartyName),
}) {}

class Employee extends Entity(
  employeeType,
  { rank: field(Rank) },
  { parent: Party },
) {}

class Contractor extends Entity(
  contractorType,
  { specialty: field(Specialty) },
  { parent: Party },
) {}

class Company extends Entity(companyType, {
  id: field(CompanyId, Key),
  name: field(CompanyName),
}) {}

class Employment extends Relation(employmentType, {
  code: field(EmploymentCode, Key),
  employee: role(Party, { cardinality: Card(1, 1) }),
  employer: role(Company, { cardinality: Card(1, 1) }),
}) {}

type FixtureThing = Employee | Contractor | Company | Employment;
type IdentityKind = "employee" | "contractor" | "company" | "employment";

interface IdentitySummary {
  readonly kind: IdentityKind;
  readonly iid: string;
}

describe("public ./typed selected-query integration", () => {
  const db = connectIntegration();
  defineSchema(db, schemaTypeql());
  const fixture = insertFixture(db);

  test("owned selected rows preserve exact/subtype constructors and order", () => {
    const session = registeredSession(db);
    const partyRefs = references(Party);

    const employee = session.var(Employee, "exact");
    const exactRows = session.query(employee).rows({
      limit: 10,
      orderBy: [employee.field(partyRefs.fields.name).asc()],
    });
    assert.deepEqual(
      exactRows.map((item) => item.name.value),
      ["Alice", "Carol"],
    );
    assert.ok(exactRows.every((item) => item instanceof Employee));
    assert.deepEqual(
      exactRows.map(identity),
      [identity(fixture.alice), identity(fixture.carol)],
    );

    const alice = session
      .query(employee)
      .where(employee.field(partyRefs.fields.id).eq(new PartyId("alice")))
      .one();
    assert.ok(alice instanceof Employee);
    assert.deepEqual(identity(alice), identity(fixture.alice));

    const exactParty = session.var(Party, "exact");
    assert.deepEqual(session.query(exactParty).rows({ limit: 10 }), []);

    const party = session.var(Party, "subtypes");
    const subtypeRows = session.query(party).rows({
      limit: 10,
      orderBy: [party.field(partyRefs.fields.name).asc()],
    });
    assert.deepEqual(
      subtypeRows.map((item) => item.name.value),
      ["Alice", "Bob", "Carol"],
    );
    assert.ok(subtypeRows[0] instanceof Employee);
    assert.ok(subtypeRows[1] instanceof Contractor);
    assert.ok(subtypeRows[2] instanceof Employee);
    assert.deepEqual(
      subtypeRows.map(identity),
      [identity(fixture.alice), identity(fixture.bob), identity(fixture.carol)],
    );
  });

  test("named collected pages deduplicate roots and values, order each collection, and hydrate roles", () => {
    const session = registeredSession(db);
    const party = session.var(Party, "subtypes");
    const employment = session.var(Employment, "exact");
    const company = session.var(Company, "exact");
    const partyRefs = references(Party);
    const employmentRefs = references(Employment);
    const companyRefs = references(Company);

    const query = session
      .queryNamed({
        party,
        employments: employment
          .collect()
          .orderBy(employment.field(employmentRefs.fields.code).asc()),
        companies: company
          .collect()
          .distinct()
          .orderBy(company.field(companyRefs.fields.name).asc()),
      })
      .where(
        employment.role(employmentRefs.roles.employee).connects(party),
        employment.role(employmentRefs.roles.employer).connects(company),
      );

    assert.equal(query.countBy(party), 2n);
    assert.equal(query.existsBy(party), true);

    const page = query.pageBy(party, {
      limit: 1,
      offset: 0,
      includeTotal: true,
      orderBy: [party.field(partyRefs.fields.name).asc()],
    });
    assert.equal(page.offset, 0);
    assert.equal(page.limit, 1);
    assert.equal(page.total, 2n);
    assert.equal(page.items.length, 1);
    assert.ok(Object.isFrozen(page));
    assert.ok(Object.isFrozen(page.items));

    const first = page.items[0]!;
    assert.ok(first.party instanceof Employee);
    assert.equal(first.party.name.value, "Alice");
    assert.deepEqual(
      first.employments.map((item) => item.code.value),
      ["E-1", "E-2", "E-3"],
    );
    assert.deepEqual(
      first.companies.map((item) => item.name.value),
      ["Acme", "Zulu"],
    );
    assert.deepEqual(
      first.employments.map(identity),
      [identity(fixture.e1), identity(fixture.e2), identity(fixture.e3)],
    );
    assert.deepEqual(
      first.companies.map(identity),
      [identity(fixture.acme), identity(fixture.zulu)],
    );

    const expectedEmployers = new Map([
      ["E-1", fixture.acme],
      ["E-2", fixture.zulu],
      ["E-3", fixture.acme],
    ]);
    for (const item of first.employments) {
      assert.ok(item instanceof Employment);
      assert.ok(item.employee instanceof Employee);
      assert.ok(item.employer instanceof Company);
      assert.deepEqual(identity(item.employee), identity(fixture.alice));
      assert.deepEqual(
        identity(item.employer),
        identity(expectedEmployers.get(item.code.value)!),
      );
      assert.ok(Object.isFrozen(item));
      assert.ok(Object.isFrozen(item.employee));
      assert.ok(Object.isFrozen(item.employer));
    }

    const parityReadySummary = Object.freeze({
      root: identity(first.party),
      employments: Object.freeze(first.employments.map(identity)),
      companies: Object.freeze(first.companies.map(identity)),
      rolePlayers: Object.freeze(first.employments.map((item) => {
        assert.ok(item.employee instanceof Employee);
        assert.ok(item.employer instanceof Company);
        return Object.freeze({
          employment: identity(item),
          employee: identity(item.employee),
          employer: identity(item.employer),
        });
      })),
    });
    assert.deepEqual(parityReadySummary, {
      root: identity(fixture.alice),
      employments: [identity(fixture.e1), identity(fixture.e2), identity(fixture.e3)],
      companies: [identity(fixture.acme), identity(fixture.zulu)],
      rolePlayers: [
        {
          employment: identity(fixture.e1),
          employee: identity(fixture.alice),
          employer: identity(fixture.acme),
        },
        {
          employment: identity(fixture.e2),
          employee: identity(fixture.alice),
          employer: identity(fixture.zulu),
        },
        {
          employment: identity(fixture.e3),
          employee: identity(fixture.alice),
          employer: identity(fixture.acme),
        },
      ],
    });
  });

  test("borrowed read contexts remain reusable across repeated count and exists terminals", () => {
    const tx = db.transaction("read");
    try {
      const session = registeredSession(tx);
      const party = session.var(Party, "subtypes");
      const query = session.query(party);

      assert.equal(query.countBy(party), 3n);
      assert.equal(query.existsBy(party), true);
      assert.equal(query.countBy(party), 3n);
      assert.equal(query.existsBy(party), true);
      assert.equal(tx.transactionType(), "read");
    } finally {
      tx.close();
    }
  });
});

function registeredSession(
  connection: ManagerConnection,
) {
  return new QuerySession(connection).registerModels(
    Employee,
    Contractor,
    Company,
    Employment,
  );
}

function identity(thing: FixtureThing): IdentitySummary {
  assert.ok(thing._iid !== null, "fixture and hydrated things must carry an IID");
  return Object.freeze({
    kind: identityKind(thing),
    iid: thing._iid,
  });
}

function identityKind(thing: FixtureThing): IdentityKind {
  if (thing instanceof Employee) return "employee";
  if (thing instanceof Contractor) return "contractor";
  if (thing instanceof Company) return "company";
  if (thing instanceof Employment) return "employment";
  throw new TypeError("unknown typed-query fixture constructor");
}

function connectIntegration() {
  typeBridge.ensureDatabase(address, database, { username, password, httpPort });
  return typeBridge.RustDatabase.connect(address, database, {
    username,
    password,
    httpPort,
  });
}

function defineSchema(db: ReturnType<typeof connectIntegration>, typeql: string): void {
  const tx = db.transaction("schema");
  try {
    tx.query(typeql);
    tx.commit();
  } catch (error) {
    tx.close();
    throw error;
  }
}

function insertFixture(db: ReturnType<typeof connectIntegration>) {
  const employeeManager = Employee.manager(db);
  const contractorManager = Contractor.manager(db);
  const companyManager = Company.manager(db);
  const employmentManager = Employment.manager(db);

  const alice = employeeManager.insert(new Employee({
    id: new PartyId("alice"),
    name: new PartyName("Alice"),
    rank: new Rank(7n),
  }));
  const carol = employeeManager.insert(new Employee({
    id: new PartyId("carol"),
    name: new PartyName("Carol"),
    rank: new Rank(9n),
  }));
  const bob = contractorManager.insert(new Contractor({
    id: new PartyId("bob"),
    name: new PartyName("Bob"),
    specialty: new Specialty("security"),
  }));
  const acme = companyManager.insert(new Company({
    id: new CompanyId("acme"),
    name: new CompanyName("Acme"),
  }));
  const zulu = companyManager.insert(new Company({
    id: new CompanyId("zulu"),
    name: new CompanyName("Zulu"),
  }));

  const insertEmployment = (
    code: string,
    employee: Employee | Contractor,
    employer: Company,
  ) => employmentManager.insert(new Employment({
    code: new EmploymentCode(code),
    employee,
    employer,
  }));

  const e1 = insertEmployment("E-1", alice, acme);
  const e2 = insertEmployment("E-2", alice, zulu);
  const e3 = insertEmployment("E-3", alice, acme);
  const e4 = insertEmployment("E-4", bob, acme);

  return Object.freeze({ alice, bob, carol, acme, zulu, e1, e2, e3, e4 });
}

function schemaTypeql(): string {
  return `define
attribute ${partyIdAttr}, value string;
attribute ${partyNameAttr}, value string;
attribute ${rankAttr}, value integer;
attribute ${specialtyAttr}, value string;
attribute ${companyIdAttr}, value string;
attribute ${companyNameAttr}, value string;
attribute ${employmentCodeAttr}, value string;
entity ${partyType}, owns ${partyIdAttr} @key, owns ${partyNameAttr}, plays ${employmentType}:employee;
entity ${employeeType} sub ${partyType}, owns ${rankAttr};
entity ${contractorType} sub ${partyType}, owns ${specialtyAttr};
entity ${companyType}, owns ${companyIdAttr} @key, owns ${companyNameAttr}, plays ${employmentType}:employer;
relation ${employmentType}, relates employee, relates employer, owns ${employmentCodeAttr} @key;
`;
}
