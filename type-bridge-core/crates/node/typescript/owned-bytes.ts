import { Buffer } from "node:buffer";

const typedArrayPrototype = Object.getPrototypeOf(Uint8Array.prototype) as object;
function missingTypedArrayIntrinsic(name: string): never {
  throw new Error(`Node runtime does not expose TypedArray.prototype.${name}`);
}
const typedArrayByteLength = Object.getOwnPropertyDescriptor(
  typedArrayPrototype,
  "byteLength",
)?.get ?? missingTypedArrayIntrinsic("byteLength");
const typedArrayBuffer = Object.getOwnPropertyDescriptor(
  typedArrayPrototype,
  "buffer",
)?.get ?? missingTypedArrayIntrinsic("buffer");
const typedArrayByteOffset = Object.getOwnPropertyDescriptor(
  typedArrayPrototype,
  "byteOffset",
)?.get ?? missingTypedArrayIntrinsic("byteOffset");
const applyIntrinsic = Reflect.apply;
const OwnedUint8Array = Uint8Array;
const copyBufferFrom = Buffer.from;

/**
 * Copy caller-controlled bytes into ordinary owned memory before N-API sees
 * them. This also detaches SharedArrayBuffer-backed and Buffer views so another
 * JavaScript worker cannot mutate memory while Rust borrows it. Copy at most
 * one byte beyond the relevant wire ceiling so an oversized input preserves
 * the native resource-limit result without an attacker-sized allocation.
 *
 * Read the view's internal slots through captured TypedArray intrinsics. Own
 * properties, subclass species, and patched `subarray` methods are therefore
 * unable to widen the bounded copy or return shared storage in its place.
 */
export function ownedByteSnapshot(bytes: Uint8Array, maxBytes: number): Buffer {
  if (
    !Number.isSafeInteger(maxBytes) ||
    maxBytes < 0 ||
    maxBytes >= Number.MAX_SAFE_INTEGER
  ) {
    throw new RangeError("maxBytes must be a non-negative safe byte ceiling");
  }
  const byteLength = applyIntrinsic(typedArrayByteLength, bytes, []) as number;
  const byteOffset = applyIntrinsic(typedArrayByteOffset, bytes, []) as number;
  const backing = applyIntrinsic(typedArrayBuffer, bytes, []) as ArrayBufferLike;
  const snapshotLength = Math.min(byteLength, maxBytes + 1);
  const snapshotView = new OwnedUint8Array(
    backing,
    byteOffset,
    snapshotLength,
  );
  return copyBufferFrom(snapshotView);
}
