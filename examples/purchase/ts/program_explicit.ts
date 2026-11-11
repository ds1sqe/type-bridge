/** Core-builder equality oracles for generated authoring. */
import { Plan, type PlanAuthority } from "@type-bridge/node";
import { Account, Doctor, Item, Purchase } from "./index.js";

export function purchaseExplicit(authority: PlanAuthority): Plan {
  const p = Plan.builder();
  const item = p.exact(Item);
  const itemKey = p.field(item, Item.fields.itemId);
  const key = p.input("item_item_id_2", "string");
  p.where(itemKey.eq(key));
  const stock = p.field(item, Item.fields.stock);
  const minimum = p.input("item_stock_2", "long");
  p.where(stock.gt(minimum));
  const buyer = p.exact(Account);
  const buyerKey = p.field(buyer, Account.fields.accountId);
  const account = p.input("account_account_id_2", "string");
  p.where(buyerKey.eq(account));
  const price = p.field(item, Item.fields.price);
  const balance = p.field(buyer, Account.fields.balance);
  p.where(price.le(balance));
  const decrement = p.input("item_stock_3", "long");
  p.update(item, Item.fields.stock, p.let("item_stock_4", stock.sub(decrement)));
  p.update(buyer, Account.fields.balance, p.let("account_balance_2", balance.sub(price)));
  const order = p.insert(Purchase);
  const orderId = p.input("purchase_order_id", "string");
  order.field(Purchase.fields.orderId, orderId).player(Purchase.roles.buyer, buyer).player(Purchase.roles.item, item);
  p.row([key.value("I1"), minimum.value(0n), account.value("A1"), decrement.value(1n), orderId.value("O1")]);
  return p.build(authority);
}

export function touchExplicit(authority: PlanAuthority): Plan {
  const p = Plan.builder();
  const doctor = p.exact(Doctor);
  p.update(doctor, Doctor.fields.onCall, p.field(doctor, Doctor.fields.onCall));
  return p.build(authority);
}

export function doctorExplicit(authority: PlanAuthority, state: boolean): Plan {
  const p = Plan.builder();
  const doctor = p.insert(Doctor);
  const name = p.input("doctor_name", "string");
  const onCall = p.input("doctor_on_call", "boolean");
  doctor.field(Doctor.fields.name_, name).field(Doctor.fields.onCall, onCall);
  p.row([name.value("D1"), onCall.value(state)]);
  return p.build(authority);
}
