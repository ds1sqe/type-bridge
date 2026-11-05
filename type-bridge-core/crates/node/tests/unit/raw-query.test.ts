import assert from "node:assert/strict";
import test from "node:test";
import { RawQueryError, RawTransaction, rawNativeCall, rawNativePromise } from "../../typescript/raw-query.js";

const fields = { code: "commit_outcome_unknown", message: "exact provider message", status: 503, providerCode: "CONNECTION", commitOutcome: "unknown" };
const nativeError = () => new Error(`typebridge.raw:${JSON.stringify(fields)}`);
function check(error: unknown): boolean {
  assert.ok(error instanceof RawQueryError);
  assert.equal(error.code, fields.code);
  assert.equal(error.message, fields.message);
  assert.equal(error.status, fields.status);
  assert.equal(error.providerCode, fields.providerCode);
  assert.equal(error.commitOutcome, fields.commitOutcome);
  return true;
}

test("raw errors retain provider detail and commit certainty across native calls", async () => {
  assert.throws(() => rawNativeCall(() => { throw nativeError(); }), check);
  await assert.rejects(rawNativePromise(Promise.reject(nativeError())), check);
  const ordinary = new TypeError("not a raw error");
  assert.throws(() => rawNativeCall(() => { throw ordinary; }), (error: unknown) => error === ordinary);
});

test("raw facade preserves native answers and delegates explicit completion", async () => {
  const calls: string[] = [];
  const answer = { answers: [{ min: -9223372036854775808n, max: 9223372036854775807n, safe: 7, double: 1.25 }] };
  const transaction = new RawTransaction({
    async executeQuery(query) { calls.push(query); return answer; },
    async commit() { calls.push("commit"); },
    async rollback() { calls.push("rollback"); },
    async close() { calls.push("close"); },
  });
  assert.equal(await transaction.executeQuery("  exact TypeQL;\n"), answer);
  await transaction.commit();
  await transaction.rollback();
  await transaction.close();
  assert.deepEqual(calls, ["  exact TypeQL;\n", "commit", "rollback", "close"]);
});
