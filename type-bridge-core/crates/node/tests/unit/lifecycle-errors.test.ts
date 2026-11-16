import assert from "node:assert/strict";
import test from "node:test";

test("connection, administration and transaction failures use PlanError", async (context) => {
  const path = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  assert.ok(path);
  const raw = require(path) as Record<string, (...args: unknown[]) => unknown>;
  const diagnostic = { version: 1, category: "transaction", sdk_category: "transaction", code: "resource_in_use", message: "an active transaction retains the connection", path: [], details: {} };
  const fail = () => { throw Object.assign(new Error(JSON.stringify(diagnostic)), { code: "InvalidArg" }); };
  const originalConnect = raw["connectRustDatabase"];
  const originalEnsure = raw["ensureRustDatabase"];
  raw["connectRustDatabase"] = fail;
  raw["ensureRustDatabase"] = fail;
  context.after(() => {
    raw["connectRustDatabase"] = originalConnect;
    raw["ensureRustDatabase"] = originalEnsure;
  });
  const { Database, PlanError, ensureDatabase, createRustDatabaseFromNative } = await import("../../typescript/index.js");
  let rejecting = false;
  const native = new Proxy({}, { get: () => () => rejecting ? fail() : native });
  const db = createRustDatabaseFromNative(native as Parameters<typeof createRustDatabaseFromNative>[0]);
  const tx = db.transaction();
  const deletion = db.planDatabaseDelete();
  rejecting = true;
  const calls = [
    () => Database.connect("localhost:1", "unused"),
    () => ensureDatabase("localhost:1", "unused"),
    () => db.close(), () => db.isConnected(), () => db.databaseName(),
    () => db.databaseExists(), () => db.databaseExistsControlled(),
    () => db.createDatabase(), () => db.createDatabaseControlled(),
    () => db.createDatabaseOutcome(), () => db.createDatabaseOutcomeControlled(),
    () => db.deleteDatabase(), () => db.deleteDatabaseOutcome(),
    () => db.inspectDatabasePair(), () => db.inspectDatabasePairControlled(),
    () => db.planDatabaseDelete(), () => db.planDatabaseDeleteControlled(),
    () => db.resetDatabase(), () => db.transaction(), () => db.stagedTransaction(),
    () => tx.commit(), () => tx.rollback(), () => tx.close(), () => tx.transactionType(),
    () => deletion.inspectedState(), () => deletion.execute(), () => deletion.executeControlled(), () => deletion.close(),
  ];
  for (const invoke of calls) {
    assert.throws(invoke, (error: unknown) => {
      assert.ok(error instanceof PlanError);
      assert.equal(error.code, "resource_in_use");
      assert.equal(error.category, "transaction");
      assert.equal(error.diagnosticMessage, diagnostic.message);
      return true;
    });
  }
});
