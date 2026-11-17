/** Millisecond timeout input; zero disables the deadline. */
export type TimeoutMilliseconds = number | bigint;

/** @internal Validate once before crossing the native boundary. */
export function normalizeTimeoutMilliseconds(value: TimeoutMilliseconds | undefined): bigint | undefined {
  if (value === undefined) return undefined;
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) {
    return BigInt(value);
  }
  if (typeof value === "bigint" && value >= 0n && value <= 0xffff_ffff_ffff_ffffn) {
    return value;
  }
  throw new TypeError("timeoutMilliseconds must be a non-negative safe integer number or a bigint within the u64 range; use 0 to disable");
}
