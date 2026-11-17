import assert from "node:assert/strict";
import test from "node:test";
import { normalizeTimeoutMilliseconds, type TimeoutMilliseconds } from "../../typescript/timeouts.js";
import { QueryExecutionResourceLimits } from "../../typescript/runtime-projection.js";

test("all timeout inputs share safe-integer and u64 validation", () => {
  assert.equal(normalizeTimeoutMilliseconds(undefined), undefined);
  for (const input of [0, 1, 30000, Number.MAX_SAFE_INTEGER, 0n, 1n, 30000n, 0xffff_ffff_ffff_ffffn]) {
    assert.equal(normalizeTimeoutMilliseconds(input), BigInt(input));
  }
  for (const input of [-1, -1n, 0.5, NaN, Infinity, -Infinity, Number.MAX_SAFE_INTEGER + 1, 0x1_0000_0000_0000_0000n, "30000", null, true]) {
    assert.throws(() => normalizeTimeoutMilliseconds(input as TimeoutMilliseconds), /timeoutMilliseconds/);
  }
});

test("query resource timeout inputs accept numbers and bigint and keep bigint outputs", () => {
  for (const input of [0, 0n, 30000, 30000n]) {
    assert.equal(new QueryExecutionResourceLimits({ timeoutMilliseconds: input }).timeoutMilliseconds, BigInt(input));
  }
  assert.throws(() => new QueryExecutionResourceLimits({ timeoutMilliseconds: -1 }), /timeoutMilliseconds/);
});
