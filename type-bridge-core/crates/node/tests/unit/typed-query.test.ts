import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  AggregateSpec,
  ComparisonExpr,
  SortExpr,
  TypedQuery,
  agg,
  long,
  string,
  type AggregateInput,
  type DynamicEntityRow,
  type DynamicQuerySpec,
} from "../../typescript/index.js";

const nameField = { attrName: "q-name" } as const;
const ageField = { attrName: "q-age" } as const;

const name = {
  eq: (value: string) => new ComparisonExpr(nameField.attrName, "eq", string(value)),
  startsWith: (value: string) =>
    new ComparisonExpr(nameField.attrName, "starts_with", string(value)),
  asc: () => new SortExpr(nameField.attrName, "Asc"),
};

const age = {
  ne: (value: bigint) => new ComparisonExpr(ageField.attrName, "neq", long(value)),
  gt: (value: bigint) => new ComparisonExpr(ageField.attrName, "gt", long(value)),
  gte: (value: bigint) => new ComparisonExpr(ageField.attrName, "gte", long(value)),
  desc: () => new SortExpr(ageField.attrName, "Desc"),
  avg: () =>
    new AggregateSpec(
      { result_key: "avg_q_age", function: "mean", attr_name: ageField.attrName },
      "avg_q-age",
    ),
};

// Records the spec/args each terminal method receives and returns canned rows,
// so the query builder and aggregate normalizers can be exercised without a DB.
class StubManager {
  lastSpec: DynamicQuerySpec | null = null;
  lastGroupFields: string[] | null = null;
  lastAggregates: AggregateInput[] | null = null;
  rows: DynamicEntityRow[] = [];
  countValue = 0n;
  aggregateRows: Record<string, unknown>[] = [];
  groupRows: Record<string, unknown>[] = [];

  query(spec: DynamicQuerySpec): DynamicEntityRow[] {
    this.lastSpec = spec;
    return this.rows;
  }

  queryCount(spec: DynamicQuerySpec): bigint {
    this.lastSpec = spec;
    return this.countValue;
  }

  queryAggregate(spec: DynamicQuerySpec, aggregates: AggregateInput[]): unknown[] {
    this.lastSpec = spec;
    this.lastAggregates = aggregates;
    return this.aggregateRows;
  }

  queryGroupByAggregate(
    spec: DynamicQuerySpec,
    groupFields: string[],
    aggregates: AggregateInput[],
  ): unknown[] {
    this.lastSpec = spec;
    this.lastGroupFields = groupFields;
    this.lastAggregates = aggregates;
    return this.groupRows;
  }
}

function queryOver(stub: StubManager): TypedQuery<DynamicEntityRow, DynamicEntityRow> {
  return new TypedQuery<DynamicEntityRow, DynamicEntityRow>(stub, (rows) => rows);
}

describe("typed query expression serialization", () => {
  test("comparison + boolean tree serializes to the DynamicExpr wire shape", () => {
    const expr = age.gte(18n).and_(name.eq("Al"));
    assert.deepEqual(expr.toExpr(), {
      kind: "and",
      exprs: [
        { kind: "compare", attr_name: "q-age", operator: "gte", value: { value_type: "long", value: "18" } },
        { kind: "compare", attr_name: "q-name", operator: "eq", value: { value_type: "string", value: "Al" } },
      ],
    });
  });

  test("startsWith carries the raw literal under the starts_with op", () => {
    assert.deepEqual(name.startsWith("Al").toExpr(), {
      kind: "compare",
      attr_name: "q-name",
      operator: "starts_with",
      value: { value_type: "string", value: "Al" },
    });
  });

  test("ne lowers to the neq wire operator", () => {
    assert.equal((age.ne(1n).toExpr() as { operator: string }).operator, "neq");
  });
});

describe("typed aggregate lowering", () => {
  test("avg lowers to mean; wire key is sanitized, user-facing key keeps the name", () => {
    const avg = age.avg();
    assert.deepEqual(avg.input, { result_key: "avg_q_age", function: "mean", attr_name: "q-age" });
    assert.equal(avg.resultKey, "avg_q-age");
  });

  test("count is field-independent", () => {
    const count = agg.count();
    assert.deepEqual(count.input, { result_key: "count", function: "count", attr_name: null });
    assert.equal(count.resultKey, "count");
  });
});

describe("typed query builder", () => {
  test("filter/orderBy/limit/offset accumulate into one spec", () => {
    const stub = new StubManager();
    queryOver(stub)
      .filter(age.gt(30n))
      .orderBy(name.asc(), age.desc())
      .offset(5)
      .limit(10)
      .all();
    assert.deepEqual(stub.lastSpec, {
      expr: [{ kind: "compare", attr_name: "q-age", operator: "gt", value: { value_type: "long", value: "30" } }],
      sort: [
        { kind: "attribute", attr_name: "q-name", direction: "Asc" },
        { kind: "attribute", attr_name: "q-age", direction: "Desc" },
      ],
      limit: 10,
      offset: 5,
    });
  });

  test("count sends only the expr list", () => {
    const stub = new StubManager();
    stub.countValue = 7n;
    assert.equal(queryOver(stub).filter(age.gt(1n)).count(), 7n);
    assert.deepEqual(stub.lastSpec, {
      expr: [{ kind: "compare", attr_name: "q-age", operator: "gt", value: { value_type: "long", value: "1" } }],
    });
  });

  test("first requests a single row", () => {
    const stub = new StubManager();
    stub.rows = [{ iid: "0x1", type_name: "q-age", attributes: [] }];
    assert.deepEqual(queryOver(stub).first(), { iid: "0x1", type_name: "q-age", attributes: [] });
    assert.equal(stub.lastSpec?.limit, 1);
  });

  test("execute is an alias for all", () => {
    const stub = new StubManager();
    stub.rows = [{ iid: "0x1", type_name: "q-age", attributes: [] }];
    const query = queryOver(stub).filter(age.gt(1n));
    assert.deepEqual(query.execute(), query.all());
  });
});

describe("typed aggregate normalization", () => {
  test("aggregate unwraps reduce { value } documents under user-facing keys", () => {
    const stub = new StubManager();
    stub.aggregateRows = [{ count: { value: 2 }, avg_q_age: { value: 31.5 } }];
    assert.deepEqual(queryOver(stub).aggregate(agg.count(), age.avg()), {
      count: 2,
      "avg_q-age": 31.5,
    });
  });

  test("groupBy maps group<index> back to attribute names", () => {
    const stub = new StubManager();
    stub.groupRows = [{ group0: { value: "Alice" }, count: { value: 1 } }];
    const result = queryOver(stub).groupBy(nameField).aggregate(agg.count());
    assert.deepEqual(result, [{ "q-name": "Alice", count: 1 }]);
    assert.deepEqual(stub.lastGroupFields, ["q-name"]);
  });

  test("an aggregate entry that is not a { value } document raises", () => {
    const stub = new StubManager();
    stub.aggregateRows = [{ count: 2 }];
    assert.throws(() => queryOver(stub).aggregate(agg.count()), /not a \{ value \} document/);
  });
});
