/**
 * Shared cardinality predicate helpers for TypeScript code generation.
 *
 * Used by both renderEntities.ts and renderRelations.ts. Each predicate
 * mirrors the corresponding Python `Cardinality` property in models.py.
 */

import type { Cardinality } from "../parser.js";

/** True if at most one value is allowed (max === 1). */
export function isCardSingle(c: Cardinality): boolean {
  return c.max === 1;
}

/** True if at least one value is required (min >= 1). */
export function isCardRequired(c: Cardinality): boolean {
  return c.min >= 1;
}

/** True if multiple values are allowed (max === null || max > 1). */
export function isCardMulti(c: Cardinality): boolean {
  return c.max === null || c.max > 1;
}

/** True if zero or one value (min === 0, max === 1) — the optional-single case. */
export function isCardOptionalSingle(c: Cardinality): boolean {
  return c.min === 0 && c.max === 1;
}
