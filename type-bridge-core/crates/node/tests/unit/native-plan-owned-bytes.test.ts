import assert = require("node:assert/strict");
import test = require("node:test");

import { loadNative } from "../../typescript/native.js";

type UnknownCall = (...args: unknown[]) => unknown;

function sharedBytes(values: readonly number[]): Uint8Array {
  const bytes = new Uint8Array(new SharedArrayBuffer(values.length));
  bytes.set(values);
  return bytes;
}

function assertDetachedSnapshot(
  captured: Uint8Array,
  source: Uint8Array,
  expected: readonly number[],
): void {
  Atomics.store(source, 0, 0xff);
  assert.deepEqual([...captured], expected);
  assert.notEqual(captured.buffer, source.buffer);
}

test("public loadNative snapshots native plan byte inputs before N-API", async (context) => {
  const nativePath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  assert.ok(nativePath, "native test path");

  // The test runner gives each unit file an isolated process. Patch the raw
  // addon before loadNative first sees it so the public facade can be observed
  // without asking Rust to parse deliberately synthetic bytes.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const raw = require(nativePath) as Record<string, UnknownCall>;
  const originals = new Map<string, UnknownCall>();
  const captured = new Map<string, Uint8Array[]>();
  const remember = (name: string, positions: readonly number[], result: unknown): UnknownCall => {
    const original = raw[name];
    assert.ok(original, `raw ${name}`);
    originals.set(name, original);
    return (...args: unknown[]): unknown => {
      captured.set(
        name,
        positions.map((position) => args[position] as Uint8Array),
      );
      return result;
    };
  };

  let synchronousDecodeFailures = 0;
  const rawModelContext = Object.freeze({});
  const rawModelPending = {
    requestBytes: (): Uint8Array => Buffer.from([0x52]),
    decodeReply: (response: Uint8Array): Promise<object> => {
      captured.set("decodeModelReply", [response]);
      if (response[0] === 0xee) {
        synchronousDecodeFailures += 1;
        throw new Error(synchronousDecodeFailures === 1
          ? "native synchronous decode failure" : "query_remote_reply_replayed");
      }
      return Promise.resolve(Object.freeze({}));
    },
  };
  const authority = Object.freeze({});
  raw["planAuthority"] = remember("planAuthority", [0], authority);
  raw["planRemoteModelContextWithResources"] = remember(
    "planRemoteModelContextWithResources",
    [1],
    rawModelContext,
  );
  raw["planPrepareRemoteModelRows"] = remember(
    "planPrepareRemoteModelRows",
    [],
    rawModelPending,
  );
  context.after(() => {
    for (const [name, original] of originals) {
      raw[name] = original;
    }
  });

  const native = loadNative() as unknown as Record<string, UnknownCall>;
  const declared = sharedBytes([1, 2, 3]);
  native["planAuthority"](declared, "scope", "profile");
  assertDetachedSnapshot(
    captured.get("planAuthority")![0],
    declared,
    [1, 2, 3],
  );

  const modelAdvertisement = sharedBytes([22, 23, 24]);
  const modelContext = native["planRemoteModelContextWithResources"](
    authority,
    modelAdvertisement,
    {} as never,
    {} as never,
  );
  assert.equal(modelContext, rawModelContext);
  assertDetachedSnapshot(
    captured.get("planRemoteModelContextWithResources")![0],
    modelAdvertisement,
    [22, 23, 24],
  );
  const modelPending = native["planPrepareRemoteModelRows"](
    {},
    modelContext,
    [],
    0n,
    1n,
    "exactly_one",
  ) as { decodeReply(response: Uint8Array): Promise<object> };
  const modelReply = sharedBytes([25, 26, 27]);
  assert.deepEqual(await modelPending.decodeReply(modelReply), {});
  assertDetachedSnapshot(
    captured.get("decodeModelReply")![0],
    modelReply,
    [25, 26, 27],
  );

  const failingPending = native["planPrepareRemoteModelRows"](
    {}, modelContext, [], 0n, 1n, "exactly_one",
  ) as { decodeReply(response: Uint8Array): Promise<object> };
  assert.throws(
    () => failingPending.decodeReply(sharedBytes([0xee])),
    /native synchronous decode failure/,
  );
  assert.equal(
    synchronousDecodeFailures,
    1,
    "the facade must not retry a claimed native decode",
  );

  const descriptor = Object.getOwnPropertyDescriptor(native, "planAuthority");
  assert.notEqual(descriptor?.value, raw["planAuthority"]);
  const reflectedDeclared = sharedBytes([28, 29, 30]);
  (descriptor?.value as UnknownCall)(reflectedDeclared, "scope", "profile");
  assertDetachedSnapshot(
    captured.get("planAuthority")![0],
    reflectedDeclared,
    [28, 29, 30],
  );
});
