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

test("public loadNative snapshots every V2 byte input before N-API", async (context) => {
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
  const rawPending = {
    requestBytes: (): Uint8Array => Buffer.from([0x51]),
    decodeReply: (response: Uint8Array): Promise<string> => {
      captured.set("decodeReply", [response]);
      if (response[0] === 0xee) {
        synchronousDecodeFailures += 1;
        throw new Error(
          synchronousDecodeFailures === 1
            ? "native synchronous decode failure"
            : "query_remote_reply_replayed",
        );
      }
      return Promise.resolve("decoded");
    },
  };
  const authority = Object.freeze({});
  raw["queryV2Authority"] = remember("queryV2Authority", [0], authority);
  raw["queryV2QueryOnlyAuthority"] = remember(
    "queryV2QueryOnlyAuthority",
    [1],
    authority,
  );
  raw["queryV2ExecuteLocal"] = remember(
    "queryV2ExecuteLocal",
    [2],
    Promise.resolve("local"),
  );
  raw["queryV2RemoteCapabilities"] = remember(
    "queryV2RemoteCapabilities",
    [0],
    [],
  );
  raw["queryV2PrepareRemote"] = remember(
    "queryV2PrepareRemote",
    [1, 3],
    rawPending,
  );
  context.after(() => {
    for (const [name, original] of originals) {
      raw[name] = original;
    }
  });

  const native = loadNative() as unknown as Record<string, UnknownCall>;
  const declared = sharedBytes([1, 2, 3]);
  native["queryV2Authority"](declared, "scope", "profile");
  assertDetachedSnapshot(
    captured.get("queryV2Authority")![0],
    declared,
    [1, 2, 3],
  );

  const queryOnlyDeclared = sharedBytes([4, 5, 6]);
  native["queryV2QueryOnlyAuthority"]({}, queryOnlyDeclared, "scope", "profile");
  assertDetachedSnapshot(
    captured.get("queryV2QueryOnlyAuthority")![0],
    queryOnlyDeclared,
    [4, 5, 6],
  );

  const plan = sharedBytes([7, 8, 9]);
  await native["queryV2ExecuteLocal"]({}, authority, plan, "{}", null);
  assertDetachedSnapshot(
    captured.get("queryV2ExecuteLocal")![0],
    plan,
    [7, 8, 9],
  );

  const capabilities = sharedBytes([10, 11, 12]);
  native["queryV2RemoteCapabilities"](capabilities);
  assertDetachedSnapshot(
    captured.get("queryV2RemoteCapabilities")![0],
    capabilities,
    [10, 11, 12],
  );

  const remotePlan = sharedBytes([13, 14, 15]);
  const advertisement = sharedBytes([16, 17, 18]);
  const pending = native["queryV2PrepareRemote"](
    authority,
    remotePlan,
    "{}",
    advertisement,
    1n,
    1n,
    1n,
    null,
  ) as { decodeReply(response: Uint8Array): Promise<string> };
  assertDetachedSnapshot(
    captured.get("queryV2PrepareRemote")![0],
    remotePlan,
    [13, 14, 15],
  );
  assertDetachedSnapshot(
    captured.get("queryV2PrepareRemote")![1],
    advertisement,
    [16, 17, 18],
  );

  const reply = sharedBytes([19, 20, 21]);
  assert.equal(await pending.decodeReply(reply), "decoded");
  assertDetachedSnapshot(captured.get("decodeReply")![0], reply, [19, 20, 21]);

  const failingPending = native["queryV2PrepareRemote"](
    authority,
    remotePlan,
    "{}",
    advertisement,
    1n,
    1n,
    1n,
    null,
  ) as { decodeReply(response: Uint8Array): Promise<string> };
  assert.throws(
    () => failingPending.decodeReply(sharedBytes([0xee])),
    /native synchronous decode failure/,
  );
  assert.equal(
    synchronousDecodeFailures,
    1,
    "the facade must not retry a claimed native decode",
  );

  const descriptor = Object.getOwnPropertyDescriptor(native, "queryV2Authority");
  assert.notEqual(descriptor?.value, raw["queryV2Authority"]);
  const reflectedDeclared = sharedBytes([22, 23, 24]);
  (descriptor?.value as UnknownCall)(reflectedDeclared, "scope", "profile");
  assertDetachedSnapshot(
    captured.get("queryV2Authority")![0],
    reflectedDeclared,
    [22, 23, 24],
  );
});
