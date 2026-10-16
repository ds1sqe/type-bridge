import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import {
  Plan,
  PlanBuilder,
  PlanExpr,
  QueryV2Authority,
  QueryV2Error,
  decimal,
  type PlanFieldToken,
  type PlanModelToken,
  type PlanRoleToken,
} from "../../typescript/public.js";

// Written and checked by the Rust test
// `typescript_purchase_fixture_is_the_rust_hand_built_purchase`: the Rust
// SDK's hand-built purchase and the declared schema it was built under.
interface Fixture {
  readonly scope: string;
  readonly profile: string;
  readonly declared_hex: string;
  readonly purchase: { readonly order: string; readonly item_id: string; readonly account_id: string };
  readonly purchase_hex: string;
}

const fixture = JSON.parse(
  fs.readFileSync(path.resolve(process.cwd(), "tests/fixtures/plan-builder-purchase.json"), "utf8"),
) as Fixture;

function authority(): QueryV2Authority {
  return new QueryV2Authority(Buffer.from(fixture.declared_hex, "hex"), fixture.scope, fixture.profile);
}

// The shapes a generated package's tokens carry (`Item`, `Item.fields.stock`,
// `Purchase.roles.buyer`).
type Kind = "entity" | "relation";
function model<Id extends string>(kind: Kind, label: string): PlanModelToken<Id> {
  return { typeKey: JSON.stringify({ kind, label }) as Id };
}
function field<Id extends string>(owner: PlanModelToken<Id>, attribute: string): PlanFieldToken<Id> {
  return {
    kind: "field",
    owner: owner.typeKey,
    metadata: { declaring_id: { attribute, owner: JSON.parse(owner.typeKey) as unknown } },
  };
}
function role<Id extends string>(owner: PlanModelToken<Id>, label: string): PlanRoleToken<Id> {
  const relation = (JSON.parse(owner.typeKey) as { label: string }).label;
  return {
    kind: "role",
    owner: owner.typeKey,
    role: JSON.stringify({ declaring_relation: relation, label }),
  };
}

const Item = model<"item">("entity", "item");
const Account = model<"account">("entity", "account");
const Purchase = model<"purchase">("relation", "purchase");
const ItemFields = {
  itemId: field(Item, "item-id"),
  stock: field(Item, "stock"),
  price: field(Item, "price"),
};
const AccountFields = { accountId: field(Account, "account-id"), balance: field(Account, "balance") };
const PurchaseTokens = {
  orderId: field(Purchase, "order-id"),
  buyer: role(Purchase, "buyer"),
  item: role(Purchase, "item"),
};

/** The purchase as the guide writes it, in the Rust SDK's call order. */
function purchase(order: string, itemId: string, accountId: string): Plan {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const orderInput = plan.input("order_id", "string");
  const itemInput = plan.input("item_id", "string");
  const accountInput = plan.input("account_id", "string");
  const itemKey = plan.field(item, ItemFields.itemId);
  const buyerKey = plan.field(buyer, AccountFields.accountId);
  const stock = plan.field(item, ItemFields.stock);
  const price = plan.field(item, ItemFields.price);
  const balance = plan.field(buyer, AccountFields.balance);
  plan
    .where(itemKey.eq(itemInput))
    .where(buyerKey.eq(accountInput))
    .where(stock.gt(0n))
    .where(balance.ge(price));
  const nextStock = plan.let("next_stock", stock.sub(1n));
  const nextBalance = plan.let("next_balance", balance.sub(price));
  plan.update(item, ItemFields.stock, nextStock).update(buyer, AccountFields.balance, nextBalance);
  plan
    .insert(Purchase)
    .field(PurchaseTokens.orderId, orderInput)
    .player(PurchaseTokens.buyer, buyer)
    .player(PurchaseTokens.item, item);
  plan.row([orderInput.value(order), itemInput.value(itemId), accountInput.value(accountId)]);
  return plan.build(authority());
}

function buildError(build: () => unknown): QueryV2Error {
  try {
    build();
  } catch (error) {
    assert.ok(error instanceof QueryV2Error, `expected QueryV2Error, got ${String(error)}`);
    return error;
  }
  assert.fail("build unexpectedly succeeded");
}

test("the TypeScript purchase equals the Rust hand-built purchase byte for byte", () => {
  const { order, item_id, account_id } = fixture.purchase;
  const plan = purchase(order, item_id, account_id);
  assert.equal(Buffer.from(plan.canonicalBytes()).toString("hex"), fixture.purchase_hex);
  assert.equal(plan.writes(), true);
  assert.match(plan.fingerprint(), /^[0-9a-f]{64}$/);
  // Another row is another plan: different bytes, same shape.
  assert.notEqual(Buffer.from(purchase("O2", item_id, account_id).canonicalBytes()).toString("hex"), fixture.purchase_hex);
});

test("derived names follow labels and take a suffix on collision", () => {
  const plan = new PlanBuilder();
  const left = plan.exact(Item);
  const right = plan.exact(Item);
  plan.returning([plan.field(left, ItemFields.stock), plan.field(right, ItemFields.stock)]);
  const json = JSON.parse(Buffer.from(plan.build(authority()).canonicalBytes()).toString("utf8")) as {
    bindings: { variable: string }[];
  };
  assert.deepEqual(
    json.bindings.map((binding) => binding.variable),
    ["item", "item_2", "item_stock", "item_2_stock"],
  );
});

test("reads answer rows, count or exists, and never write", () => {
  const operations = (["rows", "count", "exists"] as const).map((terminal) => {
    const plan = new PlanBuilder();
    const item = plan.subtypes(Item);
    const stock = plan.field(item, ItemFields.stock);
    const doubled = plan.let("doubled", PlanExpr.max(stock.mul(2n), PlanExpr.abs(-1n)));
    plan.where(doubled.gt(0n));
    if (terminal === "rows") plan.returning([item, doubled]);
    if (terminal === "count") plan.count();
    if (terminal === "exists") plan.exists();
    const built = plan.build(authority());
    assert.equal(built.writes(), false);
    return (JSON.parse(Buffer.from(built.canonicalBytes()).toString("utf8")) as { operation: unknown })
      .operation;
  });
  assert.equal(new Set(operations.map((operation) => JSON.stringify(operation))).size, 3);
});

test("builder calls never throw on content; build reports the first error", () => {
  const duplicate = new PlanBuilder();
  duplicate.input("order_id", "string");
  duplicate.input("order_id", "string");
  assert.equal(buildError(() => duplicate.build(authority())).code, "query_plan_builder_duplicate_name");

  const countedWrite = new PlanBuilder();
  const item = countedWrite.exact(Item);
  countedWrite.update(item, ItemFields.stock, 1n).count();
  assert.equal(buildError(() => countedWrite.build(authority())).code, "query_plan_builder_write_operation");

  const badLiteral = new PlanBuilder();
  const priced = badLiteral.exact(Item);
  badLiteral.where(badLiteral.field(priced, ItemFields.price).gt(decimal("not-a-decimal")));
  badLiteral.input("order_id", "string");
  badLiteral.input("order_id", "string");
  // The literal came first, so it is the reported error.
  assert.notEqual(buildError(() => badLiteral.build(authority())).code, "query_plan_builder_duplicate_name");

  const badIdentity = new PlanBuilder();
  badIdentity.exact({ typeKey: "not json" });
  assert.equal(buildError(() => badIdentity.build(authority())).code, "invalid_model_identity");

  const unowned = new PlanBuilder();
  const account = unowned.exact(Account);
  unowned.update(account, ItemFields.stock as unknown as PlanFieldToken<"account">, 1n);
  buildError(() => unowned.build(authority()));
});

test("handles belong to their builder", () => {
  const first = new PlanBuilder();
  const second = new PlanBuilder();
  const item = first.exact(Item);
  assert.throws(() => second.field(item, ItemFields.stock), TypeError);
  assert.throws(() => second.delete(item), TypeError);
});

test("an inserted thing takes fields and players; a plain value is not a plan", () => {
  const plan = new PlanBuilder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const created = plan.put(Purchase).player(PurchaseTokens.item, item).player(PurchaseTokens.buyer, buyer).thing();
  plan.returning([created]);
  assert.equal(plan.build(authority()).writes(), true);
  assert.throws(() => Plan.prototype.canonicalBytes.call({} as Plan), TypeError);
});

// Compile-time checks only; never called.
function typeChecks(plan: PlanBuilder): void {
  const item = plan.exact(Item);
  const account = plan.exact(Account);
  // @ts-expect-error a field token of another model
  plan.field(item, AccountFields.balance);
  // @ts-expect-error a role of another relation's model
  plan.links(account, PurchaseTokens.buyer, item);
  // @ts-expect-error numbers are not literals
  plan.field(item, ItemFields.stock).gt(1);
}
void typeChecks;
