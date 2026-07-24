import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

interface Corpus {
  readonly declared_b64: string;
  readonly plan_b64: string;
  readonly profile: string;
  readonly scope: string;
}

interface RawNative {
  queryV2Authority(
    declaredSchema: Uint8Array,
    scope: string,
    profile: string,
  ): object;
  queryV2PrepareRemote(
    authority: object,
    plan: Uint8Array,
    invocationJson: string,
    advertisement: Uint8Array,
    maxItems: bigint,
    maxBytes: bigint,
    maxCollectionMembers: bigint,
    deadlineMs?: bigint | null,
  ): object;
  queryV2RemoteCapabilities(advertisement: Uint8Array): string[];
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
  const corpusPath = path.resolve(
    process.cwd(),
    "../../../tests/fixtures/query-v2-remote-failures.json",
  );
  const corpus = JSON.parse(fs.readFileSync(corpusPath, "utf8")) as Corpus;
  const shared = sharedBuffer([0x7b, 0x7d]);

  assert.throws(
    () => raw.queryV2Authority(shared, corpus.scope, corpus.profile),
    /query_v2_shared_buffer_unsupported/,
  );
  assert.throws(
    () => raw.queryV2RemoteCapabilities(shared),
    /query_v2_shared_buffer_unsupported/,
  );

  const authority = raw.queryV2Authority(
    Buffer.from(corpus.declared_b64, "base64"),
    corpus.scope,
    corpus.profile,
  );
  assert.throws(
    () =>
      raw.queryV2PrepareRemote(
        authority,
        Buffer.from(
          new SharedArrayBuffer(Buffer.from(corpus.plan_b64, "base64").byteLength),
        ),
        "{}",
        Buffer.alloc(0),
        1n,
        1n,
        1n,
        null,
      ),
    /query_v2_shared_buffer_unsupported/,
  );

  assert.doesNotThrow(() => {
    try {
      raw.queryV2RemoteCapabilities(Buffer.from("{}"));
    } catch (error) {
      assert.doesNotMatch(String(error), /query_v2_shared_buffer_unsupported/);
    }
  });
});
