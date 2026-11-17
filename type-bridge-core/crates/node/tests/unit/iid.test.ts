import assert from "node:assert/strict";
import test from "node:test";
import { Iid, PlanError } from "../../typescript/index.js";

test("IID is nominal, immutable, and preserves provider spelling", () => {
  const iid = new Iid("0xAbCd");
  assert.equal(iid.toString(), "0xAbCd");
  assert.equal(JSON.stringify(iid), '"0xAbCd"');
  assert.equal(iid.equals(new Iid("0xAbCd")), true);
  assert.equal(iid.equals("0xAbCd"), false);
  assert.equal(Object.isFrozen(iid), true);
});

test("IID construction rejects invalid input through the shared engine", () => {
  for (const value of ["", "0x", "0X01", "0xgg", " 0x01", "0x" + "a".repeat(257)]) {
    assert.throws(() => new Iid(value), (error: unknown) =>
      error instanceof PlanError && error.code === "invalid_iid");
  }
});
