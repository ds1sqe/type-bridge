import assert = require("node:assert/strict");
import test = require("node:test");
import { QueryV2Error, queryV2NativeCall } from "../../typescript/query-v2-internals.js";

test("native typed failure is preserved as immutable exception data", () => {
  const failure = { phase: "commit", reason: "conflict", lock_kind: "exclusive",
    annotation: null, op_index: null, retry_safe: true, code: "STC2" };
  try {
    queryV2NativeCall(() => { throw new Error(JSON.stringify({ version: 1,
      category: "transaction", sdk_category: "transaction", query_category: null,
      code: "commit_definitely_aborted", message: "The provider proves that the transaction did not commit",
      path: [], details: {}, failure })); });
    assert.fail("native error must propagate");
  } catch (error) {
    assert.ok(error instanceof QueryV2Error);
    assert.equal(error.code, "commit_definitely_aborted");
    assert.deepEqual(error.failure, failure);
    assert.ok(Object.isFrozen(error.failure));
  }
});
