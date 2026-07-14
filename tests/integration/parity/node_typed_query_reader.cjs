"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

const consumerRoot = fs.realpathSync(process.env.TYPE_BRIDGE_PACKED_CONSUMER_ROOT);
const sourcePackageRoot = fs.realpathSync(process.env.TYPE_BRIDGE_SOURCE_PACKAGE_ROOT);
const fixturePath = process.env.TYPE_BRIDGE_PARITY_TYPED_QUERY_FIXTURE;
if (!fixturePath) throw new Error("TYPE_BRIDGE_PARITY_TYPED_QUERY_FIXTURE is not set");

const requirePackage = createRequire(path.join(consumerRoot, "typed-query-parity.cjs"));
const resolvedRoot = fs.realpathSync(requirePackage.resolve("@type-bridge/node"));
const resolvedTyped = fs.realpathSync(requirePackage.resolve("@type-bridge/node/typed"));
const installedRoot = fs.realpathSync(
  path.join(consumerRoot, "node_modules", "@type-bridge", "node"),
);

function isWithin(candidate, root) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

for (const [name, resolved] of [["root", resolvedRoot], ["typed", resolvedTyped]]) {
  assert.ok(isWithin(resolved, installedRoot), `${name} import escaped packed install: ${resolved}`);
  assert.ok(!isWithin(resolved, sourcePackageRoot), `${name} import leaked to source: ${resolved}`);
}
assert.equal(fs.lstatSync(installedRoot).isSymbolicLink(), false);
assert.equal(fs.existsSync(path.join(installedRoot, "typescript")), false);
assert.equal(process.env.TYPE_BRIDGE_NODE_NATIVE_PATH, undefined);

const typeBridge = requirePackage("@type-bridge/node");
const typedBridge = requirePackage("@type-bridge/node/typed");
const contract = JSON.parse(fs.readFileSync(fixturePath, "utf8"));
const labels = contract.labels;
const expected = contract.expected;

const { Card, Entity, Key, Relation, attr, field, role } = typeBridge;
const { QuerySession, references } = typedBridge;

class PersonId extends attr.String(labels.person_id) {}
class PersonName extends attr.String(labels.person_name) {}
class Rank extends attr.Integer(labels.rank) {}
class Specialty extends attr.String(labels.specialty) {}
class CompanyId extends attr.String(labels.company_id) {}
class CompanyName extends attr.String(labels.company_name) {}
class EmploymentCode extends attr.String(labels.employment_code) {}

class Person extends Entity(labels.person, {
  person_id: field(PersonId, Key),
  name: field(PersonName),
}) {}

class Employee extends Entity(labels.employee, { rank: field(Rank) }, { parent: Person }) {}
class Contractor extends Entity(
  labels.contractor,
  { specialty: field(Specialty) },
  { parent: Person },
) {}

class Company extends Entity(labels.company, {
  company_id: field(CompanyId, Key),
  name: field(CompanyName),
}) {}

class Employment extends Relation(labels.employment, {
  code: field(EmploymentCode, Key),
  employee: role(Person, { cardinality: Card(1, 1) }),
  employer: role(Company, { cardinality: Card(1, 1) }),
}) {}

function registeredSession(connection) {
  return new QuerySession(connection).registerModels(Employee, Contractor, Company, Employment);
}

function graphQuery(connection) {
  const session = registeredSession(connection);
  const person = session.var(Person, "subtypes");
  const employment = session.var(Employment, "exact");
  const company = session.var(Company, "exact");
  const personRefs = references(Person);
  const employmentRefs = references(Employment);
  const companyRefs = references(Company);
  const query = session
    .queryNamed({
      person,
      employments: employment
        .collect()
        .orderBy(employment.field(employmentRefs.fields.code).asc()),
      companies: company
        .collect()
        .distinct()
        .orderBy(company.field(companyRefs.fields.name).asc()),
    })
    .where(
      employment.role(employmentRefs.roles.employee).connects(person),
      employment.role(employmentRefs.roles.employer).connects(company),
    );
  return { person, personRefs, query };
}

function identity(thing) {
  assert.equal(typeof thing._iid, "string", "hydrated things must carry an IID");
  let kind;
  if (thing instanceof Employee) kind = "employee";
  else if (thing instanceof Contractor) kind = "contractor";
  else if (thing instanceof Company) kind = "company";
  else if (thing instanceof Employment) kind = "employment";
  else throw new TypeError("unknown typed-query parity constructor");
  return { kind, iid: thing._iid };
}

function summarize(db) {
  const { person, personRefs, query } = graphQuery(db);
  const page = query.pageBy(person, {
    limit: 10,
    includeTotal: true,
    orderBy: [person.field(personRefs.fields.name).asc()],
  });
  const semanticPage = query.pageBy(person, {
    limit: 1,
    includeTotal: true,
    orderBy: [person.field(personRefs.fields.name).asc()],
  });

  assert.deepEqual(page.items.map((row) => row.person.person_id.value), expected.root_keys);
  for (const row of page.items) {
    const rootKey = row.person.person_id.value;
    assert.deepEqual(
      row.employments.map((item) => item.code.value),
      expected.employment_keys[rootKey],
    );
    assert.deepEqual(
      row.companies.map((item) => item.company_id.value),
      expected.company_keys[rootKey],
    );
  }

  const count = query.countBy(person);
  const exists = query.existsBy(person);
  assert.equal(count, BigInt(expected.total));
  assert.equal(exists, true);
  const semanticProjection = {
    source_fixture: contract.semantic_corpus_projection.source_fixture,
    distinct_roots: page.items.map((row) => `person:${row.person.person_id.value}`),
    page_by_person_offset_0_limit_1: {
      roots: semanticPage.items.map((row) => `person:${row.person.person_id.value}`),
      offset: Number(semanticPage.offset),
      limit: Number(semanticPage.limit),
      total: Number(semanticPage.total),
    },
    alice_collect_count: page.items[0].employments.length,
    alice_collect_distinct_count: page.items[0].companies.length,
    count_by_person: Number(count),
    exists_by_person: exists,
  };
  assert.deepEqual(semanticProjection, contract.semantic_corpus_projection);

  const tx = db.transaction("read");
  let borrowed;
  try {
    const borrowedGraph = graphQuery(tx);
    borrowed = {
      counts: [
        Number(borrowedGraph.query.countBy(borrowedGraph.person)),
        Number(borrowedGraph.query.countBy(borrowedGraph.person)),
      ],
      exists: [
        borrowedGraph.query.existsBy(borrowedGraph.person),
        borrowedGraph.query.existsBy(borrowedGraph.person),
      ],
    };
    assert.equal(tx.transactionType(), "read");
  } finally {
    tx.close();
  }

  return {
    version: contract.version,
    page: {
      offset: Number(page.offset),
      limit: Number(page.limit),
      total: Number(page.total),
      items: page.items.map((row) => ({
        person: identity(row.person),
        employments: row.employments.map(identity),
        companies: row.companies.map(identity),
        role_players: row.employments.map((item) => ({
          employment: identity(item),
          employee: identity(item.employee),
          employer: identity(item.employer),
        })),
      })),
    },
    count: Number(count),
    exists,
    semantic_corpus_projection: semanticProjection,
    borrowed,
  };
}

const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
const database = process.env.TYPE_BRIDGE_PARITY_DATABASE ?? "type_bridge_test";
const username = process.env.TYPEDB_USERNAME ?? "admin";
const password = process.env.TYPEDB_PASSWORD ?? "password";
const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");
const db = typeBridge.RustDatabase.connect(address, database, {
  username,
  password,
  httpPort,
});

process.stdout.write(JSON.stringify({ artifact: "packed", summary: summarize(db) }));
