import assert = require("node:assert/strict");
import test = require("node:test");

import { Card, Entity, Key, Relation, attr, field, role } from "../../typescript/index.js";
import {
  QuerySession,
  TypedMatchError,
  references,
  type Selection,
} from "../../typescript/typed/index.js";
import { pageFromValidatedResult } from "../../typescript/typed/page.js";
import {
  materializeValidatedCount,
  materializeValidatedExists,
  materializeValidatedOne,
  materializeValidatedPage,
  materializeValidatedRows,
} from "../../typescript/typed/results.js";
import {
  diagnosticQuerySession,
  registeredModelConstructors,
} from "../../typescript/typed/session.js";
import { corpusError } from "./semantic-corpus.js";

class RuntimeQueryName extends attr.String("runtime-query-v2-name") {}
class RuntimeQueryPerson extends Entity("runtime-query-v2-person", {
  name: field(RuntimeQueryName, Key),
}) {}
class RuntimeQueryPersonCollision extends Entity("runtime-query-v2-person", {
  name: field(RuntimeQueryName, Key),
}) {}
class RuntimeQueryCompany extends Entity("runtime-query-v2-company", {
  name: field(RuntimeQueryName, Key),
}) {}
class RuntimeQuerySibling extends Entity("runtime-query-v2-sibling", {
  name: field(RuntimeQueryName, Key),
}) {}
class RuntimeQueryEmployment extends Relation("runtime-query-v2-employment", {
  code: field(RuntimeQueryName, Key),
  employee: role(RuntimeQueryPerson),
  employer: role(RuntimeQueryCompany),
}) {}
class RuntimeQueryParty extends Entity("runtime-query-v2-party", {
  name: field(RuntimeQueryName, Key),
}) {}
class RuntimeQueryEmployee extends Entity(
  "runtime-query-v2-employee",
  {},
  { parent: RuntimeQueryParty },
) {}
class RuntimeQueryPartyAssociation extends Relation("runtime-query-v2-party-association", {
  party: role(RuntimeQueryParty),
}) {}
class RuntimeQueryTraversal extends Relation("runtime-query-v2-traversal", {
  previous: role(RuntimeQueryParty),
  next: role(RuntimeQueryParty),
}) {}
class RuntimeQuerySpecialTraversal extends Relation(
  "runtime-query-v2-special-traversal",
  {},
  { parent: RuntimeQueryTraversal },
) {}
class RuntimeQueryForeignTraversal extends Relation(
  "runtime-query-v2-foreign-traversal",
  {
    previous: role(RuntimeQueryParty),
    next: role(RuntimeQueryParty),
  },
) {}
class RuntimeMaterializedLong extends attr.Integer("runtime-materialized-long") {}
class RuntimeOrderedText extends attr.String("runtime-ordered-text") {}
class RuntimeOptionalOrderedText extends attr.String("runtime-optional-ordered-text") {}
class RuntimeMaterializedPerson extends Entity("runtime-materialized-person", {
  value: field(RuntimeMaterializedLong, Key),
}) {
  static constructions = 0;

  constructor(values: { readonly value: RuntimeMaterializedLong }) {
    super(values);
    RuntimeMaterializedPerson.constructions += 1;
  }
}
class RuntimeOrderedEntity extends Entity("runtime-ordered-entity", {
  requiredValues: field(RuntimeOrderedText).ordered(),
  optionalValues: field(RuntimeOptionalOrderedText).optional().ordered(),
}) {}
class RuntimeMaterializedMembership extends Relation("runtime-materialized-membership", {
  code: field(RuntimeMaterializedLong, Key),
  member: role(RuntimeMaterializedPerson, { cardinality: Card(0) }),
}) {}
class RuntimeMaterializedEnvelope extends Relation("runtime-materialized-envelope", {
  nested: role(RuntimeMaterializedMembership, { cardinality: Card(1, 1) }),
}) {}

const personRefs = references(RuntimeQueryPerson);
const employmentRefs = references(RuntimeQueryEmployment);
const traversalRefs = references(RuntimeQueryTraversal);
const foreignTraversalRefs = references(RuntimeQueryForeignTraversal);
const envelopeRefs = references(RuntimeMaterializedEnvelope);

function expectMatchError(
  operation: () => unknown,
  category: TypedMatchError["category"],
  code: string,
): TypedMatchError {
  let observed: TypedMatchError | undefined;
  assert.throws(operation, (error: unknown) => {
    if (!(error instanceof TypedMatchError)) return false;
    assert.equal(error.category, category);
    assert.equal(error.code, code);
    observed = error;
    return true;
  });
  if (observed === undefined) {
    throw new Error("expected TypedMatchError predicate to capture the thrown error");
  }
  return observed;
}

test("public query sessions reject missing and forged execution connections", () => {
  const RuntimeQuerySession = QuerySession as unknown as new (
    connection?: unknown,
  ) => QuerySession;
  for (const connection of [undefined, null, {}, 42, "connection", Symbol("connection")]) {
    assert.throws(
      () => new RuntimeQuerySession(connection),
      /requires a RustDatabase or RustTransactionContext/,
    );
  }
  assert.ok(diagnosticQuerySession() instanceof QuerySession);
});

test("query construction is immutable, opaque, and preserves distinct repeated models", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const secondPerson = session.var(RuntimeQueryPerson);
  const company = session.var(RuntimeQueryCompany);
  const sibling = session.var(RuntimeQuerySibling);
  const employment = session.var(RuntimeQueryEmployment);

  const base = session.query(person, company);
  const hidden = base.match(employment);
  const filtered = hidden.where(
    employment.role(employmentRefs.roles.employee).connects(person),
    employment.role(employmentRefs.roles.employer).connects(company),
  );
  const siblingQuery = hidden.where(person.field(personRefs.fields.name).startsWith("A"));
  const repeated = session.query(person, secondPerson);
  const shapedSibling = session.query(person, sibling);

  assert.notStrictEqual(base, hidden);
  assert.notStrictEqual(hidden, filtered);
  assert.notStrictEqual(filtered, siblingQuery);
  assert.notStrictEqual(repeated, shapedSibling);
  assert.ok(Object.isFrozen(base));
  assert.ok(Object.isFrozen(filtered));
  assert.deepEqual(Object.keys(filtered), []);

  expectMatchError(
    () => filtered.one(),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => base.one(),
    "invalid_plan",
    corpusError("topology.disconnected")[1],
  );
  expectMatchError(
    () => base.allowCrossJoin(person, company).one(),
    "invalid_plan",
    "execution_connection_required",
  );
});

test("bounded reachability is construction-time, inherited-role aware, and output-neutral", () => {
  const session = diagnosticQuerySession();
  const source = session.var(RuntimeQueryParty, "subtypes");
  const target = session.var(RuntimeQueryEmployee);
  const reachable = session.reachable(
    source,
    target,
    RuntimeQuerySpecialTraversal,
    traversalRefs.roles.previous,
    traversalRefs.roles.next,
    { minDepth: 0, maxDepth: 2 },
  );
  const query = session.query(target).match(source).where(reachable);
  const identity = session.reachable(
    source,
    source,
    RuntimeQueryTraversal,
    traversalRefs.roles.previous,
    traversalRefs.roles.next,
    { minDepth: 0, maxDepth: 0 },
  );

  assert.ok(Object.isFrozen(reachable));
  assert.ok(Object.isFrozen(identity));
  expectMatchError(
    () => query.one(),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => session.query(source).where(identity).one(),
    "invalid_plan",
    "execution_connection_required",
  );
});

test("bounded reachability rejects malformed bounds and preserves native diagnostics before terminals", () => {
  const session = diagnosticQuerySession();
  const source = session.var(RuntimeQueryParty);
  const target = session.var(RuntimeQueryEmployee);
  const invoke = (bounds: unknown): unknown =>
    (
      session.reachable as unknown as (
        ...args: readonly unknown[]
      ) => unknown
    )(
      source,
      target,
      RuntimeQueryTraversal,
      traversalRefs.roles.previous,
      traversalRefs.roles.next,
      bounds,
    );

  for (const bounds of [
    null,
    [],
    { minDepth: "0", maxDepth: 1 },
    { minDepth: 0.5, maxDepth: 1 },
    { minDepth: Number.NaN, maxDepth: 1 },
    { minDepth: 0, maxDepth: Number.POSITIVE_INFINITY },
  ]) {
    assert.throws(() => invoke(bounds), TypeError);
  }
  for (const bounds of [
    { minDepth: -1, maxDepth: 1 },
    { minDepth: 0, maxDepth: 256 },
  ]) {
    assert.throws(() => invoke(bounds), RangeError);
  }

  expectMatchError(
    () => invoke({ minDepth: 2, maxDepth: 1 }),
    "invalid_plan",
    "reachable_bounds",
  );
  expectMatchError(
    () => invoke({ minDepth: 0, maxDepth: 65 }),
    "invalid_plan",
    "reachable_depth_limit",
  );

  const foreignSource = diagnosticQuerySession().var(RuntimeQueryParty);
  expectMatchError(
    () =>
      session.reachable(
        foreignSource,
        target,
        RuntimeQueryTraversal,
        traversalRefs.roles.previous,
        traversalRefs.roles.next,
        { minDepth: 1, maxDepth: 1 },
      ),
    "invalid_plan",
    "cross_session_handle",
  );
});

test("bounded reachability rejects forged relation and role provenance at construction", () => {
  const session = diagnosticQuerySession();
  const source = session.var(RuntimeQueryParty);
  const target = session.var(RuntimeQueryEmployee);
  const invoke = session.reachable as unknown as (
    ...args: readonly unknown[]
  ) => unknown;

  assert.throws(
    () =>
      invoke(
        source,
        target,
        RuntimeQueryTraversal,
        foreignTraversalRefs.roles.previous,
        foreignTraversalRefs.roles.next,
        { minDepth: 1, maxDepth: 2 },
      ),
    /must belong to the relation model/,
  );
  assert.throws(
    () =>
      invoke(
        source,
        target,
        RuntimeQueryParty,
        traversalRefs.roles.previous,
        traversalRefs.roles.next,
        { minDepth: 1, maxDepth: 2 },
      ),
    /must be a declared Relation model class/,
  );
  assert.throws(
    () =>
      invoke(
        source,
        target,
        RuntimeQueryTraversal,
        Object.freeze({}),
        traversalRefs.roles.next,
        { minDepth: 1, maxDepth: 2 },
      ),
    /role reference was not created by references/,
  );
});

test("boolean binding rules match the semantic corpus", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const company = session.var(RuntimeQueryCompany);
  const companyRefs = references(RuntimeQueryCompany);
  const personName = person
    .field(personRefs.fields.name)
    .eq(new RuntimeQueryName("Alice"));
  const companyName = company
    .field(companyRefs.fields.name)
    .eq(new RuntimeQueryName("Acme"));

  const partialOr = session
    .query(person)
    .match(company)
    .where(personName.or(companyName));
  expectMatchError(
    () => partialOr.rows({ limit: 1 }),
    "invalid_plan",
    corpusError("boolean.or-definite-binding")[1],
  );
  expectMatchError(
    () => session.query(person).where(companyName.not()),
    "invalid_plan",
    corpusError("boolean.not-unattached-reference")[1],
  );
});

test("native terminals preserve operation-specific validation and stable diagnostics", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const company = session.var(RuntimeQueryCompany);
  const employment = session.var(RuntimeQueryEmployment);
  const connected = session
    .query(person, company)
    .match(employment)
    .where(
      employment.role(employmentRefs.roles.employee).connects(person),
      employment.role(employmentRefs.roles.employer).connects(company),
    );

  expectMatchError(
    () => connected.rows({
      limit: 25,
      orderBy: [person.field(personRefs.fields.name).asc()],
    }),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => connected.countBy(person),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => connected.existsBy(company),
    "invalid_plan",
    "execution_connection_required",
  );

  const single = session.query(person);
  expectMatchError(
    () => single.pageBy(person, {
      limit: 10,
      offset: 2,
      orderBy: [person.field(personRefs.fields.name).asc()],
      includeTotal: true,
    }),
    "invalid_plan",
    "execution_connection_required",
  );

  expectMatchError(
    () => single.rows({ limit: 0 }),
    "invalid_plan",
    corpusError("bounds.public-invalid-limit")[1],
  );
  expectMatchError(
    () => single.rows({ limit: 1, offset: -1 }),
    "invalid_plan",
    corpusError("bounds.public-invalid-offset")[1],
  );
  expectMatchError(
    () => single.rows({ limit: Number.MAX_SAFE_INTEGER, offset: 1 }),
    "invalid_plan",
    corpusError("bounds.window-overflow")[1],
  );
});

test("duplicate, hidden, cross-session, and 1/16/17 arity checks fail before execution", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const company = session.var(RuntimeQueryCompany);
  const unattached = session.var(RuntimeQueryEmployment);

  expectMatchError(
    () => session.query(person, person),
    "invalid_plan",
    corpusError("selection.duplicate-handle")[1],
  );

  expectMatchError(
    () => session.query(person).where(
      unattached.role(employmentRefs.roles.employee).connects(person),
    ),
    "invalid_plan",
    "unattached_binding",
  );

  const other = diagnosticQuerySession().var(RuntimeQueryCompany);
  expectMatchError(
    () => (session.query as unknown as (...selections: Selection<unknown>[]) => unknown)(
      person,
      other,
    ),
    "invalid_plan",
    "cross_session_handle",
  );

  const variables = Array.from({ length: 17 }, () => session.var(RuntimeQueryPerson));
  assert.ok(session.query(person));
  assert.ok(
    session.query(
      person,
      company,
      unattached,
      variables[0]!,
      variables[1]!,
    ),
  );
  assert.ok(
    (session.query as unknown as (...selections: Selection<unknown>[]) => unknown)(
      ...variables.slice(0, 16),
    ),
  );
  expectMatchError(
    () => (session.query as unknown as (...selections: Selection<unknown>[]) => unknown)(
      ...variables,
    ),
    "invalid_plan",
    corpusError("selection.seventeen-slot-rejection")[1],
  );
  expectMatchError(
    () => (session.query as unknown as (...selections: Selection<unknown>[]) => unknown)(),
    "invalid_plan",
    "empty_output",
  );

  // A selected but intentionally independent pair must declare topology.
  const independent = session.query(person, company);
  expectMatchError(
    () => independent.one(),
    "invalid_plan",
    corpusError("topology.disconnected")[1],
  );
});

test("session constructor metadata recursively registers role players and ancestors", () => {
  const session = diagnosticQuerySession();
  const employment = session.var(RuntimeQueryEmployment);
  const employee = session.var(RuntimeQueryEmployee);
  const person = session.var(RuntimeQueryPerson);
  const company = session.var(RuntimeQueryCompany);

  const connected = session
    .query(employment, person, company)
    .where(
      employment.role(employmentRefs.roles.employee).connects(person),
      employment.role(employmentRefs.roles.employer).connects(company),
    );

  assert.ok(employee);
  expectMatchError(
    () => connected.one(),
    "invalid_plan",
    "execution_connection_required",
  );
});

test("positional and exact named collections lower only to page operations", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const employment = session.var(RuntimeQueryEmployment);
  const company = session.var(RuntimeQueryCompany);
  const employeeEdge = employment
    .role(employmentRefs.roles.employee)
    .connects(person);
  const employerEdge = employment
    .role(employmentRefs.roles.employer)
    .connects(company);
  const employments = employment
    .collect()
    .orderBy(employment.field(employmentRefs.fields.code).asc());
  const companies = company
    .collect()
    .distinct()
    .orderBy(company.field(references(RuntimeQueryCompany).fields.name).asc());

  const positional = session
    .query(person, employments, companies)
    .where(employeeEdge, employerEdge);
  const named = session
    .queryNamed({ person, employments, companies })
    .where(employeeEdge, employerEdge);

  expectMatchError(
    () => positional.pageBy(person, {
      limit: 10,
      includeTotal: true,
      orderBy: [person.field(personRefs.fields.name).asc()],
    }),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => named.pageBy(person, {
      limit: 10,
      orderBy: [person.field(personRefs.fields.name).asc()],
    }),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => named.countBy(person),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => named.existsBy(person),
    "invalid_plan",
    "execution_connection_required",
  );

  expectMatchError(
    () => (positional as unknown as { one(): unknown }).one(),
    "invalid_plan",
    "collection_requires_page_root",
  );
  expectMatchError(
    () => (named as unknown as { rows(options: { limit: number }): unknown }).rows({ limit: 10 }),
    "invalid_plan",
    "collection_requires_page_root",
  );

  expectMatchError(
    () => session.queryNamed({ first: person, duplicate: person.collect() }),
    "invalid_plan",
    corpusError("selection.duplicate-handle")[1],
  );
});

test("named shape and page envelope diagnostics are exact and immutable", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const company = session.var(RuntimeQueryCompany);

  const singular = session
    .queryNamed({ person, company })
    .allowCrossJoin(person, company);
  const secondPerson = session.var(RuntimeQueryPerson);
  const repeated = session
    .queryNamed({ first: person, second: secondPerson })
    .allowCrossJoin(person, secondPerson);
  expectMatchError(
    () => singular.one(),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => (singular as unknown as {
      pageBy(root: typeof person, options: { limit: number }): unknown;
    }).pageBy(person, { limit: 10 }),
    "invalid_plan",
    corpusError("shape.page-non-root-singular")[1],
  );
  expectMatchError(
    () => repeated.one(),
    "invalid_plan",
    "execution_connection_required",
  );
  expectMatchError(
    () => (repeated as unknown as {
      pageBy(root: typeof person, options: { limit: number }): unknown;
    }).pageBy(person, { limit: 10 }),
    "invalid_plan",
    corpusError("shape.page-non-root-singular")[1],
  );

  const invalidName = (
    session.queryNamed as unknown as (
      selections: Readonly<Record<string, Selection<unknown>>>,
    ) => { one(): unknown }
  )({ "": person });
  expectMatchError(() => invalidName.one(), "invalid_plan", "invalid_output_name");

  expectMatchError(
    () => (
      session.queryNamed as unknown as (
        selections: Readonly<Record<string, Selection<unknown>>>,
      ) => unknown
    )({}),
    "invalid_plan",
    "empty_output",
  );

  const seventeen = Object.fromEntries(
    Array.from({ length: 17 }, (_, index) => [
      `member${index}`,
      session.var(RuntimeQueryPerson),
    ]),
  ) as Readonly<Record<string, Selection<unknown>>>;
  expectMatchError(
    () => (
      session.queryNamed as unknown as (
        selections: Readonly<Record<string, Selection<unknown>>>,
      ) => unknown
    )(seventeen),
    "invalid_plan",
    corpusError("selection.seventeen-slot-rejection")[1],
  );

  const source = [{ value: 1 }];
  const withoutTotal = pageFromValidatedResult(source, 0n, 1n);
  source.push({ value: 2 });
  assert.ok(Object.isFrozen(withoutTotal));
  assert.ok(Object.isFrozen(withoutTotal.items));
  assert.equal(withoutTotal.items.length, 1);
  assert.equal(withoutTotal.offset, 0);
  assert.equal(withoutTotal.limit, 1);
  assert.equal(withoutTotal.total, undefined);
  assert.equal("total" in withoutTotal, true);

  const withTotal = pageFromValidatedResult(source, 0n, 2n, 2n);
  assert.equal(withTotal.total, 2n);
  assert.throws(
    () => (withTotal.items as { value: number }[]).push({ value: 3 }),
    TypeError,
  );
  assert.throws(
    () => pageFromValidatedResult([], BigInt(Number.MAX_SAFE_INTEGER) + 1n, 1n),
    RangeError,
  );
});

test("validated-handle materialization preserves bigint and freezes every public container", () => {
  RuntimeMaterializedPerson.constructions = 0;
  const query = Object.freeze({});
  const thing = Object.freeze({
    iid: () => "0x-materialized-person",
    concreteDescriptor: () => `entity:${RuntimeMaterializedPerson.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["value"],
    fieldValuesJson: (fieldName: string) =>
      fieldName === "value" ? JSON.stringify([{ Long: "9007199254740993" }]) : null,
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const positionalResult = Object.freeze({
    rowCount: (owner: object) => owner === query ? 1 : 0,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => null,
    slotCount: () => 1,
    slotThing: () => thing,
  });
  const models = new Map([[RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson]]);

  const one = materializeValidatedOne(
    query as never,
    positionalResult as never,
    models,
  ) as RuntimeMaterializedPerson;
  assert.ok(one instanceof RuntimeMaterializedPerson);
  assert.equal(one.value.value, 9007199254740993n);
  assert.equal(one._iid, "0x-materialized-person");
  assert.ok(Object.isFrozen(one));
  assert.ok(Object.isFrozen(one.value));
  assert.equal(RuntimeMaterializedPerson.constructions, 1);
  assert.throws(() => {
    (one as unknown as { value: RuntimeMaterializedLong }).value =
      new RuntimeMaterializedLong(1n);
  }, TypeError);

  const namedResult = Object.freeze({
    ...positionalResult,
    outputNames: () => ["person"],
  });
  const rows = materializeValidatedRows(
    query as never,
    namedResult as never,
    models,
  ) as readonly Readonly<{ person: RuntimeMaterializedPerson }>[];
  assert.ok(Object.isFrozen(rows));
  assert.ok(Object.isFrozen(rows[0]));
  assert.ok(Object.isFrozen(rows[0]!.person));
  assert.throws(
    () => (rows as Readonly<{ person: RuntimeMaterializedPerson }>[]).push(rows[0]!),
    TypeError,
  );
});

test("named __proto__ outputs remain enumerable own data properties", () => {
  const query = Object.freeze({});
  const thing = Object.freeze({
    iid: () => "0x-proto-person",
    concreteDescriptor: () => `entity:${RuntimeMaterializedPerson.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["value"],
    fieldValuesJson: () => JSON.stringify([{ Long: "1" }]),
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const models = new Map([
    [RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson],
  ]);
  const rowsResult = Object.freeze({
    rowCount: () => 1,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => ["__proto__"],
    slotCount: () => 1,
    slotThing: () => thing,
  });
  const pageResult = Object.freeze({
    pageEntryCount: () => 1,
    pageOffset: () => 0n,
    pageLimit: () => 1n,
    pageTotal: () => null,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => ["__proto__"],
    pageSlotCount: () => 1,
    pageSlotValueCount: () => 1,
    pageSlotThing: () => thing,
  });
  type ProtoRow = Readonly<Record<"__proto__", RuntimeMaterializedPerson>>;
  const [row] = materializeValidatedRows(
    query as never,
    rowsResult as never,
    models,
  ) as readonly ProtoRow[];
  const page = materializeValidatedPage(
    query as never,
    pageResult as never,
    models,
    0n,
    1n,
    false,
  ) as Readonly<{ items: readonly ProtoRow[] }>;

  for (const value of [row!, page.items[0]!]) {
    const selected = value["__proto__"];
    assert.ok(selected instanceof RuntimeMaterializedPerson);
    assert.equal(selected._iid, "0x-proto-person");
    assert.deepEqual(Object.getOwnPropertyDescriptor(value, "__proto__"), {
      value: selected,
      writable: false,
      enumerable: true,
      configurable: false,
    });
    assert.equal(Object.getPrototypeOf(value), Object.prototype);
    assert.deepEqual(Object.keys(value), ["__proto__"]);
    assert.ok(Object.isFrozen(value));
  }
});

test("bare ordered fields derive cardinality from requiredness without a cast", () => {
  const query = Object.freeze({});
  const thing = Object.freeze({
    iid: () => "0x-ordered-entity",
    concreteDescriptor: () => `entity:${RuntimeOrderedEntity.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["requiredValues"],
    fieldValuesJson: (fieldName: string) =>
      fieldName === "requiredValues"
        ? JSON.stringify([{ String: "first" }, { String: "second" }])
        : null,
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const result = Object.freeze({
    rowCount: () => 1,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => null,
    slotCount: () => 1,
    slotThing: () => thing,
  });
  const models = new Map([[RuntimeOrderedEntity.typeName, RuntimeOrderedEntity]]);

  const materialized = materializeValidatedOne(
    query as never,
    result as never,
    models,
  ) as InstanceType<typeof RuntimeOrderedEntity>;
  assert.deepEqual(
    materialized.requiredValues.map((value) => value.value),
    ["first", "second"],
  );
  assert.equal(materialized.optionalValues, undefined);
  assert.ok(Object.isFrozen(materialized.requiredValues));

  const missingRequired = Object.freeze({
    ...thing,
    fieldNames: () => [],
    fieldValuesJson: () => null,
  });
  expectMatchError(
    () => materializeValidatedOne(
      query as never,
      Object.freeze({ ...result, slotThing: () => missingRequired }) as never,
      models,
    ),
    "result_decode",
    "missing_result_field",
  );
});

test("hostile result ownership and shape fail before model construction", () => {
  RuntimeMaterializedPerson.constructions = 0;
  const query = Object.freeze({});
  const ownershipFailure = Object.freeze({
    rowCount: () => {
      throw new Error(JSON.stringify({
        category: "result_decode",
        code: "result_query_mismatch",
        message: "foreign result",
        path: [{ kind: "result" }],
        details: {},
      }));
    },
  });
  const models = new Map([[RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson]]);

  expectMatchError(
    () => materializeValidatedRows(query as never, ownershipFailure as never, models),
    "result_decode",
    "result_query_mismatch",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);

  const shapeFailure = Object.freeze({
    rowCount: () => 1,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => ["first", "second"],
    slotCount: () => 1,
    slotThing: () => {
      throw new Error("slot access must not occur after shape failure");
    },
  });
  expectMatchError(
    () => materializeValidatedRows(query as never, shapeFailure as never, models),
    "result_decode",
    "named_result_slot_count_mismatch",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);
});

test("subtype sessions register exact concrete constructors without base substitution", () => {
  const exact = diagnosticQuerySession();
  exact.var(RuntimeQueryParty, "exact");
  const exactModels = registeredModelConstructors(exact);
  assert.equal(exactModels.get(RuntimeQueryParty.typeName), RuntimeQueryParty);
  assert.equal(exactModels.has(RuntimeQueryEmployee.typeName), false);

  const subtypes = diagnosticQuerySession();
  subtypes.registerModels(RuntimeQueryEmployee);
  subtypes.var(RuntimeQueryParty, "subtypes");
  const subtypeModels = registeredModelConstructors(subtypes);
  assert.equal(subtypeModels.get(RuntimeQueryEmployee.typeName), RuntimeQueryEmployee);

  const relation = diagnosticQuerySession();
  relation.registerModels(RuntimeQueryEmployee);
  relation.var(RuntimeQueryPartyAssociation);
  assert.equal(
    registeredModelConstructors(relation).get(RuntimeQueryEmployee.typeName),
    RuntimeQueryEmployee,
  );

  const query = Object.freeze({});
  const employee = Object.freeze({
    iid: () => "0x-runtime-employee",
    concreteDescriptor: () => `entity:${RuntimeQueryEmployee.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["name"],
    fieldValuesJson: () => JSON.stringify([{ String: "Alice" }]),
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const result = Object.freeze({
    rowCount: () => 1,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => null,
    slotCount: () => 1,
    slotThing: () => employee,
  });

  expectMatchError(
    () => materializeValidatedOne(query as never, result as never, exactModels),
    "result_decode",
    "unregistered_result_model",
  );
  const hydrated = materializeValidatedOne(
    query as never,
    result as never,
    subtypeModels,
  ) as RuntimeQueryEmployee;
  assert.ok(hydrated instanceof RuntimeQueryEmployee);
  assert.equal(hydrated._iid, "0x-runtime-employee");
});

test("constructor collisions cannot rewrite earlier immutable query metadata", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeQueryPerson);
  const earlier = session.query(person);
  const before = registeredModelConstructors(session);
  assert.equal(before.get(RuntimeQueryPerson.typeName), RuntimeQueryPerson);

  expectMatchError(
    () => session.registerModels(RuntimeQueryPersonCollision),
    "invalid_plan",
    "model_constructor_conflict",
  );
  assert.equal(
    registeredModelConstructors(session).get(RuntimeQueryPerson.typeName),
    RuntimeQueryPerson,
  );
  expectMatchError(
    () => earlier.one(),
    "invalid_plan",
    "execution_connection_required",
  );
});

test("relation-valued role players pass planning before execution", () => {
  const session = diagnosticQuerySession();
  const envelope = session.var(RuntimeMaterializedEnvelope);
  const membership = session.var(RuntimeMaterializedMembership);
  const query = session
    .query(envelope)
    .match(membership)
    .where(envelope.role(envelopeRefs.roles.nested).connects(membership));

  expectMatchError(
    () => query.one(),
    "invalid_plan",
    "execution_connection_required",
  );
});

test("validated relation roles preserve roots and materialize nested relations shallowly", () => {
  RuntimeMaterializedPerson.constructions = 0;
  const query = Object.freeze({});
  const person = Object.freeze({
    iid: () => "0x-repeated-person",
    concreteDescriptor: () => `entity:${RuntimeMaterializedPerson.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["value"],
    fieldValuesJson: () => JSON.stringify([{ Long: "7" }]),
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const membership = Object.freeze({
    iid: () => "0x-membership",
    concreteDescriptor: () => `relation:${RuntimeMaterializedMembership.typeName}`,
    thingKind: () => "relation" as const,
    fieldNames: () => ["code"],
    fieldValuesJson: () => JSON.stringify([{ Long: "11" }]),
    roleDataComplete: () => true,
    roleNames: () => ["member"],
    rolePlayerCount: (roleName: string) => roleName === "member" ? 2 : 0,
    rolePlayer: () => person,
  });
  const result = Object.freeze({
    rowCount: () => 1,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => null,
    slotCount: () => 1,
    slotThing: () => membership,
  });
  const models = new Map<string, typeof RuntimeMaterializedMembership | typeof RuntimeMaterializedPerson>([
    [RuntimeMaterializedMembership.typeName, RuntimeMaterializedMembership],
    [RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson],
  ]);
  const hydrated = materializeValidatedOne(
    query as never,
    result as never,
    models,
  ) as RuntimeMaterializedMembership;
  assert.ok(hydrated instanceof RuntimeMaterializedMembership);
  const members = hydrated.member as readonly RuntimeMaterializedPerson[];
  assert.ok(Array.isArray(members));
  assert.equal(members.length, 2);
  assert.equal(members[0]!._iid, "0x-repeated-person");
  assert.equal(members[1]!._iid, "0x-repeated-person");
  assert.notStrictEqual(members[0], members[1]);
  assert.equal(RuntimeMaterializedPerson.constructions, 2);
  assert.ok(Object.isFrozen(hydrated));
  assert.ok(Object.isFrozen(members));

  const wrongPlayer = Object.freeze({
    iid: () => "0x-wrong-company",
    concreteDescriptor: () => `entity:${RuntimeQueryCompany.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["name"],
    fieldValuesJson: () => JSON.stringify([{ String: "Not a person" }]),
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const wrongMembership = Object.freeze({
    ...membership,
    rolePlayerCount: () => 1,
    rolePlayer: () => wrongPlayer,
  });
  const wrongResult = Object.freeze({ ...result, slotThing: () => wrongMembership });
  const wrongModels = new Map<string,
    | typeof RuntimeMaterializedMembership
    | typeof RuntimeMaterializedPerson
    | typeof RuntimeQueryCompany
  >([
    [RuntimeMaterializedMembership.typeName, RuntimeMaterializedMembership],
    [RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson],
    [RuntimeQueryCompany.typeName, RuntimeQueryCompany],
  ]);
  RuntimeMaterializedPerson.constructions = 0;
  expectMatchError(
    () => materializeValidatedOne(query as never, wrongResult as never, wrongModels),
    "result_decode",
    "result_role_player_type_mismatch",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);

  const incompleteNested = Object.freeze({
    ...membership,
    roleDataComplete: () => false,
    roleNames: () => {
      throw new Error("shallow relation roles must not be inspected");
    },
    rolePlayerCount: () => {
      throw new Error("shallow relation role counts must not be inspected");
    },
    rolePlayer: () => {
      throw new Error("shallow relation players must not be expanded");
    },
  });
  const envelope = Object.freeze({
    iid: () => "0x-envelope",
    concreteDescriptor: () => `relation:${RuntimeMaterializedEnvelope.typeName}`,
    thingKind: () => "relation" as const,
    fieldNames: () => [],
    fieldValuesJson: () => null,
    roleDataComplete: () => true,
    roleNames: () => ["nested"],
    rolePlayerCount: () => 1,
    rolePlayer: () => incompleteNested,
  });
  const hostile = Object.freeze({ ...result, slotThing: () => envelope });
  const hostileModels = new Map<string,
    | typeof RuntimeMaterializedEnvelope
    | typeof RuntimeMaterializedMembership
    | typeof RuntimeMaterializedPerson
  >([
    [RuntimeMaterializedEnvelope.typeName, RuntimeMaterializedEnvelope],
    [RuntimeMaterializedMembership.typeName, RuntimeMaterializedMembership],
    [RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson],
  ]);
  RuntimeMaterializedPerson.constructions = 0;
  const hydratedEnvelope = materializeValidatedOne(
    query as never,
    hostile as never,
    hostileModels,
  ) as RuntimeMaterializedEnvelope;
  assert.ok(hydratedEnvelope instanceof RuntimeMaterializedEnvelope);
  assert.equal(hydratedEnvelope._iid, "0x-envelope");
  const nested = hydratedEnvelope.nested;
  assert.ok(nested instanceof RuntimeMaterializedMembership);
  assert.equal(nested._iid, "0x-membership");
  assert.equal(nested.code.value, 11n);
  assert.equal(nested.member, undefined);
  assert.deepEqual(Object.getOwnPropertyDescriptor(nested, "member"), {
    value: undefined,
    writable: false,
    enumerable: true,
    configurable: false,
  });
  assert.ok(Object.isFrozen(hydratedEnvelope));
  assert.ok(Object.isFrozen(nested));
  assert.equal(RuntimeMaterializedPerson.constructions, 0);

  const incompleteRoot = Object.freeze({
    ...result,
    slotThing: () => incompleteNested,
  });
  expectMatchError(
    () => materializeValidatedOne(query as never, incompleteRoot as never, hostileModels),
    "result_decode",
    "incomplete_relation_role_data",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);
});

test("hostile concrete fields and value tags fail before model construction", () => {
  RuntimeMaterializedPerson.constructions = 0;
  const query = Object.freeze({});
  const base = Object.freeze({
    iid: () => "0x-hostile-person",
    concreteDescriptor: () => `entity:${RuntimeMaterializedPerson.typeName}`,
    thingKind: () => "entity" as const,
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const resultFor = (thing: object) => Object.freeze({
    rowCount: () => 1,
    outputSlotCount: () => 1,
    outputSlotIsCollection: () => false,
    outputNames: () => null,
    slotCount: () => 1,
    slotThing: () => thing,
  });
  const models = new Map([[RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson]]);

  const unknownField = Object.freeze({
    ...base,
    fieldNames: () => ["invented"],
    fieldValuesJson: () => JSON.stringify([{ Long: "1" }]),
  });
  expectMatchError(
    () => materializeValidatedRows(query as never, resultFor(unknownField) as never, models),
    "result_decode",
    "unknown_result_field",
  );

  const wrongTag = Object.freeze({
    ...base,
    fieldNames: () => ["value"],
    fieldValuesJson: () => JSON.stringify([{ String: "1" }]),
  });
  expectMatchError(
    () => materializeValidatedRows(query as never, resultFor(wrongTag) as never, models),
    "result_decode",
    "invalid_result_attribute_value",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);
});

test("validated named and positional pages freeze collections and preserve repeated IID multiplicity", () => {
  RuntimeMaterializedPerson.constructions = 0;
  const query = Object.freeze({});
  const company = Object.freeze({
    iid: () => "0x-page-company",
    concreteDescriptor: () => `entity:${RuntimeQueryCompany.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["name"],
    fieldValuesJson: () => JSON.stringify([{ String: "Acme" }]),
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const person = Object.freeze({
    iid: () => "0x-repeated-page-person",
    concreteDescriptor: () => `entity:${RuntimeMaterializedPerson.typeName}`,
    thingKind: () => "entity" as const,
    fieldNames: () => ["value"],
    fieldValuesJson: () => JSON.stringify([{ Long: "9007199254740993" }]),
    roleDataComplete: () => true,
    roleNames: () => [],
    rolePlayerCount: () => 0,
    rolePlayer: () => {
      throw new Error("entity has no role players");
    },
  });
  const namedResult = Object.freeze({
    pageEntryCount: () => 1,
    pageOffset: () => 0n,
    pageLimit: () => 1n,
    pageTotal: () => 9007199254740993n,
    outputSlotCount: () => 2,
    outputSlotIsCollection: (_owner: object, slotIndex: number) => slotIndex === 1,
    outputNames: () => ["company", "people"],
    pageSlotCount: () => 2,
    pageSlotValueCount: (_owner: object, _entry: number, slotIndex: number) =>
      slotIndex === 0 ? 1 : 2,
    pageSlotThing: (
      _owner: object,
      _entry: number,
      slotIndex: number,
    ) => slotIndex === 0 ? company : person,
  });
  const models = new Map<string,
    typeof RuntimeQueryCompany | typeof RuntimeMaterializedPerson
  >([
    [RuntimeQueryCompany.typeName, RuntimeQueryCompany],
    [RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson],
  ]);

  const page = materializeValidatedPage(
    query as never,
    namedResult as never,
    models,
    0n,
    1n,
    true,
  ) as Readonly<{
    items: readonly Readonly<{
      company: RuntimeQueryCompany;
      people: readonly RuntimeMaterializedPerson[];
    }>[];
    offset: number;
    limit: number;
    total: bigint | undefined;
  }>;
  assert.equal(page.total, 9007199254740993n);
  assert.equal(page.items[0]!.people.length, 2);
  assert.equal(page.items[0]!.people[0]!.value.value, 9007199254740993n);
  assert.equal(page.items[0]!.people[0]!._iid, "0x-repeated-page-person");
  assert.equal(page.items[0]!.people[1]!._iid, "0x-repeated-page-person");
  assert.notStrictEqual(page.items[0]!.people[0], page.items[0]!.people[1]);
  assert.ok(Object.isFrozen(page));
  assert.ok(Object.isFrozen(page.items));
  assert.ok(Object.isFrozen(page.items[0]));
  assert.ok(Object.isFrozen(page.items[0]!.people));
  assert.throws(
    () => (page.items[0]!.people as RuntimeMaterializedPerson[]).pop(),
    TypeError,
  );

  const positionalResult = Object.freeze({
    ...namedResult,
    outputNames: () => null,
  });
  const positional = materializeValidatedPage(
    query as never,
    positionalResult as never,
    models,
    0n,
    1n,
    true,
  ) as Readonly<{
    items: readonly (readonly [
      RuntimeQueryCompany,
      readonly RuntimeMaterializedPerson[],
    ])[];
  }>;
  assert.ok(Object.isFrozen(positional.items[0]));
  assert.ok(Object.isFrozen(positional.items[0]![1]));
  assert.equal(positional.items[0]![1].length, 2);
});

test("page shape, window, and token mismatches fail before constructors", () => {
  RuntimeMaterializedPerson.constructions = 0;
  const query = Object.freeze({});
  const base = Object.freeze({
    pageEntryCount: () => 1,
    pageOffset: () => 0n,
    pageLimit: () => 1n,
    pageTotal: () => null,
    outputSlotCount: () => 2,
    outputSlotIsCollection: () => false,
    outputNames: () => null,
    pageSlotCount: () => 2,
    pageSlotValueCount: () => 1,
    pageSlotThing: () => {
      throw new Error("page thing access must not occur after shape failure");
    },
  });
  const models = new Map([[RuntimeMaterializedPerson.typeName, RuntimeMaterializedPerson]]);

  expectMatchError(
    () => materializeValidatedPage(query as never, base as never, models, 0n, 1n, false),
    "result_decode",
    "invalid_page_output_shape",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);

  const wrongWindow = Object.freeze({ ...base, pageOffset: () => 1n });
  expectMatchError(
    () => materializeValidatedPage(
      query as never,
      wrongWindow as never,
      models,
      0n,
      1n,
      false,
    ),
    "result_decode",
    "result_page_window_mismatch",
  );

  const foreign = Object.freeze({
    pageEntryCount: () => {
      throw new Error(JSON.stringify({
        category: "result_decode",
        code: "request_token_mismatch",
        message: "foreign invocation",
        path: [{ kind: "result" }],
        details: {},
      }));
    },
  });
  expectMatchError(
    () => materializeValidatedPage(query as never, foreign as never, models, 0n, 1n, false),
    "result_decode",
    "request_token_mismatch",
  );
  assert.equal(RuntimeMaterializedPerson.constructions, 0);
});

test("validated count and exists remain lossless and operation-typed", () => {
  const query = Object.freeze({});
  assert.equal(
    materializeValidatedCount(
      query as never,
      { countValue: () => 18446744073709551615n } as never,
    ),
    18446744073709551615n,
  );
  assert.equal(
    materializeValidatedExists(
      query as never,
      { existsValue: () => true } as never,
    ),
    true,
  );
  expectMatchError(
    () => materializeValidatedCount(
      query as never,
      { countValue: () => 18446744073709551616n } as never,
    ),
    "result_decode",
    "invalid_result_count",
  );
  expectMatchError(
    () => materializeValidatedExists(
      query as never,
      { existsValue: () => 1 } as never,
    ),
    "result_decode",
    "invalid_result_exists",
  );
});
