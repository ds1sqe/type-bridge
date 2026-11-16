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
    () => db.executeQuery("match;"),
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
  await assert.rejects(db.rawTransaction(), (error: unknown) => error instanceof PlanError && error.code === "resource_in_use");
});


test("migration facade failures preserve canonical diagnostic fields", async () => {
  const { MigrationCatalog, MigrationPlan, MigrationPreview, MigrationApprovalBuilder, MigrationApprovalSet, openMigrationCatalog, PlanError, createRustDatabaseFromNative } = await import("../../typescript/index.js");
  const diagnostic = { version: 1, category: "invalid_input", code: "migration_catalog_invalid", message: "invalid migration catalog", path: [], details: {} };
  const fail = () => { throw Object.assign(new Error(JSON.stringify(diagnostic)), { code: "GenericFailure" }); };
  const native = new Proxy({}, { get: fail });
  const catalog = new MigrationCatalog(native as ConstructorParameters<typeof MigrationCatalog>[0]);
  const plan = new MigrationPlan(native as ConstructorParameters<typeof MigrationPlan>[0]);
  const preview = new MigrationPreview(native as ConstructorParameters<typeof MigrationPreview>[0]);
  const approvalSet = new MigrationApprovalSet(native as ConstructorParameters<typeof MigrationApprovalSet>[0]);
  const approvals = new MigrationApprovalBuilder(native as ConstructorParameters<typeof MigrationApprovalBuilder>[0]);
  const db = createRustDatabaseFromNative(native as Parameters<typeof createRustDatabaseFromNative>[0]);
  assert.throws(() => openMigrationCatalog(new Uint8Array(), new Uint8Array()),
    (error: unknown) => error instanceof PlanError && error.code === "generated_schema_authority_invalid");
  const check = (error: unknown) => error instanceof PlanError && error.code === diagnostic.code && error.diagnosticMessage === diagnostic.message;
  for (const invoke of [
    () => catalog.register("step", async () => {}), () => catalog.length,
    () => catalog.fingerprintJson(), () => catalog.isEmpty(), () => catalog.heads(),
    () => catalog.entry(0), () => catalog.verify(db), () => catalog.appliedMigrations(db),
    () => catalog.previewApply([]), () => catalog.previewRollback([], []),
    () => plan.executionAuthorized, () => preview.direction, () => preview.executionAuthorized,
    () => preview.migrationCount(), () => preview.migration(0), () => preview.approvalBuilder(),
    () => preview.authorize(approvalSet),
    () => approvals.approve(0), () => approvals.finish(),
  ]) assert.throws(invoke, check);
  await assert.rejects(plan.execute(db, "holder"), check);
  await assert.rejects(plan.executeControlled(db, "holder"), check);
});
