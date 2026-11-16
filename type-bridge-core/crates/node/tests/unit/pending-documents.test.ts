import assert = require("node:assert/strict");
import test = require("node:test");
import { Pending } from "../../typescript/staged-transaction.js";
import type { NativePendingPlanAnswer } from "../../typescript/native.js";

test("documents observes the same answer without invoking its typed decoder", () => {
  let committed = false;
  let answer = { kind: "documents", documents: [{ _iid: "0x01", attributes: { stock: 4 } }] };
  const native: NativePendingPlanAnswer = {
    getJson: () => {
      if (!committed) throw new Error("not committed");
      return JSON.stringify(answer);
    },
    getProjection: () => null,
    getQueryResult: () => { throw new Error("unexpected query projection"); },
  };
  let hydrated = 0;
  const pending = new Pending(native, () => { hydrated++; return "typed"; });
  assert.throws(() => pending.documents(), /not committed/);
  committed = true;
  const documents = pending.documents();
  assert.deepEqual(documents, answer.documents);
  assert.equal(hydrated, 0);
  (documents[0]!["attributes"] as { stock: number }).stock = 99;
  assert.deepEqual(pending.documents(), answer.documents);
  assert.equal(pending.result(), "typed");
  assert.equal(hydrated, 1);
  answer = { kind: "count", documents: [] };
  assert.throws(() => pending.documents(), /plan_result_shape/);
});


test("documents rejects typed-query pendings with the shared plan shape diagnostic", () => {
  const unexpected = (): never => { throw new Error("documents must not resolve a typed query"); };
  const native: NativePendingPlanAnswer = {
    getJson: unexpected, getProjection: unexpected, getQueryResult: unexpected,
  };
  const pending = new Pending(native, unexpected, true);
  assert.throws(() => pending.documents(), /^TypeError: plan_result_shape:/);
});

test("plan documents and rows retain unsafe integers without rounding", () => {
  for (const kind of ["documents", "rows"] as const) {
    const json = `{"kind":"${kind}","${kind}":[{"integer":9007199254740995,"nested":[-9007199254740995,4,1.25],"text":"9007199254740995"}]}`;
    const native: NativePendingPlanAnswer = {
      getJson: () => json,
      getProjection: () => null,
      getQueryResult: () => { throw new Error("unexpected query projection"); },
    };
    const pending = new Pending(native);
    const result = pending.result();
    assert.ok(result.kind === "rows" || result.kind === "documents");
    const values = result.kind === "rows" ? result.rows : result.documents;
    const expected = { integer: 9007199254740995n, nested: [-9007199254740995n, 4, 1.25], text: "9007199254740995" };
    assert.deepEqual(values, [expected]);
    if (kind === "documents") assert.deepEqual(pending.documents(), [expected]);
  }
});
