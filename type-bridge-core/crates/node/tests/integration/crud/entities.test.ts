/**
 * Entity CRUD integration — exercises insert, get, update, putMany, count,
 * aggregate, groupByAggregate, and deleteByIid through the public package
 * surface.  Mirrors node_entity_crud_against_typedb in entity_crud_integration.rs.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";

import {
  connectIntegration,
  defineSchema,
  newCrudSchema,
  crudSchemaTypeql,
  personDescriptor,
  rowAttribute,
  string,
  long,
  double,
} from "../common/index.ts";

describe("entity CRUD", () => {
  const db = connectIntegration();
  const s = newCrudSchema("entity");
  defineSchema(db, crudSchemaTypeql(s));
  const manager = db.entityManager(personDescriptor(s));

  test("insert returns a non-empty IID", () => {
    const iid = manager.insert({ name: string("Alice"), age: long(30n), score: double(88.5) });
    assert.ok(iid.length > 0, "IID should be non-empty");
  });

  test("get with equality filter returns matching rows", () => {
    manager.insert({ name: string("FilterTarget"), age: long(25n) });
    const rows = manager.get({ name: string("FilterTarget") });
    assert.equal(rows.length, 1);
    assert.deepEqual(rowAttribute(rows[0], s.nameAttr), { String: "FilterTarget" });
  });

  test("update by IID changes attributes", () => {
    const iid = manager.insert({ name: string("UpdateMe"), age: long(10n) });
    manager.update({ name: string("UpdateMe"), age: long(11n) }, iid);
    const row = manager.getByIid(iid);
    assert.ok(row !== null, "row should still exist after update");
    assert.deepEqual(rowAttribute(row!, s.ageAttr), { Long: "11" });
  });

  test("putMany inserts two entities and returns two IIDs", () => {
    const iids = manager.putMany([
      { name: string("Batch1"), age: long(40n), score: double(70.0) },
      { name: string("Batch2"), age: long(50n), score: double(80.0) },
    ]);
    assert.equal(iids.length, 2);
  });

  test("count returns the current number of entities", () => {
    // At least the rows from earlier subtests are present.
    const c = manager.count();
    assert.ok(c >= 1n, `count should be at least 1, got ${c}`);
  });

  test("aggregate with count function returns a numeric value", () => {
    const result = manager.aggregate([
      { result_key: "count", function: "count", attr_name: null },
    ]) as Array<Record<string, { value: number }>>;
    assert.ok(result.length >= 1);
    assert.ok(typeof result[0]["count"]?.value === "number", "count result should be a number");
  });

  test("groupByAggregate returns one group per distinct name", () => {
    // Insert known-distinct names so the group count is predictable.
    const s2 = newCrudSchema("entity-group");
    defineSchema(db, crudSchemaTypeql(s2));
    const m2 = db.entityManager(personDescriptor(s2));
    m2.putMany([
      { name: string("G1"), age: long(1n) },
      { name: string("G2"), age: long(2n) },
      { name: string("G3"), age: long(3n) },
    ]);
    const groups = m2.groupByAggregate(
      ["name"],
      [{ result_key: "count", function: "count", attr_name: null }],
    ) as unknown[];
    assert.equal(groups.length, 3);
  });

  test("deleteByIid removes the entity", () => {
    const iid = manager.insert({ name: string("DeleteMe"), age: long(99n) });
    manager.deleteByIid(iid);
    const row = manager.getByIid(iid);
    assert.equal(row, null, "entity should be null after deletion");
  });
});
