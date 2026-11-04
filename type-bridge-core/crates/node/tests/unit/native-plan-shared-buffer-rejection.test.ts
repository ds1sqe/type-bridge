import assert = require("node:assert/strict");
import test = require("node:test");

interface RawNative {
  planAuthority(
    declaredSchema: Uint8Array,
    scope: string,
    profile: string,
  ): object;
}

function sharedBuffer(values: readonly number[]): Buffer {
  const storage = new SharedArrayBuffer(values.length);
  const view = new Uint8Array(storage);
  view.set(values);
  return Buffer.from(storage);
}

test("directly loaded addon rejects SharedArrayBuffer before Rust borrowing", () => {
  const nativePath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  assert.ok(nativePath, "native test path");
  // Deliberately bypass the package facade: the shipped addon itself is the
  // security boundary for callers that require its artifact path directly.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const raw = require(nativePath) as RawNative;
  const shared = sharedBuffer([0x7b, 0x7d]);

  assert.throws(
    () => raw.planAuthority(shared, "shared-buffer", "typedb-3.13.6/v1"),
    /query_v2_shared_buffer_unsupported/,
  );
  assert.doesNotThrow(() => {
    try {
      raw.planAuthority(Buffer.from("{}"), "shared-buffer", "typedb-3.13.6/v1");
    } catch (error) {
      assert.doesNotMatch(String(error), /query_v2_shared_buffer_unsupported/);
    }
  });
});
