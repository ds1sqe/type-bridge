import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import { Handle, Plan, PlanRecording, planFieldToken, planMatch, type FieldToken, type PlanFieldToken, type PlanModelAuthoring } from "../../typescript/plan.js";
import type { Value } from "../../typescript/plan-values.js";
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

// Hand-built generated-model metadata: two entities of one package.
const issued = new Set<object>();
function authoring(label: string, attributes: readonly string[]): PlanModelAuthoring<never> {
  const model = { typeKey: JSON.stringify({ kind: "entity", label }) };
  const fields = Object.fromEntries(attributes.map((attribute) => [attribute, {
    token: planFieldToken<bigint, PlanFieldToken>({ kind: "field", owner: model.typeKey, metadata: { declaring_id: { attribute, owner: { kind: "entity", label } } } }),
    kind: "long" as const,
    site: attribute,
    encode: (value: unknown) => value,
  }]));
  for (const field of Object.values(fields)) issued.add(field.token);
  return {
    model,
    projection: () => undefined as never,
    decodeRows: () => [],
    authority: () => authority,
    isFieldToken: (token) => issued.has(token),
    fields,
    roles: [],
    decode: () => [],
    decodeScalars: () => [],
    encodeScalar: (_kind, value) => value,
  };
}
const ItemModel = authoring("item", ["price"]);
const AccountModel = authoring("account", ["balance"]);
const balanceToken = AccountModel.fields.balance.token as PlanFieldToken & FieldToken<bigint>;
const priceToken = ItemModel.fields.price.token as PlanFieldToken & FieldToken<bigint>;

test("a class token on the right resolves against the handle receiving where()", () => {
  const classToken = Plan.build((p) => {
    const item = planMatch(ItemModel, Handle);
    planMatch(AccountModel, Handle).where(item.field<bigint>("price").le(balanceToken));
    return p.count();
  });
  const bound = Plan.build((p) => {
    const item = planMatch(ItemModel, Handle);
    const buyer = planMatch(AccountModel, Handle);
    buyer.where(item.field<bigint>("price").le(buyer.field<bigint>("balance")));
    return p.count();
  });
  const explicit = Plan.builder();
  const item = explicit.exact(ItemModel.model);
  const buyer = explicit.exact(AccountModel.model);
  const price = explicit.field(item, priceToken);
  explicit.where(price.le(explicit.field(buyer, balanceToken))).count();
  assert.deepEqual(classToken.canonicalBytes(), bound.canonicalBytes());
  assert.deepEqual(classToken.canonicalBytes(), explicit.build(authority).canonicalBytes());
});

test("a right-hand class token of another model is refused", () => {
  assert.throws(() => Plan.build((p) => {
    const item = planMatch(ItemModel, Handle);
    planMatch(AccountModel, Handle).where(item.field<bigint>("price").le(priceToken));
    return p.count();
  }), (error: unknown) => error instanceof TypeError && /does not belong to this model/.test(error.message));
  assert.throws(() => Plan.build((p) => {
    p.require(planMatch(ItemModel, Handle).field<bigint>("price").le(priceToken));
    return p.count();
  }), /class field tokens need a handle's where\(\)/);
});

test("a right-hand class token must carry the value's scalar type", () => {
  const textToken = priceToken as unknown as PlanFieldToken & FieldToken<string>;
  const check = (value: Value<bigint>) => {
    // @ts-expect-error a string field cannot compare with a bigint value
    value.le(textToken);
  };
  assert.equal(typeof check, "function");
});
