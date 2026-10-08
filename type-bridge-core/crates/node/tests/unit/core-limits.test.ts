import assert = require("node:assert/strict");
import test = require("node:test");

import { loadNative } from "../../typescript/native.js";
import { QueryExecutionResourceLimits } from "../../typescript/runtime-projection.js";

// The expected values are the addon's exports of the orm constants, never
// literals: a changed core default must change what TypeScript reports.
function coreDefaults(): Record<string, bigint> {
  const native = loadNative();
  return {
    timeoutMilliseconds: native.DEFAULT_QUERY_TIMEOUT_MILLISECONDS,
    items: native.DEFAULT_QUERY_ITEMS,
    bytes: native.DEFAULT_QUERY_BYTES,
    graphNodes: native.DEFAULT_QUERY_GRAPH_NODES,
    attributeValues: native.DEFAULT_QUERY_ATTRIBUTE_VALUES,
    collectionMembers: native.DEFAULT_QUERY_COLLECTION_MEMBERS,
    rolePlayers: native.DEFAULT_QUERY_ROLE_PLAYERS,
    statements: native.DEFAULT_QUERY_STATEMENTS,
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

test("a session's default answer policy is the core's", () => {
  const defaults = coreDefaults();
  for (const value of Object.values(defaults)) {
    assert.equal(typeof value, "bigint");
  }
  // QuerySession uses `new QueryExecutionResourceLimits()` when given none.
  assert.deepEqual(reported(new QueryExecutionResourceLimits()), defaults);
});

test("an omitted dimension keeps the core default beside a given one", () => {
  const limits = new QueryExecutionResourceLimits({ items: 5n, statements: 1n });
  assert.deepEqual(reported(limits), {
    ...coreDefaults(),
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
