import assert = require("node:assert/strict");
import test = require("node:test");

import { loadNative } from "../../typescript/native.js";
import { QueryExecutionResourceLimits } from "../../typescript/runtime-projection.js";

// The expected values are the addon's exports of the orm constants, never
// literals: a changed core default must change what TypeScript reports.
function coreDefaults(transport: "DIRECT" | "PROXY"): Record<string, bigint> {
  const native = loadNative();
  return {
    timeoutMilliseconds: native[`DEFAULT_${transport}_QUERY_TIMEOUT_MILLISECONDS`],
    items: native[`DEFAULT_${transport}_QUERY_ITEMS`],
    bytes: native[`DEFAULT_${transport}_QUERY_BYTES`],
    graphNodes: native[`DEFAULT_${transport}_QUERY_GRAPH_NODES`],
    attributeValues: native[`DEFAULT_${transport}_QUERY_ATTRIBUTE_VALUES`],
    collectionMembers: native[`DEFAULT_${transport}_QUERY_COLLECTION_MEMBERS`],
    rolePlayers: native[`DEFAULT_${transport}_QUERY_ROLE_PLAYERS`],
    statements: native[`DEFAULT_${transport}_QUERY_STATEMENTS`],
  };
}

function reported(limits: QueryExecutionResourceLimits): Record<string, bigint> {
  return {
    timeoutMilliseconds: limits.timeoutMilliseconds,
    items: limits.items,
    bytes: limits.bytes,
    graphNodes: limits.graphNodes,
    attributeValues: limits.attributeValues,
    collectionMembers: limits.collectionMembers,
    rolePlayers: limits.rolePlayers,
    statements: limits.statements,
  };
}

test("both transport default sets are exported", () => {
  const direct = coreDefaults("DIRECT");
  const proxy = coreDefaults("PROXY");
  for (const value of [...Object.values(direct), ...Object.values(proxy)]) {
    assert.equal(typeof value, "bigint");
  }
  // A direct session keeps the 2.2 in-process bounds; a proxy connection is
  // bounded by what one remote envelope carries.
  assert.deepEqual(direct, {
    timeoutMilliseconds: 30_000n,
    items: 100_000n,
    bytes: 64n * 1024n * 1024n,
    graphNodes: 1_000_000n,
    attributeValues: 4_000_000n,
    collectionMembers: 1_000_000n,
    rolePlayers: 1_000_000n,
    statements: 3n,
  });
  assert.deepEqual(proxy, {
    timeoutMilliseconds: 30_000n,
    items: 65_536n,
    bytes: 32n * 1024n * 1024n,
    graphNodes: 65_536n,
    attributeValues: 65_536n,
    collectionMembers: 65_536n,
    rolePlayers: 65_536n,
    statements: 3n,
  });
  assert.equal("DEFAULT_QUERY_ITEMS" in loadNative(), false);
});

test("a session's default answer policy is the direct default", () => {
  // QuerySession uses `new QueryExecutionResourceLimits()` when given none.
  assert.deepEqual(reported(new QueryExecutionResourceLimits()), coreDefaults("DIRECT"));
});

test("an omitted dimension keeps the direct default beside a given one", () => {
  const limits = new QueryExecutionResourceLimits({ items: 5n, statements: 1n });
  assert.deepEqual(reported(limits), {
    ...coreDefaults("DIRECT"),
    items: 5n,
    statements: 1n,
  });
});

test("byte and code ceilings are exported by the addon", () => {
  const native = loadNative();
  for (const value of [
    native.MAX_CANONICAL_BYTES,
    native.MAX_REMOTE_ENVELOPE_BYTES,
    native.MAX_DIAGNOSTIC_CODE_BYTES,
  ]) {
    assert.ok(Number.isSafeInteger(value) && value > 0);
  }
});
