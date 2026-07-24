import { Buffer } from "node:buffer";
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
export declare function ownedByteSnapshot(bytes: Uint8Array, maxBytes: number): Buffer;
