import assert from "node:assert/strict";
import test from "node:test";
import { InstalledRuntimeProjection } from "../../typescript/runtime-projection.js";

test("every canonical record and archive input takes an owned snapshot", () => {
  let captured: unknown[] = [];
  const native = new Proxy({}, { get: (_, name) => (...args: unknown[]) => {
    captured = args;
    return String(name).startsWith("decodeArchive") ? [] : Buffer.from("{}");
  } });
  const projection = new InstalledRuntimeProjection(native as ConstructorParameters<typeof InstalledRuntimeProjection>[0]);
  const source = new Uint8Array(new SharedArrayBuffer(3));
  const calls: (() => unknown)[] = [
    ...["decodeAttributeJson", "decodeCreateJson", "decodeReferenceJson", "decodeSnapshotJson", "decodeStructJson"].map(name =>
      () => (projection as unknown as Record<string, (type: string, bytes: Uint8Array) => unknown>)[name]("type", source)),
    () => projection.decodeRecordJsonControlled("create", "type", source),
    () => projection.decodeArchive(source),
    () => projection.decodeArchiveControlled(source),
    () => projection.encodeArchive([source]),
    () => projection.encodeArchiveControlled([source]),
  ];
  for (const invoke of calls) {
    source.set([1, 2, 3]);
    invoke();
    const candidates = captured.flat().filter(value => value instanceof Uint8Array) as Uint8Array[];
    assert.equal(candidates.length, 1);
    Atomics.store(source, 0, 9);
    assert.deepEqual([...candidates[0]], [1, 2, 3]);
    assert.ok(candidates[0].buffer instanceof ArrayBuffer);
  }
});
