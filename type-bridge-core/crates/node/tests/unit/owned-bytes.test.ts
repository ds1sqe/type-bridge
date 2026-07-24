import assert = require("node:assert/strict");
import test = require("node:test");

import { ownedByteSnapshot } from "../../typescript/owned-bytes.js";

test("N-API byte snapshots detach Buffer and SharedArrayBuffer storage", () => {
  const buffer = Buffer.from([1, 2, 3]);
  const bufferSnapshot = ownedByteSnapshot(buffer, 3);
  buffer[0] = 9;
  assert.deepEqual([...bufferSnapshot], [1, 2, 3]);

  const shared = new Uint8Array(new SharedArrayBuffer(3));
  shared.set([4, 5, 6]);
  const sharedSnapshot = ownedByteSnapshot(shared, 3);
  Atomics.store(shared, 0, 9);
  assert.deepEqual([...sharedSnapshot], [4, 5, 6]);
  assert.notEqual(sharedSnapshot.buffer, shared.buffer);
});

test("N-API byte snapshots retain only the ceiling oversize marker", () => {
  const bytes = Buffer.alloc(32, 0x5a);
  const snapshot = ownedByteSnapshot(bytes, 7);
  assert.equal(snapshot.byteLength, 8);
  assert.deepEqual([...snapshot], [...bytes.subarray(0, 8)]);
});

test("N-API byte snapshots ignore hostile view properties and species", () => {
  class HostileBytes extends Uint8Array {
    static get [Symbol.species](): never {
      throw new Error("typed-array species was inspected");
    }
  }

  const bytes = new HostileBytes(32);
  bytes.fill(0x5a);
  Object.defineProperty(bytes, "byteLength", { value: -1 });
  Object.defineProperty(bytes, "byteOffset", { value: 1 << 20 });
  Object.defineProperty(bytes, "buffer", {
    get(): never {
      throw new Error("view buffer property was inspected");
    },
  });
  Object.defineProperty(bytes, "subarray", {
    value(): never {
      throw new Error("view subarray method was inspected");
    },
  });

  const snapshot = ownedByteSnapshot(bytes, 7);
  assert.equal(snapshot.byteLength, 8);
  assert.deepEqual([...snapshot], Array(8).fill(0x5a));
});

test("N-API byte snapshots brand-check proxies without invoking traps", () => {
  let inspected = false;
  const proxy = new Proxy(new Uint8Array([1, 2, 3]), {
    get(): never {
      inspected = true;
      throw new Error("proxy property was inspected");
    },
  });

  assert.throws(
    () => ownedByteSnapshot(proxy, 3),
    /incompatible receiver|not a typed array/i,
  );
  assert.equal(inspected, false);
});

test("N-API byte snapshots reject invalid byte ceilings", () => {
  const bytes = new Uint8Array([1]);
  for (const limit of [-1, Number.POSITIVE_INFINITY, Number.MAX_SAFE_INTEGER]) {
    assert.throws(
      () => ownedByteSnapshot(bytes, limit),
      /maxBytes must be a non-negative safe byte ceiling/,
    );
  }
});
