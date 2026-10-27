"""Generated purchase authoring must equal the explicit shared-builder form."""
from generated_idioms import Account, Item, ManyItem, Purchase, schema_authority
from type_bridge.plan import Plan

with Plan():
    item = Item.match("I1").where(Item.stock > 0)
    buyer = Account.match("A1").where(Account.balance >= item.price)
    item.set(stock=item.stock - 1)
    buyer.set(balance=buyer.balance - item.price)
    order = Purchase.insert(order_id="O1", buyer=buyer, item=item)

p = Plan.builder()
i = p.exact(Item)
ik = p.field(i, Item.item_id)
iv = p.input("item_key", "string")
p.where_(ik.eq(iv))
stock = p.field(i, Item.stock)
minimum = p.input("item_stock_2", "long")
p.where_(stock.gt(minimum))
a = p.exact(Account)
ak = p.field(a, Account.id)
av = p.input("account_key", "string")
p.where_(ak.eq(av))
price = p.field(i, Item.price)
balance = p.field(a, Account.balance)
p.where_(balance.ge(price))
decrement = p.input("item_stock_3", "long")
next_stock = p.let_("item_stock_4", stock - decrement)
p.update(i, Item.stock, next_stock)
next_balance = p.let_("account_balance_2", balance - price)
p.update(a, Account.balance, next_balance)
created = p.insert(Purchase)
ov = p.input("purchase_order_id", "string")
created.field(Purchase.order_id, ov).player(Purchase.buyer, a).player(Purchase.item, i)
p.fetch(created.thing(), [(Purchase.buyer, False), (Purchase.item, False)])
p.row([iv.value("I1"), minimum.value(0), av.value("A1"), decrement.value(1), ov.value("O1")])
explicit = p.build(schema_authority())
assert order.canonical_bytes() == explicit.canonical_bytes()
assert order.fingerprint() == explicit.fingerprint()
print("generated purchase bytes equal the core-builder form")

with Plan():
    changed = ManyItem.match("I4").set(price=2)
assert changed.canonical_bytes()

with Plan() as p:
    selected_item = Item.match("I1")
    selected_rows = p.returning(selected_item, selected_item.stock)
assert not selected_rows.writes()
