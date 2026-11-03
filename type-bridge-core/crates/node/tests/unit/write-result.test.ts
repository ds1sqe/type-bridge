import assert from "node:assert/strict";
import test from "node:test";
import { writeResult } from "../../typescript/runtime-projection.js";

test("single writes retain every root and expose item only for one root", () => {
  for (const items of [[], [{ iid: "second" }], [{ iid: "second" }, { iid: "first" }]]) {
    const result = writeResult(items);
    assert.equal(result.count, items.length);
    assert.equal(result.items, items);
    assert.equal(result.item, items.length === 1 ? items[0] : null);
    assert.ok(Object.isFrozen(result));
    assert.ok(Object.isFrozen(result.items));
  }
});
