import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import { Plan, PlanRecording } from "../../typescript/plan.js";
import { PlanAuthority } from "../../typescript/public.js";
import { QueryLimits } from "../../typescript/runtime-projection.js";

const fixture = JSON.parse(fs.readFileSync(path.resolve("tests/fixtures/plan-builder-purchase.json"), "utf8")) as {
  declared_hex: string; scope: string; profile: string;
};
const authority = new PlanAuthority(Buffer.from(fixture.declared_hex, "hex"), fixture.scope, fixture.profile);
const Item = { typeKey: '{"kind":"entity","label":"item"}' };

function match(): void {
  const recording = PlanRecording.current();
  recording.use(authority);
  recording.builder.exact(Item);
}

test("synchronous recording matches the explicit core builder", () => {
  const recorded: Plan<bigint> = Plan.build((p) => { match(); return p.count(); });
  const explicit = Plan.builder();
  explicit.exact(Item);
  explicit.count();
  assert.deepEqual(recorded.canonicalBytes(), explicit.build(authority).canonicalBytes());
  assert.throws(match, /plan_context_missing/);
});

test("nested recordings restore the outer context after errors", () => {
  const recorded = Plan.build((outer) => {
    match();
    assert.throws(() => Plan.build(() => { throw new Error("abort"); }), /abort/);
    assert.equal(PlanRecording.current(), outer);
    return outer.exists();
  });
  assert.ok(recorded.canonicalBytes().length);
  assert.throws(match, /plan_context_missing/);
});

test("a built template refuses further mutation", () => {
  const template = Plan.template();
  const input = template.input("stock", "long");
  template.row([input.value(1n)]);
  template.build((p) => { match(); return p.count(); });
  assert.throws(() => template.input("late", "long"), /plan_already_built/);
  assert.throws(() => template.optionalInput("late", "long"), /plan_already_built/);
  assert.throws(() => template.row([input.value(2n)]), /plan_already_built/);
  assert.throws(() => template.build((p) => { match(); return p.count(); }), /plan_already_built/);
});

test("recording and template capture the selected query limits", () => {
  const queryLimits = new QueryLimits({ canonical_bytes: 512n, canonical_string_bytes: 512n });
  assert.throws(() => Plan.build((p) => { match(); return p.count(); }, { queryLimits }), /canonical_json_too_large/);
  const template = Plan.template({ queryLimits });
  assert.throws(() => template.build((p) => { match(); return p.count(); }), /canonical_json_too_large/);
});

test("async callbacks are refused and continuations have no current context", async () => {
  let continuation: Promise<void> | undefined;
  assert.throws(() => Plan.build(() => {
    match();
    continuation = Promise.resolve().then(() => { assert.throws(match, /plan_context_missing/); });
    return continuation;
  }), /plan_context_async/);
  await continuation;
  assert.throws(match, /plan_context_missing/);
});
