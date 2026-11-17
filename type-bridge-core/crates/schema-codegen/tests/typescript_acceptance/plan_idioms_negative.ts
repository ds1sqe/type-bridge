import { Account, Item, Many, ManyItem, Purchase } from "./generated_plan_idioms/src/index.js";
import { Plan } from "@type-bridge/node";

Plan.build(() => {
  const item = Item.match("I1");
  const account = Account.match("A1");
  // @ts-expect-error scalar domain
  item.set({ stock: "wrong" });
  // @ts-expect-error a string value cannot compare with an integer class token
  account.where(item.itemId.le(Account.balance));
  // @ts-expect-error an integer value cannot compare with a string class token
  account.where(item.price.le(Account.id));
  // @ts-expect-error arithmetic domain
  item.stock.sub("wrong");
  // @ts-expect-error strings have no arithmetic operators
  item.itemId.add("suffix");
  // @ts-expect-error keys are immutable
  item.set({ itemId: "new" });
  // @ts-expect-error required price
  Item.insert({ itemId: "I2" });
  // @ts-expect-error required item player
  Purchase.insert({ orderId: "O1", buyer: account });
  // @ts-expect-error wrong nominal buyer
  Purchase.insert({ orderId: "O1", buyer: item, item });
  // @ts-expect-error required bulk price
  Item.insertMany([{ itemId: "I2" }]);
  // @ts-expect-error multi-valued ownership omits factories
  Many.insert({});
  // @ts-expect-error multi-valued ownership omits bulk factories
  Many.insertMany([]);
  // @ts-expect-error inherited factories stay omitted
  ManyItem.insert({ itemId: "I3", price: 3n });
  // @ts-expect-error set excludes multi-valued fields
  ManyItem.match("I3").set({ tags: "tag" });
  return item.delete();
});

const template = Plan.template();
const key = template.input("key", "string");
const quantity = template.input("quantity", "long");
// @ts-expect-error typed input column
key.value(4n);
// @ts-expect-error typed input column
quantity.value("wrong");
template.build(() => {
  // @ts-expect-error input column has the wrong key domain
  Item.match(quantity);
  const item = Item.match(key);
  // @ts-expect-error input column has the wrong assignment domain
  item.set({ stock: key });
  return item.delete();
});
