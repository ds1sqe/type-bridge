/**
 * Filter and lookup integration — exercises equality, comparison, and role-
 * player filters for both entity and relation managers.
 *
 * Mirrors node_entity_filters_and_lookup_against_typedb and
 * node_relation_filters_and_role_lookup_against_typedb.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";

import {
  connectIntegration,
  defineSchema,
  newCrudSchema,
  crudSchemaTypeql,
  personDescriptor,
  companyDescriptor,
  employmentDescriptor,
  rowAttribute,
  string,
  long,
  double,
  date,
} from "../common/index.ts";

const db = connectIntegration();

describe("entity filters and lookup", () => {
  const s = newCrudSchema("filters");
  defineSchema(db, crudSchemaTypeql(s));
  const manager = db.entityManager(personDescriptor(s));

  manager.insertMany([
    { name: string("Alice"), age: long(30n), score: double(91.25) },
    { name: string("Bob"), age: long(40n), score: double(91.25) },
    { name: string("Carol"), age: long(50n), score: double(91.25) },
  ]);

  test("equality filter returns exactly one match", () => {
    const rows = manager.get({ age: long(40n) });
    assert.equal(rows.length, 1);
    assert.deepEqual(rowAttribute(rows[0], s.nameAttr), { String: "Bob" });
  });

  test("comparison filter >= returns multiple matches", () => {
    const rows = manager.get([
      { attr_name: "age", operator: ">=", value: long(40n) },
    ]);
    assert.equal(rows.length, 2, "age >= 40 should match Bob and Carol");
  });

  test("count with comparison filter counts correctly", () => {
    const c = manager.count([
      { attr_name: "score", operator: ">", value: double(90.0) },
    ]);
    assert.equal(c, 3n, "score > 90 should match all three");
  });

  test("lookup for non-existent entity returns empty array", () => {
    const rows = manager.get({ name: string("Nobody") });
    assert.equal(rows.length, 0);
  });
});

describe("relation attribute and role-player filters", () => {
  const s = newCrudSchema("rel-filters");
  defineSchema(db, crudSchemaTypeql(s));

  const personMgr = db.entityManager(personDescriptor(s));
  const companyMgr = db.entityManager(companyDescriptor(s));
  const relMgr = db.relationManager(employmentDescriptor(s));

  const aliceIid = personMgr.insert({ name: string("Alice"), age: long(30n) });
  const bobIid = personMgr.insert({ name: string("Bob"), age: long(40n) });
  const acmeIid = companyMgr.insert({ name: string("Acme") });

  const aliceRoles = [
    { role_name: "employee", player_type_name: s.personType, iid: aliceIid },
    { role_name: "employer", player_type_name: s.companyType, iid: acmeIid },
  ];
  const bobRoles = [
    { role_name: "employee", player_type_name: s.personType, iid: bobIid },
    { role_name: "employer", player_type_name: s.companyType, iid: acmeIid },
  ];

  relMgr.insert({ since: date("2026-05-27") }, aliceRoles);
  relMgr.insert({ since: date("2026-05-28") }, bobRoles);

  test("relation attribute filter returns one match", () => {
    const rows = relMgr.get({ since: date("2026-05-27") });
    assert.equal(rows.length, 1);
  });

  test("role-player filter returns the matching relation", () => {
    const rows = relMgr.getWithRolePlayers(null, bobRoles);
    assert.equal(rows.length, 1);
    assert.deepEqual(rowAttribute(rows[0], s.sinceAttr), { Date: "2026-05-28" });
  });
});
