import { Account, AccountHandle, Item, ItemHandle, Purchase } from "./generated_plan_idioms/src/index.js";
import { Plan, type Expr, type Pending, type StagedTransaction, type Value } from "@type-bridge/node";
import { Named } from "./generated_plan_names/src/index.js";

function expectType<T>(_value: T): void {}

export function names(): void {
  expectType<Plan<Named[]>>(Plan.build(() => {
    expectType<Plan<bigint>>(Plan.build((p) => { Named.byIid("0x01"); return p.count(); }));
    const value = Named.match("key").where(Named.set_.eq("old"));
    expectType<Value<string>>(value.thing_);
    expectType<Value<string>>(value.recording_);
    expectType<Value<string>>(value.field_);
    expectType<Value<string>>(value.open_);
    expectType<Value<string>>(value.modelType_);
    return value.set({ set_: "new" });
  }));
}

export function purchase(transaction: StagedTransaction): void {
  const order = Plan.build(() => {
    const item = Item.match("I1").where(Item.stock.gt(0n));
    const buyer = Account.match("A1").where(Account.balance.ge(item.price));
    expectType<ItemHandle>(item);
    expectType<AccountHandle>(buyer);
    expectType<Value<bigint>>(item.stock);
    expectType<Expr<bigint>>(item.stock.sub(1n));
    item.set({ stock: item.stock.sub(1n) });
    buyer.set({ balance: buyer.balance.sub(item.price) });
    return Purchase.insert({ orderId: "O1", buyer, item });
  });
  expectType<Plan<Purchase[]>>(order);
  expectType<Pending<Purchase[]>>(transaction.stage(order));
  expectType<Purchase[]>(transaction.stage(order).result());
  expectType<Plan<Item[]>>(Plan.build(() => Item.insertMany([{ itemId: "I2", price: 3n }])));
  expectType<Plan<Item[]>>(Plan.build(() => Item.subtypes("I2").deleteReturning()));
  expectType<Plan<bigint>>(Plan.build((p) => { Item.byIid("0x01"); return p.count(); }));
  const rows: Item.Values[] = [{ itemId: "I2", price: 3n }];
  expectType<Plan<Item[]>>(Plan.build(() => Item.putMany(rows)));
  const template = Plan.template();
  const key = template.input("key", "string");
  const quantity = template.input("quantity", "long");
  const adjusted = template.build(() => Item.match(key).set({ stock: quantity }));
  expectType<Plan<Item[]>>(adjusted.bind([[key.value("I2"), quantity.value(4n)]]));
  expectType<Plan<[bigint, string][]>>(Plan.build((p) => {
    const item = Item.match("I2");
    return p.returning(item.stock, item.itemId);
  }));
}

export function mixedRows(transaction: StagedTransaction): void {
  const rows = Plan.build((p) => {
    const item = Item.match("I1");
    return p.returning(item, item.stock);
  });
  expectType<Plan<[Item, bigint][]>>(rows);
  expectType<[Item, bigint][]>(transaction.stage(rows).result());
}
