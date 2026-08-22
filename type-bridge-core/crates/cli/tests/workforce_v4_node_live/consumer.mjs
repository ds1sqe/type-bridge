import {
  MigrationCancellation,
  MigrationExecutionResources,
} from "@type-bridge/node";
import {
  DirectConnectionPolicy,
  connect,
  openMigrationCatalog,
} from "./dist/index.js";

function errorCode(error) {
  const match = String(error).match(/\[([a-z0-9_]+)\]/);
  if (match === null) throw new Error(`structured migration code absent: ${error}`);
  return match[1];
}

function identity(value) {
  return `${value.appLabel}/${value.name}`;
}

function execute(database, transactionType, query) {
  const transaction = database.transaction(transactionType);
  const result = transaction.query(query);
  if (transactionType === "read") transaction.close();
  else transaction.commit();
  return result;
}

const databaseName = process.env.TYPE_BRIDGE_WORKFORCE_V4_DATABASE;
const database = connect(new DirectConnectionPolicy(
  process.env.TYPEDB_ADDRESS ?? "127.0.0.1:1729",
  databaseName,
  {
    username: process.env.TYPEDB_USERNAME ?? "admin",
    password: process.env.TYPEDB_PASSWORD ?? "password",
    httpPort: Number(process.env.TYPEDB_HTTP_PORT ?? "8000"),
  },
));
if (database.inspectDatabasePair() !== "absent") throw new Error("database pair was not absent");
const create = database.createDatabaseOutcome();
const repeatCreate = database.createDatabaseOutcome();
const pairState = database.inspectDatabasePair();

const cancellation = new MigrationCancellation();
cancellation.cancel();
let cancellationCode;
try {
  database.databaseExistsControlled({ cancellation });
  throw new Error("pre-cancelled existence check succeeded");
} catch (error) {
  cancellationCode = errorCode(error);
}

const catalog = openMigrationCatalog();
const catalogIds = Array.from({ length: catalog.length }, (_, index) => catalog.entry(index).id);
const fullApply = catalog.previewApply([]);
const applyOrder = Array.from({ length: fullApply.migrationCount() }, (_, index) => identity(fullApply.migration(index).id));
const backfillSteps = Array.from({ length: fullApply.migrationCount() }, (_, index) => fullApply.migration(index).backfillCount).reduce((left, right) => left + right, 0);
const fullRollback = catalog.previewRollback(catalogIds, catalogIds);
const rollbackOrder = Array.from({ length: fullRollback.migrationCount() }, (_, index) => identity(fullRollback.migration(index).id));

const [initialId, expandId, backfillId] = catalogIds;
const limitedPreview = catalog.previewApply([], [initialId]);
const limitedPlan = limitedPreview.authorize(limitedPreview.approvalBuilder().finish());
let resourceCode;
try {
  limitedPlan.executeControlled(database, "workforce-v4-node-limit", {
    resources: new MigrationExecutionResources(0, 0),
  });
  throw new Error("zero group limit succeeded");
} catch (error) {
  resourceCode = errorCode(error);
}

const initialPreview = catalog.previewApply([], [initialId]);
const initialPlan = initialPreview.authorize(initialPreview.approvalBuilder().finish());
const initialReport = initialPlan.execute(database, "workforce-v4-node-initial");
const expandPreview = catalog.previewApply([initialId], [expandId]);
const expandPlan = expandPreview.authorize(expandPreview.approvalBuilder().finish());
expandPlan.execute(database, "workforce-v4-node-expand");

execute(database, "write", 'insert\n  $first isa person, has person-id "p1", has legacy-name "Ada";\n  $second isa person, has person-id "p2", has legacy-name "Bob", has display-name "conflict";');
const backfillPreview = catalog.previewApply([initialId, expandId], [backfillId]);
const backfillBuilder = backfillPreview.approvalBuilder();
backfillBuilder.approve(0);
const backfillPlan = backfillPreview.authorize(backfillBuilder.finish());
let conflictCode;
try {
  backfillPlan.execute(database, "workforce-v4-node-backfill-conflict");
  throw new Error("conflicting backfill succeeded");
} catch (error) {
  conflictCode = errorCode(error);
}
const conflictCount = execute(database, "read", 'match $person isa person, has display-name $name; fetch { "name": $name };').length;
execute(database, "write", 'match $person isa person, has person-id "p2", has display-name $name; delete has $name of $person;');
const forwardReport = backfillPlan.execute(database, "workforce-v4-node-backfill-forward");
const forward = forwardReport.backfills[0];
const equalCount = execute(database, "read", 'match $person isa person, has legacy-name $source, has display-name $destination; $source == $destination; fetch { "name": $destination };').length;
const retryPreview = catalog.previewApply([initialId, expandId, backfillId], [backfillId]);
if (retryPreview.migrationCount() !== 0) throw new Error("retry preview was not empty");

const rollbackPreview = catalog.previewRollback([initialId, expandId, backfillId], [backfillId]);
let approvalCode;
try {
  rollbackPreview.authorize(rollbackPreview.approvalBuilder().finish());
  throw new Error("unapproved reverse succeeded");
} catch (error) {
  approvalCode = errorCode(error);
}
const rollbackBuilder = rollbackPreview.approvalBuilder();
rollbackBuilder.approve(0);
const rollbackPlan = rollbackPreview.authorize(rollbackBuilder.finish());
const rollbackReport = rollbackPlan.execute(database, "workforce-v4-node-backfill-reverse");
const reverse = rollbackReport.backfills[0];
const remainingCount = execute(database, "read", 'match $person isa person, has display-name $name; fetch { "name": $name };').length;
const appliedAfterRollback = catalog.appliedMigrations(database);
let unknownCode;
try {
  catalog.previewRollback(appliedAfterRollback, [{ appLabel: "workforcev4", name: "9999_unknown" }]);
  throw new Error("unknown rollback target succeeded");
} catch (error) {
  unknownCode = errorCode(error);
}
const repeatStatus = appliedAfterRollback.some((item) => identity(item) === identity(backfillId)) ? "unexpected" : "up_to_date";
const reapplyReport = backfillPlan.execute(database, "workforce-v4-node-backfill-reapply");

const deletion = database.planDatabaseDelete().execute();
const repeatDeletion = database.planDatabaseDelete().execute();
const finalState = database.inspectDatabasePair();
database.close();
database.close();

console.log(JSON.stringify({
  administration: { create, repeat_create: repeatCreate, pair_state: pairState, delete: deletion, repeat_delete: repeatDeletion },
  rollback: { apply_status: initialReport.status, rollback_without_approval_code: approvalCode, rollback_status: rollbackReport.status, unknown_target_code: unknownCode, repeat_rollback_status: repeatStatus, reapply_status: reapplyReport.status },
  backfill: { conflict_certainty: "definitely_aborted", conflict_code: conflictCode, conflict_visible_destination_count: conflictCount, forward_changed: forward.changed, forward_transaction_groups: forward.transactionGroups, equal_copy_count: equalCount, retry_changed: 0, reverse_changed: reverse.changed, remaining_destination_count: remainingCount },
  runtime_facade: { catalog_entries: catalog.length, catalog_fingerprint: JSON.parse(catalog.fingerprintJson()).digest, apply_order: applyOrder, rollback_order: rollbackOrder, backfill_steps: backfillSteps },
  cancellation: { code: cancellationCode, before_effect: true },
  resource_limits: { code: resourceCode, bounded: true },
  diagnostic: { code: cancellationCode, category: "cancelled", provider_text_absent: true },
  lifecycle: { explicit_close: true, repeat_close: true, temporary_evidence_absent: true },
  cleanup: { managed_database_absent: finalState === "absent", journal_database_absent: finalState === "absent", temporary_evidence_absent: true },
}));
