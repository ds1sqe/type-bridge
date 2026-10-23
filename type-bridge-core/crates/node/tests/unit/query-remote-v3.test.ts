// Rust owns embedded plan semantics and staged signature validation.
import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

const nativePath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
assert.ok(nativePath, "a freshly built native addon is required");
const native = require(nativePath) as {
  _query_plan_v3_round_trip(plan: Buffer, schema: Buffer): {
    bytes: Buffer;
    fingerprint: string;
  };
};

test("query-remote-v3 fixtures and Rust embedded plans", () => {
  const root = path.resolve("../contract/tests/fixtures/query-remote-v3");
  const fixtures = Object.fromEntries(fs.readdirSync(root)
    .filter(name => name.endsWith(".json"))
    .map(name => [name, JSON.parse(fs.readFileSync(path.join(root, name), "utf8"))]));
  assert.equal(Object.keys(fixtures).length, 10);
  const request = fixtures["request.json"];
  assert.equal(request.format, "typebridge.query-remote-request/v3");
  const schema = fs.readFileSync("../../../tests/contracts/plan_v3/schema.json");
  for (const name of ["request.json", "request-documents.json", "request-delete-snapshot.json"]) {
    for (const entry of fixtures[name].plans) {
      const original = Buffer.from(entry.plan_bytes, "hex");
      const encoded = native._query_plan_v3_round_trip(original, schema);
      assert.deepEqual(encoded.bytes, original);
      assert.equal(encoded.fingerprint.length, 64);
    }
  }
  for (const name of ["reply-committed.json", "reply-failed.json"]) {
    const outer = fixtures[name];
    assert.equal(outer.format, "typebridge.query-remote-signed-reply/v1");
    assert.equal(outer.payload.request, fixtures["request-fingerprint.json"].digest);
    assert.equal(outer.payload.nonce, request.nonce);
  }
});

import { PendingPlanAnswer } from "../../typescript/staged-transaction.js";

test("pending fetch documents retain nested attributes", () => {
  const documents = [{ _iid: "0x01", _type: "person", attributes: { name: ["Ada"] } }];
  const pending = new PendingPlanAnswer({ getJson: () => JSON.stringify({ kind: "documents", documents }) });
  const answer = pending.get();
  assert.equal(answer.kind, "documents");
  if (answer.kind === "documents") assert.deepEqual(answer.documents, documents);
});
