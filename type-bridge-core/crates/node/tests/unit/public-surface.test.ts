import assert from "node:assert/strict";
import test from "node:test";
import * as sdk from "../../typescript/public.js";

// @ts-expect-error Removed 2.x filter DTO.
import type { FilterInput } from "../../typescript/public.js";
// @ts-expect-error Removed 2.x aggregate DTO.
import type { AggregateInput } from "../../typescript/public.js";
// @ts-expect-error Removed 2.x attribute DTO.
import type { AttributeInput } from "../../typescript/public.js";
// @ts-expect-error Removed 2.x runtime-row DTO.
import type { RuntimeAttributeValue } from "../../typescript/public.js";

test("the Node package root has an exact generated-only runtime surface", () => {
  assert.deepEqual(Object.keys(sdk).filter(key => key !== "default").sort(), [
    "Condition", "Database", "Expr", "Handle", "Iid", "Matched", "MigrationApprovalBuilder",
    "MigrationApprovalSet", "MigrationCancellation", "MigrationCatalog", "MigrationExecutionResources",
    "MigrationPlan", "MigrationPreview", "MigrationTransaction", "Pending", "Plan", "PlanAuthority",
    "PlanBuilder", "PlanCondition", "PlanError", "PlanExpr", "PlanInput", "PlanInputValue", "PlanNewThing",
    "PlanRecording", "PlanTemplate", "PlanThing", "PlanValue", "RawQueryError", "RawTransaction",
    "StagedTransaction", "TransactionContext", "Value", "boolean", "date", "datetime", "datetimetz",
    "decimal", "double", "duration", "ensureDatabase", "long", "longFromNumberUnsafe", "openMigrationCatalog",
    "planFieldToken", "planInsert", "planInsertMany", "planMatch", "setDebugListener", "string",
  ]);
});
