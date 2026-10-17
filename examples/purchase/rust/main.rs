//! The purchase program of the `writes` SDK conformance suite, in Rust.
//!
//! It builds every plan from the generated crate's tokens and `SCHEMA`, then,
//! when a live target is configured, runs the same program directly and over
//! the proxy: seed, stage the purchase, a touch and a batch of 50 orders in
//! one transaction, commit, and read the state back; then the journeys (a
//! put, an update answering rows, an insert whose players it inserts, a
//! delete) in one transaction, and a second read. It prints one JSON
//! report. `models` is the crate generated from
//! tests/contracts/sdk_conformance/writes/purchase.

use models::{
    Account, AccountType, AppSchema, Doctor, DoctorType, Item, ItemType, Purchase, PurchaseType,
};
use serde_json::{Map, Value as Json, json};
use type_bridge::plan::{Plan, PlanBuilder};
use type_bridge::staged::{PlanAnswer, Transaction, TransactionKind, TransactionOptions};
use type_bridge::{
    ConnectionOptions, Database, ProxyConnectionPolicy, ProxyDatabase, ProxyEndpoint, Result,
};

type Built = Plan<AppSchema>;

/// One item in stock, one funded account and one on-call doctor.
fn seed() -> Result<Built> {
    let mut plan = Plan::builder();
    let item_id = plan.input::<String>("item_id");
    let stock = plan.input::<i64>("stock");
    let price = plan.input::<i64>("price");
    let account_id = plan.input::<String>("account_id");
    let balance = plan.input::<i64>("balance");
    let doctor = plan.input::<String>("doctor");
    let on_call = plan.input::<bool>("on_call");
    plan.insert::<Item>()
        .field(ItemType::item_id, item_id)
        .field(ItemType::stock, stock)
        .field(ItemType::price, price);
    plan.insert::<Account>()
        .field(AccountType::account_id, account_id)
        .field(AccountType::balance, balance);
    plan.insert::<Doctor>()
        .field(DoctorType::name, doctor)
        .field(DoctorType::on_call, on_call);
    plan.row([
        item_id.value("I1".to_owned()),
        stock.value(1),
        price.value(40),
        account_id.value("A1".to_owned()),
        balance.value(100),
        doctor.value("D1".to_owned()),
        on_call.value(true),
    ]);
    plan.build(&models::SCHEMA)
}

// --8<-- [start:purchase]
/// The one-query purchase: take one item and its price, record the order.
fn purchase() -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let buyer = plan.exact::<Account>();
    let order_input = plan.input::<String>("order_id");
    let item_input = plan.input::<String>("item_id");
    let account_input = plan.input::<String>("account_id");
    let item_key = plan.field(item, ItemType::item_id);
    let buyer_key = plan.field(buyer, AccountType::account_id);
    let stock = plan.field(item, ItemType::stock);
    let price = plan.field(item, ItemType::price);
    let balance = plan.field(buyer, AccountType::balance);
    plan.where_(item_key.eq(item_input))
        .where_(buyer_key.eq(account_input))
        .where_(stock.gt(0_i64))
        .where_(balance.ge(price));
    let next_stock = plan.let_("next_stock", stock - 1_i64);
    let next_balance = plan.let_("next_balance", balance - price);
    plan.update(item, ItemType::stock, next_stock).update(
        buyer,
        AccountType::balance,
        next_balance,
    );
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, order_input)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    plan.row([
        order_input.value("O1".to_owned()),
        item_input.value("I1".to_owned()),
        account_input.value("A1".to_owned()),
    ]);
    plan.build(&models::SCHEMA)
}
// --8<-- [end:purchase]

/// Rewrite every doctor's on-call value with itself, taking its lock.
fn touch() -> Result<Built> {
    let mut plan = Plan::builder();
    let doctor = plan.exact::<Doctor>();
    let on_call = plan.field(doctor, DoctorType::on_call);
    plan.update(doctor, DoctorType::on_call, on_call);
    plan.build(&models::SCHEMA)
}

/// Fifty orders of the item by the account, one given row each.
fn batch() -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let buyer = plan.exact::<Account>();
    let order_input = plan.input::<String>("order_id");
    let item_key = plan.field(item, ItemType::item_id);
    plan.where_(item_key.eq("I1".to_owned()));
    let buyer_key = plan.field(buyer, AccountType::account_id);
    plan.where_(buyer_key.eq("A1".to_owned()));
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, order_input)
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    for number in 1..=50 {
        plan.row([order_input.value(format!("B{number:02}"))]);
    }
    plan.build(&models::SCHEMA)
}

fn sold_out() -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let key = plan.field(item, ItemType::item_id);
    plan.where_(key.eq("I1".to_owned()));
    let stock = plan.field(item, ItemType::stock);
    plan.where_(stock.eq(0_i64));
    plan.count();
    plan.build(&models::SCHEMA)
}

fn charged() -> Result<Built> {
    let mut plan = Plan::builder();
    let account = plan.exact::<Account>();
    let key = plan.field(account, AccountType::account_id);
    plan.where_(key.eq("A1".to_owned()));
    let balance = plan.field(account, AccountType::balance);
    plan.where_(balance.eq(60_i64));
    plan.count();
    plan.build(&models::SCHEMA)
}

fn orders() -> Result<Built> {
    let mut plan: PlanBuilder<AppSchema> = Plan::builder();
    plan.exact::<Purchase>();
    plan.count();
    plan.build(&models::SCHEMA)
}

fn on_call() -> Result<Built> {
    let mut plan = Plan::builder();
    let doctor = plan.exact::<Doctor>();
    let on_call = plan.field(doctor, DoctorType::on_call);
    plan.where_(on_call.eq(true));
    plan.exists();
    plan.build(&models::SCHEMA)
}

/// Put a second doctor, off call: created, since none matches.
fn put_doctor() -> Result<Built> {
    let mut plan = Plan::builder();
    plan.put::<Doctor>()
        .field(DoctorType::name, "D2".to_owned())
        .field(DoctorType::on_call, false);
    plan.build(&models::SCHEMA)
}

/// Put the second doctor on call and answer its name (`Write{returns}`).
fn returns() -> Result<Built> {
    let mut plan = Plan::builder();
    let doctor = plan.exact::<Doctor>();
    let name = plan.field(doctor, DoctorType::name);
    plan.where_(name.eq("D2".to_owned()));
    plan.update(doctor, DoctorType::on_call, true);
    plan.returning([name]);
    plan.build(&models::SCHEMA)
}

/// A purchase whose players are the item and account this write inserts.
fn refer() -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan
        .insert::<Item>()
        .field(ItemType::item_id, "I2".to_owned())
        .field(ItemType::stock, 3_i64)
        .field(ItemType::price, 10_i64)
        .thing();
    let buyer = plan
        .insert::<Account>()
        .field(AccountType::account_id, "A2".to_owned())
        .field(AccountType::balance, 50_i64)
        .thing();
    plan.insert::<Purchase>()
        .field(PurchaseType::order_id, "R1".to_owned())
        .player(PurchaseType::buyer, buyer)
        .player(PurchaseType::item, item);
    plan.build(&models::SCHEMA)
}

/// Delete the first purchase.
fn remove() -> Result<Built> {
    let mut plan = Plan::builder();
    let order = plan.exact::<Purchase>();
    let key = plan.field(order, PurchaseType::order_id);
    plan.where_(key.eq("O1".to_owned()));
    plan.delete(order);
    plan.build(&models::SCHEMA)
}

fn doctors_on_call() -> Result<Built> {
    let mut plan = Plan::builder();
    let doctor = plan.exact::<Doctor>();
    let on_call = plan.field(doctor, DoctorType::on_call);
    plan.where_(on_call.eq(true));
    plan.count();
    plan.build(&models::SCHEMA)
}

fn referred() -> Result<Built> {
    let mut plan = Plan::builder();
    let order = plan.exact::<Purchase>();
    let buyer = plan.exact::<Account>();
    let key = plan.field(order, PurchaseType::order_id);
    plan.where_(key.eq("R1".to_owned()));
    let buyer_key = plan.field(buyer, AccountType::account_id);
    plan.where_(buyer_key.eq("A2".to_owned()));
    plan.links(order, PurchaseType::buyer, buyer);
    plan.count();
    plan.build(&models::SCHEMA)
}

fn removed() -> Result<Built> {
    let mut plan = Plan::builder();
    let order = plan.exact::<Purchase>();
    let key = plan.field(order, PurchaseType::order_id);
    plan.where_(key.eq("O1".to_owned()));
    plan.exists();
    plan.build(&models::SCHEMA)
}

/// A third doctor, on call: staged, then rolled back.
fn ghost() -> Result<Built> {
    let mut plan = Plan::builder();
    plan.insert::<Doctor>()
        .field(DoctorType::name, "D3".to_owned())
        .field(DoctorType::on_call, true);
    plan.build(&models::SCHEMA)
}

/// A fourth doctor, off call: staged after the rollback and committed.
fn kept() -> Result<Built> {
    let mut plan = Plan::builder();
    plan.insert::<Doctor>()
        .field(DoctorType::name, "D4".to_owned())
        .field(DoctorType::on_call, false);
    plan.build(&models::SCHEMA)
}

/// The doctor whose name the constraints suite inserts twice.
fn seed_doctor() -> Result<Built> {
    let mut plan = Plan::builder();
    plan.insert::<Doctor>()
        .field(DoctorType::name, "D1".to_owned())
        .field(DoctorType::on_call, true);
    plan.build(&models::SCHEMA)
}

/// A second doctor named D1: the key rejects the commit.
fn duplicate_doctor() -> Result<Built> {
    let mut plan = Plan::builder();
    plan.insert::<Doctor>()
        .field(DoctorType::name, "D1".to_owned())
        .field(DoctorType::on_call, false);
    plan.build(&models::SCHEMA)
}

type Steps = &'static [(&'static str, &'static [&'static str])];

const PURCHASE_STEPS: Steps = &[
    ("seed", &["seed"]),
    ("program", &["purchase", "touch", "batch"]),
    ("read", &["sold_out", "charged", "orders", "on_call"]),
    ("journeys", &["put_doctor", "returns", "refer", "remove"]),
    ("after", &["doctors_on_call", "referred", "removed"]),
    ("rollback", &["kept"]),
];
const CONSTRAINTS_STEPS: Steps = &[
    ("seed", &["seed_doctor"]),
    ("reject", &["duplicate_doctor"]),
];

/// Plans a step stages and rolls back before staging its own.
fn rolled_back(step: &str) -> &'static [&'static str] {
    match step {
        "rollback" => &["ghost"],
        _ => &[],
    }
}

/// One answer as [kind, value]; rows report their count, exists 0 or 1.
fn answer(value: &PlanAnswer) -> Json {
    match value {
        PlanAnswer::Matched(count) => json!(["matched", count]),
        PlanAnswer::Count(count) => json!(["count", count]),
        PlanAnswer::Exists(exists) => json!(["exists", u8::from(*exists)]),
        PlanAnswer::Rows(rows) => json!(["rows", rows.len()]),
        other => panic!("unknown answer {other:?}"),
    }
}

fn kind(step: &str) -> TransactionOptions {
    TransactionOptions::default().with_kind(if matches!(step, "read" | "after") {
        TransactionKind::Read
    } else {
        TransactionKind::Write
    })
}

/// Stage `names` in one transaction, commit, and read their answers.
/// `discarded` plans are staged first and rolled back; the transaction stays
/// usable for `names`.
async fn run(
    mut tx: Transaction<AppSchema>,
    plans: &[(&str, Built)],
    names: &[&str],
    discarded: &[&str],
) -> Result<Json> {
    let built = |name: &str| {
        plans
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, plan)| plan)
            .expect("every step names a built plan")
    };
    for name in discarded {
        tx.stage(built(name))?;
    }
    if !discarded.is_empty() {
        tx.rollback()?;
    }
    let mut pending = Vec::new();
    for name in names {
        pending.push((*name, tx.stage(built(name))?));
    }
    if let Err(error) = tx.commit().await {
        // A rejected commit answers every plan with its typed failure.
        let Some(failure) = error.failure.as_deref() else {
            return Err(error.into());
        };
        let host = failure.host_json();
        let failed = json!(["failure", {
            "phase": host["phase"],
            "reason": host["reason"],
            "code": host["code"],
            "retry_safe": host["retry_safe"],
        }]);
        return Ok(Json::Object(
            names
                .iter()
                .map(|name| ((*name).to_owned(), failed.clone()))
                .collect(),
        ));
    }
    let mut answers = Map::new();
    for (name, item) in pending {
        answers.insert(name.to_owned(), answer(&*item.get()?));
    }
    Ok(Json::Object(answers))
}

#[tokio::main]
async fn main() -> Result<()> {
    let suite = std::env::var("TYPE_BRIDGE_SDK_WRITES_SUITE").unwrap_or_else(|_| "purchase".into());
    let (suite_steps, plans) = match suite.as_str() {
        "purchase" => (
            PURCHASE_STEPS,
            vec![
                ("seed", seed()?),
                ("purchase", purchase()?),
                ("touch", touch()?),
                ("batch", batch()?),
                ("sold_out", sold_out()?),
                ("charged", charged()?),
                ("orders", orders()?),
                ("on_call", on_call()?),
                ("put_doctor", put_doctor()?),
                ("returns", returns()?),
                ("refer", refer()?),
                ("remove", remove()?),
                ("doctors_on_call", doctors_on_call()?),
                ("referred", referred()?),
                ("removed", removed()?),
                ("ghost", ghost()?),
                ("kept", kept()?),
            ],
        ),
        "constraints" => (
            CONSTRAINTS_STEPS,
            vec![
                ("seed_doctor", seed_doctor()?),
                ("duplicate_doctor", duplicate_doctor()?),
            ],
        ),
        other => panic!("unknown writes suite {other}"),
    };
    let mut encoded = Map::new();
    for (name, plan) in &plans {
        let bytes = plan.canonical_bytes()?;
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        encoded.insert((*name).to_owned(), Json::String(hex));
    }
    let mut results = Map::new();
    if let Some(database) = std::env::var("TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE")
        .ok()
        .filter(|name| !name.is_empty())
    {
        let env =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
        let options = ConnectionOptions::new(env("TYPEDB_ADDRESS", "localhost:1729"), database)
            .credentials(
                env("TYPEDB_USERNAME", "admin"),
                env("TYPEDB_PASSWORD", "password"),
            )
            .http_port(
                env("TYPEDB_HTTP_PORT", "8000")
                    .parse()
                    .expect("TYPEDB_HTTP_PORT"),
            );
        let db = Database::connect(options)
            .await?
            .with_schema(models::SCHEMA)?;
        let mut steps = Map::new();
        for &(step, names) in suite_steps {
            let tx = db.transaction(kind(step));
            steps.insert(
                step.to_owned(),
                run(tx, &plans, names, rolled_back(step)).await?,
            );
        }
        results.insert("direct".to_owned(), Json::Object(steps));
    }
    if let Some(endpoint) = std::env::var("TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT")
        .ok()
        .filter(|url| !url.is_empty())
    {
        let proxy =
            ProxyDatabase::connect(ProxyConnectionPolicy::new(ProxyEndpoint::new(endpoint)))
                .await?
                .with_schema(models::SCHEMA)?;
        let mut steps = Map::new();
        for &(step, names) in suite_steps {
            let tx = proxy.transaction(kind(step)).await?;
            steps.insert(
                step.to_owned(),
                run(tx, &plans, names, rolled_back(step)).await?,
            );
        }
        results.insert("proxy".to_owned(), Json::Object(steps));
    }
    let report = json!({"binding": "rust", "plans": encoded, "results": results});
    println!("{report}");
    Ok(())
}
