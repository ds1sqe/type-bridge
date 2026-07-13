import {
  Entity,
  Key,
  Relation,
  TypedQuery,
  agg,
  attr,
  field,
  loadNative,
  role,
  type AggregateInput,
  type DynamicEntityRow,
  type DynamicQuerySpec,
  type NativeModule,
} from "@type-bridge/node";

class LegacyName extends attr.String("legacy-name") {}
class LegacyAge extends attr.Integer("legacy-age") {}

class LegacyPerson extends Entity("legacy-person", {
  name: field(LegacyName, Key),
  age: field(LegacyAge).optional(),
}) {}

class LegacyEmployment extends Relation("legacy-employment", {
  employee: role(LegacyPerson),
}) {}

type QueryCall = Readonly<{
  operation: "query" | "count" | "aggregate" | "group";
  spec: DynamicQuerySpec;
}>;

class RecordingManager {
  readonly calls: QueryCall[] = [];
  rows: DynamicEntityRow[] = [
    { iid: "0x1", type_name: "legacy-person", attributes: [] },
    { iid: "0x2", type_name: "legacy-person", attributes: [] },
  ];
  countValue = 2n;
  aggregateRows: Record<string, unknown>[] = [
    { count: { value: 2 }, avg_legacy_age: { value: 27.5 } },
  ];
  groupRows: Record<string, unknown>[] = [
    { group0: { value: "Alice" }, count: { value: 1 } },
  ];

  query(spec: DynamicQuerySpec): DynamicEntityRow[] {
    this.calls.push({ operation: "query", spec });
    return this.rows;
  }

  queryCount(spec: DynamicQuerySpec): bigint {
    this.calls.push({ operation: "count", spec });
    return this.countValue;
  }

  queryAggregate(spec: DynamicQuerySpec, _aggregates: AggregateInput[]): unknown[] {
    this.calls.push({ operation: "aggregate", spec });
    return this.aggregateRows;
  }

  queryGroupByAggregate(
    spec: DynamicQuerySpec,
    _groupFields: string[],
    _aggregates: AggregateInput[],
  ): unknown[] {
    this.calls.push({ operation: "group", spec });
    return this.groupRows;
  }
}

function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function equal<T>(actual: T, expected: T, message: string): void {
  invariant(Object.is(actual, expected), `${message}: expected ${String(expected)}, got ${String(actual)}`);
}

function comparable(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) =>
    typeof item === "bigint" ? `${item.toString()}n` : item,
  );
}

function deepEqual(actual: unknown, expected: unknown, message: string): void {
  const actualText = comparable(actual);
  const expectedText = comparable(expected);
  invariant(actualText === expectedText, `${message}: expected ${expectedText}, got ${actualText}`);
}

const typedName: LegacyName = new LegacyPerson({
  name: new LegacyName("Alice"),
  age: new LegacyAge(30n),
}).name;
void typedName;
const typedPlayer: LegacyPerson | readonly LegacyPerson[] = new LegacyEmployment({
  employee: new LegacyPerson({ name: new LegacyName("Alice") }),
}).employee;
void typedPlayer;

const nativeLoader: () => NativeModule = loadNative;
void nativeLoader;

const manager = new RecordingManager();
const hydrateRows = (rows: DynamicEntityRow[]): LegacyPerson[] =>
  rows.map(
    (row) =>
      new LegacyPerson({
        name: new LegacyName(row.iid ?? "missing"),
      }),
  );
const query: TypedQuery<LegacyPerson, DynamicEntityRow> = new TypedQuery<
  LegacyPerson,
  DynamicEntityRow
>(manager, hydrateRows);

const aliasProbe = new TypedQuery<LegacyPerson, DynamicEntityRow>(manager, hydrateRows);
equal(aliasProbe.filter(LegacyAge.gte(new LegacyAge(1n))), aliasProbe, "filter return alias");
equal(aliasProbe.orderBy(LegacyName.asc()), aliasProbe, "orderBy return alias");
equal(aliasProbe.limit(1), aliasProbe, "limit return alias");
equal(aliasProbe.offset(1), aliasProbe, "offset return alias");

// The legacy builder mutates in place and returns the same object. Ignoring any
// return value must therefore retain all changes on the original alias.
const sibling = query;
query.filter(LegacyAge.gte(new LegacyAge(18n)));
query.filter(LegacyName.contains("legacy"));
query.orderBy(LegacyName.asc());
query.limit(2);
query.offset(1);
equal(sibling, query, "sibling must remain the same mutable query");

const expectedFullSpec: DynamicQuerySpec = {
  expr: [
    {
      kind: "compare",
      attr_name: "legacy-age",
      operator: "gte",
      value: { value_type: "long", value: "18" },
    },
    {
      kind: "compare",
      attr_name: "legacy-name",
      operator: "contains",
      value: { value_type: "string", value: "legacy" },
    },
  ],
  sort: [{ kind: "attribute", attr_name: "legacy-name", direction: "Asc" }],
  limit: 2,
  offset: 1,
};

const allRows: LegacyPerson[] = sibling.all();
equal(allRows.length, 2, "all must hydrate every recording row");
invariant(allRows.every((row) => row instanceof LegacyPerson), "all must return typed models");
deepEqual(manager.calls.at(-1), { operation: "query", spec: expectedFullSpec }, "all request");

const executeRows: LegacyPerson[] = query.execute();
equal(executeRows.length, 2, "execute must remain an alias for all");
deepEqual(
  manager.calls.at(-1),
  { operation: "query", spec: expectedFullSpec },
  "execute request",
);

const firstRow: LegacyPerson | null = query.first();
invariant(firstRow instanceof LegacyPerson, "first must hydrate one typed model");
deepEqual(
  manager.calls.at(-1),
  { operation: "query", spec: { ...expectedFullSpec, limit: 1 } },
  "first request",
);

const count: bigint = query.count();
equal(count, 2n, "count result");
deepEqual(
  manager.calls.at(-1),
  { operation: "count", spec: { expr: expectedFullSpec.expr } },
  "count must ignore sort and window",
);

const exists: boolean = query.exists();
equal(exists, true, "exists result");
deepEqual(
  manager.calls.at(-1),
  { operation: "count", spec: { expr: expectedFullSpec.expr } },
  "exists must use the legacy count path",
);

const aggregate: Record<string, unknown> = query.aggregate(agg.count(), LegacyAge.avg());
deepEqual(aggregate, { count: 2, "avg_legacy-age": 27.5 }, "aggregate normalization");
deepEqual(
  manager.calls.at(-1),
  { operation: "aggregate", spec: { expr: expectedFullSpec.expr } },
  "aggregate must ignore sort and window",
);

const grouped: Record<string, unknown>[] = query
  .groupBy(LegacyName)
  .aggregate(agg.count());
deepEqual(grouped, [{ "legacy-name": "Alice", count: 1 }], "groupBy normalization");
deepEqual(
  manager.calls.at(-1),
  { operation: "group", spec: { expr: expectedFullSpec.expr } },
  "groupBy must ignore sort and window",
);

// Pin the current public two-parameter generic arity. These must remain compile
// errors during #170; the new facade belongs to a different package subpath.
// @ts-expect-error legacy TypedQuery requires both T and Row
type MissingLegacyRow = TypedQuery<LegacyPerson>;
// @ts-expect-error legacy TypedQuery accepts exactly two generic parameters
type ExtraLegacySlot = TypedQuery<LegacyPerson, DynamicEntityRow, LegacyName>;
void (null as unknown as MissingLegacyRow);
void (null as unknown as ExtraLegacySlot);
