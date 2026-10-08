import assert from "node:assert/strict";
import test from "node:test";

import {
  createRustDatabaseFromNative,
  QueryV2Error,
  type NativeRustDatabase,
  type NativeRustTransactionContext,
  type RustDatabase,
  type RustTransactionContext,
} from "../../typescript/index.js";
import {
  InstalledRuntimeProjection,
  projectedManagerNativeCall,
  type RuntimeProjectionConnection,
} from "../../typescript/runtime-projection.js";
import {
  isRegisteredRustConnection,
  registerRustDatabaseHandle,
  registerRustTransactionHandle,
  rustDatabaseHandle,
} from "../../typescript/runtime-handles.js";

test("package-owned native databases are registered and retain idempotent close", () => {
  let closed = false;
  const native = {
    close: (): void => {
      closed = true;
    },
    isConnected: (): boolean => !closed,
  } as NativeRustDatabase;
  const database = createRustDatabaseFromNative(native);

  assert.equal(isRegisteredRustConnection(database), true);
  assert.equal(rustDatabaseHandle(database), native);
  assert.equal(database.isConnected(), true);
  database.close();
  database.close();
  assert.equal(database.isConnected(), false);
});

const nativeDiagnostic = Object.freeze({
  category: "cardinality",
  sdkCategory: "cardinality",
  queryCategory: "cardinality",
  code: "query_v2_expected_one",
  message: "the query returned more than one row",
  path: [{ kind: "operation" }],
  details: { actual: { kind: "count", value: "2" } },
});

function throwNativeDiagnostic(): never {
  throw new Error(JSON.stringify(nativeDiagnostic));
}

test("generated manager calls preserve structured SDK diagnostics", () => {
  assert.throws(() => projectedManagerNativeCall(throwNativeDiagnostic), (error: unknown) => {
    assert.ok(error instanceof QueryV2Error);
    assert.equal(error.category, nativeDiagnostic.category);
    assert.equal(error.sdkCategory, nativeDiagnostic.sdkCategory);
    assert.equal(error.queryCategory, nativeDiagnostic.queryCategory);
    assert.equal(error.code, nativeDiagnostic.code);
    assert.equal(error.diagnosticMessage, nativeDiagnostic.message);
    assert.deepEqual(error.path, nativeDiagnostic.path);
    assert.deepEqual(error.details, nativeDiagnostic.details);
    return true;
  });
});

// Direct terminals are native Promises; a failure arrives as a rejection.
const rejectNativeDiagnostic = async (): Promise<never> => throwNativeDiagnostic();

function directExecutionFixture(): {
  projection: InstalledRuntimeProjection;
  query: never;
} {
  const projection = new InstalledRuntimeProjection({
    revalidateMatchDiagnostic: (diagnostic: string): string => diagnostic,
  } as never);
  const query = {
    fetchRowsDiagnostic: () => "{}",
    pageByDiagnostic: () => "{}",
    countByDiagnostic: () => "{}",
    existsByDiagnostic: () => "{}",
    reduceByDiagnostic: () => "{}",
    reduceByFieldDiagnostic: () => "{}",
    reduceByFieldsDiagnostic: () => "{}",
    executeFetchRowsOwned: rejectNativeDiagnostic,
    executeFetchRowsBorrowed: rejectNativeDiagnostic,
    executePageByOwned: rejectNativeDiagnostic,
    executePageByBorrowed: rejectNativeDiagnostic,
    executeCountByOwned: rejectNativeDiagnostic,
    executeCountByBorrowed: rejectNativeDiagnostic,
    executeExistsByOwned: rejectNativeDiagnostic,
    executeExistsByBorrowed: rejectNativeDiagnostic,
    executeReduceByOwned: rejectNativeDiagnostic,
    executeReduceByBorrowed: rejectNativeDiagnostic,
    executeReduceByFieldOwned: rejectNativeDiagnostic,
    executeReduceByFieldBorrowed: rejectNativeDiagnostic,
    executeReduceByFieldsOwned: rejectNativeDiagnostic,
    executeReduceByFieldsBorrowed: rejectNativeDiagnostic,
  } as never;
  return { projection, query };
}

test("direct generated terminals project native diagnostics for both ownership paths", async () => {
  const database = {} as RustDatabase;
  registerRustDatabaseHandle(database, {} as NativeRustDatabase);
  const transaction = {} as RustTransactionContext;
  registerRustTransactionHandle(
    transaction,
    {} as NativeRustTransactionContext,
  );
  const { projection, query } = directExecutionFixture();
  const root = {} as never;
  const field = {} as never;
  const calls: readonly [
    string,
    (connection: RuntimeProjectionConnection) => Promise<unknown>,
  ][] = [
    ["rows", (connection) =>
      projection.direct(connection).rows(query, [], 0n, 1n, "exactly_one")],
    ["page", (connection) =>
      projection.direct(connection).page(query, root, [], 0n, 1n, true)],
    ["count", (connection) =>
      projection.direct(connection).count(query, root)],
    ["exists", (connection) =>
      projection.direct(connection).exists(query, root)],
    ["reduce", (connection) =>
      projection.direct(connection).reduce(query, root, null, ["count"], [null])],
    ["reduceByField", (connection) =>
      projection.direct(connection).reduceByField(
        query,
        root,
        field,
        ["count"],
        [null],
      )],
    ["reduceByFields", (connection) =>
      projection.direct(connection).reduceByFields(
        query,
        root,
        [field],
        ["count"],
        [null],
      )],
  ];

  for (const [ownership, connection] of [
    ["owned", database],
    ["borrowed", transaction],
  ] as const) {
    for (const [terminal, invoke] of calls) {
      await assert.rejects(invoke(connection), (error: unknown) => {
        assert.ok(error instanceof QueryV2Error, `${ownership} ${terminal}`);
        assert.equal(error.category, nativeDiagnostic.category);
        assert.equal(error.sdkCategory, nativeDiagnostic.sdkCategory);
        assert.equal(error.queryCategory, nativeDiagnostic.queryCategory);
        assert.equal(error.code, nativeDiagnostic.code);
        assert.equal(error.diagnosticMessage, nativeDiagnostic.message);
        assert.deepEqual(error.path, nativeDiagnostic.path);
        assert.deepEqual(error.details, nativeDiagnostic.details);
        return true;
      });
    }
  }
});
