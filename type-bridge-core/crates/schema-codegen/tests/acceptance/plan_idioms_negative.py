from generated_idioms import Account, Item, Many, ManyItem, Purchase

from type_bridge.plan import Plan

with Plan():
    item = Item.match("I1")
    buyer = Account.match("A1")
    item.set(stock="wrong")  # error
    item.where(Item.stock > "wrong")  # error
    Purchase.insert(order_id="O1", buyer=buyer)  # error
    Purchase.insert(order_id="O1", buyer=item, item=buyer)  # error
    Item.insert(item_id="I2")  # error
    Item.match(id="not-a-field")  # error
    Account.match(123)  # error
    Many.insert(tags=["a"])  # error
    ManyItem.insert(item_id="I3", price=1)  # error
    Many.insert_many([])  # error
    ManyItem.match("I3").set(tags=["a"])  # error
    Item.insert_many([Item.Values(item_id="missing-price")])  # error
    item.set(item_id="immutable-key")  # error
