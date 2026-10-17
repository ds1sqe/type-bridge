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
const root = path.resolve(process.cwd(), "../../../tests/contracts/plan_v3");
const manifest = JSON.parse(fs.readFileSync(path.join(root, "manifest.json"), "utf8")) as Record<string, string>;
const schema = fs.readFileSync(path.join(root, "schema.json"));
for (const [filename, fingerprint] of Object.entries(manifest)) {
  test(`plan-v3 canonical bytes and fingerprint: ${filename}`, () => {
    const original = fs.readFileSync(path.join(root, filename));
    const actual = native._query_plan_v3_round_trip(original, schema);
    assert.deepEqual(actual.bytes, original);
    assert.equal(actual.fingerprint, fingerprint);
  });
}
test("plan-v3 rejects noncanonical bytes", () => {
  const original = fs.readFileSync(path.join(root, Object.keys(manifest)[0]!));
  assert.throws(() => native._query_plan_v3_round_trip(Buffer.concat([original, Buffer.from("\n")]), schema));
});
