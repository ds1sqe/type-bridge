"""Core-builder equality oracles for the generated authoring examples."""

from typing import Any

from type_bridge.plan import Plan


def purchase(models: Any, authority: Any) -> Plan:
    p = Plan.builder()
    item = p.exact(models.Item)
    item_key = p.field(item, models.Item.item_id)
    key = p.input("item_item_id_2", "string")
    p.where_(item_key.eq(key))
    stock = p.field(item, models.Item.stock)
    minimum = p.input("item_stock_2", "long")
    p.where_(stock.gt(minimum))
    buyer = p.exact(models.Account)
    buyer_key = p.field(buyer, models.Account.account_id)
    account = p.input("account_account_id_2", "string")
    p.where_(buyer_key.eq(account))
    balance = p.field(buyer, models.Account.balance)
    price = p.field(item, models.Item.price)
    p.where_(balance.ge(price))
    decrement = p.input("item_stock_3", "long")
    p.update(item, models.Item.stock, p.let_("item_stock_4", stock - decrement))
    p.update(buyer, models.Account.balance, p.let_("account_balance_2", balance - price))
    order = p.insert(models.Purchase)
    order_id = p.input("purchase_order_id", "string")
    order.field(models.Purchase.order_id, order_id).player(models.Purchase.buyer, buyer).player(
        models.Purchase.item, item
    )
    p.row(
        [
            key.value("I1"),
            minimum.value(0),
            account.value("A1"),
            decrement.value(1),
            order_id.value("O1"),
        ]
    )
    return p.build(authority)


def touch(models: Any, authority: Any) -> Plan:
    p = Plan.builder()
    doctor = p.exact(models.Doctor)
    p.update(doctor, models.Doctor.on_call, p.field(doctor, models.Doctor.on_call))
    return p.build(authority)


def doctor(models: Any, authority: Any, on_call: bool) -> Plan:
    p = Plan.builder()
    inserted = p.insert(models.Doctor)
    name = p.input("doctor_name", "string")
    state = p.input("doctor_on_call", "boolean")
    inserted.field(models.Doctor.name, name).field(models.Doctor.on_call, state)
    p.row([name.value("D1"), state.value(on_call)])
    return p.build(authority)
