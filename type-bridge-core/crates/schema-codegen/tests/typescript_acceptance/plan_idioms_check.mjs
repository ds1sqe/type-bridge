import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { Account, Item, ManyItem, Purchase, schemaAuthority } from "./generated_plan_idioms/dist/index.js";
import { Plan, StagedTransaction } from "@type-bridge/node";
import { Named } from "./generated_plan_names/dist/index.js";

const named = Plan.build(() => {
  assert.equal(typeof Named.byIid, "function");
  assert.equal(typeof Named.byIid_.eq, "function");
  const value = Named.match("key").where(Named.set_.eq("old"));
  assert.equal(typeof value.thing_.eq, "function");
  assert.equal(typeof value.recording_.eq, "function");
  assert.equal(typeof value.field_.eq, "function");
  assert.equal(typeof value.open_.eq, "function");
  assert.equal(typeof value.modelType_.eq, "function");
  return value.set({ set_: "new" });
});
assert.ok(named.canonicalBytes().length);

const order = Plan.build(() => {
  const item = Item.match("I1").where(Item.stock.gt(0n));
  const buyer = Account.match("A1").where(Account.balance.ge(item.price));
  item.set({ stock: item.stock.sub(1n) });
  buyer.set({ balance: buyer.balance.sub(item.price) });
  return Purchase.insert({ orderId: "O1", buyer, item });
});
const p = Plan.builder();
const item = p.exact(Item);
const key = p.field(item, Item.itemId);
const itemId = p.input("item_key", "string");
p.where(key.eq(itemId));
const stock = p.field(item, Item.stock);
const minimum = p.input("item_stock_2", "long");
p.where(stock.gt(minimum));
const buyer = p.exact(Account);
const buyerKey = p.field(buyer, Account.id_);
const accountId = p.input("account_key", "string");
p.where(buyerKey.eq(accountId));
const price = p.field(item, Item.price);
const balance = p.field(buyer, Account.balance);
p.where(balance.ge(price));
const decrement = p.input("item_stock_3", "long");
p.update(item, Item.stock, p.let("item_stock_4", stock.sub(decrement)));
p.update(buyer, Account.balance, p.let("account_balance_2", balance.sub(price)));
const created = p.insert(Purchase);
const orderId = p.input("purchase_order_id", "string");
created.field(Purchase.orderId, orderId).player(Purchase.buyer, buyer).player(Purchase.item, item);
p.fetch(created.thing(), [[Purchase.buyer, false], [Purchase.item, false]]);
p.row([itemId.value("I1"), minimum.value(0n), accountId.value("A1"), decrement.value(1n), orderId.value("O1")]);
const explicit = p.build(schemaAuthority());
assert.deepEqual(order.canonicalBytes(), explicit.canonicalBytes());
assert.equal(order.fingerprint(), explicit.fingerprint());
assert.ok(Plan.build(() => ManyItem.match("I4").set({ price: 2n })).canonicalBytes().length);
assert.throws(() => Item.match("I1"), /plan_context_missing/);

const scalars = Plan.build((p) => p.returning(Item.match("I1").stock));
const scalarWire = JSON.parse(new TextDecoder().decode(scalars.canonicalBytes()));
const name = scalarWire.bindings[scalarWire.output.columns[0]].variable;
const answerJson = `{"kind":"rows","rows":[{${JSON.stringify(name)}:{"category":"value","value_type":"integer","value":9007199254740993}}]}`;
const transaction = new StagedTransaction({ stage: () => ({ getProjection: () => null, getJson: () => answerJson }) });
assert.deepEqual(transaction.stage(scalars).result(), [[9007199254740993n]]);

const template = Plan.template();
const keyInput = template.input("key", "string");
const quantity = template.input("quantity", "long");
const adjusted = template.build(() => Item.match(keyInput).set({ stock: quantity }));
const first = adjusted.bind([[keyInput.value("I2"), quantity.value(2n)]]);
const second = adjusted.bind([[keyInput.value("I2"), quantity.value(3n)]]);
assert.notDeepEqual(first.canonicalBytes(), second.canonicalBytes());
assert.deepEqual(first.canonicalBytes(), adjusted.bind([[keyInput.value("I2"), quantity.value(2n)]]).canonicalBytes());
const foreign = Plan.builder().input("key", "string");
assert.throws(() => adjusted.bind([[foreign.value("I2")]]), /does not belong/);

// The native frame owns metadata once, and bulk values bypass the reusable literal registry.
const entry = createRequire(import.meta.url).resolve("@type-bridge/node");
const { loadNative } = await import(new URL("./native.js", pathToFileURL(entry)));
const prototype = loadNative().NodePlanBuilder.prototype;
const modelRows = prototype.modelRows;
const literal = prototype.literal;
let calls = 0;
prototype.literal = function () { throw new Error("bulk entered the literal registry"); };
prototype.modelRows = function (thing, columns, rows) {
  calls += 1;
  assert.equal(columns.length, 3);
  assert.equal(new Set(columns.map(column => column.field)).size, 3);
  assert.equal(rows.length, 10000);
  assert.deepEqual(rows[0], ["I0", 3n, 10n]);
  assert.equal(rows.every(row => row.length === columns.length), true);
  return modelRows.call(this, thing, columns, rows);
};
try {
  const bulk = Plan.build(() => Item.insertMany(Array.from({ length: 10000 }, (_, index) => ({
    itemId: `I${index}`, price: 3n, stock: 10n,
  }))));
  assert.equal(bulk.writes(), true);
  assert.equal(calls, 1);
} finally {
  prototype.modelRows = modelRows;
  prototype.literal = literal;
}
