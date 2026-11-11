//! Core-builder equality oracles for generated authoring.
use super::*;

pub fn purchase() -> Result<Built> {
    let mut p = Plan::builder();
    let item = p.exact::<Item>();
    let item_key = p.field(item, ItemType::item_id);
    let key = p.input::<String>("item_item_id_2");
    p.where_(item_key.eq(key));
    let stock = p.field(item, ItemType::stock);
    let minimum = p.input::<i64>("item_stock_2");
    p.where_(stock.gt(minimum));
    let buyer = p.exact::<Account>();
    let buyer_key = p.field(buyer, AccountType::account_id);
    let account = p.input::<String>("account_account_id_2");
    p.where_(buyer_key.eq(account));
    let price = p.field(item, ItemType::price);
    let balance = p.field(buyer, AccountType::balance);
    p.where_(price.le(balance));
    let decrement = p.input::<i64>("item_stock_3");
    let next_stock = p.let_("item_stock_4", stock - decrement);
    p.update(item, ItemType::stock, next_stock);
    let next_balance = p.let_("account_balance_2", balance - price);
    p.update(buyer, AccountType::balance, next_balance);
    let order_id = p.input::<String>("purchase_order_id");
    p.insert::<Purchase>()
        .field(PurchaseType::order_id, order_id)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    p.row([
        key.value("I1".into()),
        minimum.value(0),
        account.value("A1".into()),
        decrement.value(1),
        order_id.value("O1".into()),
    ]);
    p.build(&models::SCHEMA)
}

pub fn touch() -> Result<Built> {
    let mut p = Plan::builder();
    let doctor = p.exact::<Doctor>();
    let on_call = p.field(doctor, DoctorType::on_call);
    p.update(doctor, DoctorType::on_call, on_call);
    p.build(&models::SCHEMA)
}

pub fn doctor(state: bool) -> Result<Built> {
    let mut p = Plan::builder();
    let name = p.input::<String>("doctor_name");
    let on_call = p.input::<bool>("doctor_on_call");
    p.insert::<Doctor>()
        .field(DoctorType::name, name)
        .field(DoctorType::on_call, on_call);
    p.row([name.value("D1".into()), on_call.value(state)]);
    p.build(&models::SCHEMA)
}
