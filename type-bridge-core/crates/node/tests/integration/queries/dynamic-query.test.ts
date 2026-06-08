/**
 * Dynamic expression-tree query integration — exercises the Phase 2 NAPI query
 * seam (`queryJson` / `queryCountJson`) directly, before the typed layer wraps
 * it. Confirms an OR-filtered + sorted + limited spec executes DB-side and
 * returns deterministic rows.
 *
 * Query spec comparison values use the same precision-safe `{ value_type, value }`
 * encoding as CRUD filters (the `long`/`double` builders), so `long` keeps full
 * i64 precision — see DynamicExpr in typescript/index.ts.
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
import type { DynamicQuerySpec } from "../../../typescript/index.ts";

const db = connectIntegration();

describe("dynamic expression-tree query seam", () => {
  const s = newCrudSchema("dynamic-query");
  defineSchema(db, crudSchemaTypeql(s));
  const manager = db.entityManager(personDescriptor(s));

  manager.insertMany([
    { name: string("Alice"), age: long(30n), score: double(91.25) },
    { name: string("Bob"), age: long(40n), score: double(91.25) },
    { name: string("Carol"), age: long(50n), score: double(91.25) },
  ]);

  // OR(age == 30, age == 50) matches Alice and Carol.
  const orSpec: DynamicQuerySpec = {
    expr: [
      {
        kind: "or",
        exprs: [
          { kind: "compare", attr_name: s.ageAttr, operator: "eq", value: long(30n) },
          { kind: "compare", attr_name: s.ageAttr, operator: "eq", value: long(50n) },
        ],
      },
    ],
  };

  test("OR-filtered query matches both branches", () => {
    const rows = manager.query(orSpec);
    assert.equal(rows.length, 2, "age == 30 OR age == 50 should match Alice and Carol");
  });

  test("OR-filtered count matches both branches", () => {
    assert.equal(manager.queryCount(orSpec), 2n);
  });

  test("OR + sort desc + limit returns the single top DB-side row", () => {
    const rows = manager.query({
      ...orSpec,
      sort: [{ kind: "attribute", attr_name: s.ageAttr, direction: "Desc" }],
      limit: 1,
    });
    assert.equal(rows.length, 1, "limit 1 should trim to a single row");
    assert.deepEqual(rowAttribute(rows[0], s.ageAttr), { Long: "50" }, "sort desc should surface Carol (age 50)");
  });
});
