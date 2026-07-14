/** Immutable public envelope for one validated distinct-root page. */
export interface Page<T> {
  readonly items: readonly T[];
  readonly offset: number;
  readonly limit: number;
  readonly total: bigint | undefined;
}

/**
 * Preserve a native-validated page as defensively copied, frozen JavaScript
 * data. This helper is intentionally not re-exported from the package subpath;
 * typed query terminals are its only caller.
 */
export function pageFromValidatedResult<T>(
  items: readonly T[],
  offset: bigint,
  limit: bigint,
  total?: bigint,
): Page<T> {
  if (offset < 0n) {
    throw new RangeError("page offset must be non-negative");
  }
  if (limit <= 0n) {
    throw new RangeError("page limit must be positive");
  }
  if (total !== undefined && total < 0n) {
    throw new RangeError("page total must be non-negative");
  }
  const publicOffset = pageWindowNumber(offset, "offset");
  const publicLimit = pageWindowNumber(limit, "limit");

  const page: Page<T> = {
    items: Object.freeze(Array.from(items)),
    offset: publicOffset,
    limit: publicLimit,
    total,
  };
  return Object.freeze(page);
}

function pageWindowNumber(value: bigint, name: "offset" | "limit"): number {
  const result = Number(value);
  if (!Number.isSafeInteger(result)) {
    throw new RangeError(`page ${name} must fit a JavaScript safe integer`);
  }
  return result;
}
