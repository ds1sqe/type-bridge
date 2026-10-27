from typing import assert_type

from type_bridge.plan import Expr, Matched, Pending, Plan, Transaction, Value
from generated_idioms import Account, AccountHandle, Item, ItemHandle, Purchase


def purchase(transaction: Transaction, item_id: str, account_id: str, order_id: str) -> None:
    with Plan() as p:
        assert_type(p, Plan[Matched])
        item = Item.match(item_id).where(Item.stock > 0)
        buyer = Account.match(account_id).where(Account.balance >= item.price)
        assert_type(item, ItemHandle)
        assert_type(buyer, AccountHandle)
        assert_type(item.stock, Value[int])
        assert_type(item.stock - 1, Expr[int])
        item.set(stock=item.stock - 1)
        buyer.set(balance=buyer.balance - item.price)
        order = Purchase.insert(order_id=order_id, buyer=buyer, item=item)
    assert_type(order, Plan[list[Purchase]])
    bought = transaction.stage(order)
    assert_type(bought, Pending[list[Purchase]])
    assert_type(bought.result(), list[Purchase])
    with Plan():
        removed = Item.subtypes(item_id=item_id).delete_returning()
    assert_type(removed, Plan[list[Item]])
    with Plan() as p:
        Item.by_iid("0x1234567890abcdef")
        counted = p.count()
    assert_type(counted, Plan[int])
    with Plan():
        bulk = Item.insert_many([Item.Values(item_id="I2", price=3)])
    assert_type(bulk, Plan[list[Item]])
    assert_type(transaction.stage(bulk).result(), list[Item])
    with Plan.template() as template:
        key = template.input("key", "string")
        quantity = template.input("quantity", "long")
        adjusted = Item.match(key).set(stock=quantity)
    assert_type(adjusted.bind([[key.value("I2"), quantity.value(4)]]), Plan[list[Item]])
    with Plan() as p:
        one = Item.match("I2")
        scalars = p.returning(one.stock, one.item_id)
    assert_type(scalars, Plan[list[tuple[int, str]]])


def mixed_rows(transaction: Transaction) -> None:
    with Plan() as p:
        item = Item.match("I1")
        selected = p.returning(item, item.stock)
        assert_type(selected, Plan[list[tuple[Item, int]]])
    assert_type(transaction.stage(selected).result(), list[tuple[Item, int]])
