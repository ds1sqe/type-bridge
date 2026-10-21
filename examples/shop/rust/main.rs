//! The shop checkout of the `writes` SDK conformance suite, in Rust.
//!
//! A checkout takes a whole cart at once: every line must be in stock and
//! the account must cover the total, and then every stock, the balance and
//! the order lines change together. So a cart is one plan, and one plan is
//! one TypeQL query. If any check fails the match is empty, nothing is
//! written, and the plan answers `Matched(0)`: check the answer. Several
//! plans staged in one transaction commit together, and each sees the writes
//! staged before it.
//!
//! The race step has several buyers commit one cart for the last lamp at
//! once; the losers get a retry-safe conflict and run their whole
//! transaction again, where the lamp is gone.
//!
//! When a live target is configured, it runs the steps of
//! tests/contracts/sdk_conformance/writes/shop/program.json directly and over
//! the proxy, and prints one JSON report. `models` is the crate generated
//! from tests/contracts/sdk_conformance/writes/shop.

use std::sync::Arc;

use models::{Account, AccountType, AppSchema, Item, ItemType, OrderLine, OrderLineType};
use serde_json::{Map, Value as Json, json};
use tokio::sync::Barrier;
use tokio::task::JoinSet;
use type_bridge::failure::{Failure, Reason};
use type_bridge::plan::{Expr, Plan, PlanBuilder};
use type_bridge::staged::{Pending, PlanAnswer, Transaction, TransactionKind, TransactionOptions};
use type_bridge::{
    ConnectionOptions, Database, ProxyConnectionPolicy, ProxyDatabase, ProxyEndpoint, Result,
};

type Built = Plan<AppSchema>;

/// Three accounts, one given row each.
fn seed_accounts() -> Result<Built> {
    let mut plan = Plan::builder();
    let account_id = plan.input::<String>("account_id");
    let balance = plan.input::<i64>("balance");
    plan.insert::<Account>()
        .field(AccountType::account_id, account_id)
        .field(AccountType::balance, balance);
    for (key, value) in [("A1", 100), ("A2", 30), ("R1", 100)] {
        plan.row([account_id.value(key.to_owned()), balance.value(value)]);
    }
    plan.build(&models::SCHEMA)
}

/// Four items with a price and a stock, one given row each.
fn seed_items() -> Result<Built> {
    let mut plan = Plan::builder();
    let sku = plan.input::<String>("sku");
    let price = plan.input::<i64>("price");
    let stock = plan.input::<i64>("stock");
    plan.insert::<Item>()
        .field(ItemType::sku, sku)
        .field(ItemType::price, price)
        .field(ItemType::stock, stock);
    for (key, cost, count) in [
        ("pen", 3, 10),
        ("book", 40, 1),
        ("mug", 12, 5),
        ("lamp", 10, 1),
    ] {
        plan.row([
            sku.value(key.to_owned()),
            price.value(cost),
            stock.value(count),
        ]);
    }
    plan.build(&models::SCHEMA)
}

// --8<-- [start:checkout]
/// The whole cart in one query: check every line and the total, then write.
fn checkout(account_id: &str, order_id: &str, cart: &[(&str, i64)]) -> Result<Built> {
    let mut plan = Plan::builder();
    let buyer = plan.exact::<Account>();
    let balance = plan.field(buyer, AccountType::balance);
    let buyer_key = plan.field(buyer, AccountType::account_id);
    plan.where_(buyer_key.eq(account_id.to_owned()));

    let mut lines = Vec::new();
    let mut total: Option<Expr> = None;
    for &(sku, quantity) in cart {
        let item = plan.exact::<Item>();
        let stock = plan.field(item, ItemType::stock);
        let item_key = plan.field(item, ItemType::sku);
        plan.where_(item_key.eq(sku.to_owned()))
            .where_(stock.ge(quantity));
        let cost = plan.field(item, ItemType::price) * quantity;
        total = Some(match total {
            None => cost,
            Some(total) => total + cost,
        });
        lines.push((item, stock, quantity));
    }
    let total = plan.let_("total", total.expect("a cart has at least one line"));
    plan.where_(balance.ge(total));

    for (number, &(item, stock, quantity)) in (1..).zip(&lines) {
        let next_stock = plan.let_(&format!("stock_{number}"), stock - quantity);
        plan.update(item, ItemType::stock, next_stock);
    }
    let balance_after = plan.let_("balance_after", balance - total);
    plan.update(buyer, AccountType::balance, balance_after);
    for &(item, _, quantity) in &lines {
        plan.insert::<OrderLine>()
            .field(OrderLineType::order_id, order_id.to_owned())
            .field(OrderLineType::quantity, quantity)
            .player(OrderLineType::buyer, buyer)
            .player(OrderLineType::item, item);
    }
    plan.build(&models::SCHEMA)
}
// --8<-- [end:checkout]

/// Count the accounts whose id and balance hold the given values.
fn balance_is(account_id: &str, value: i64) -> Result<Built> {
    let mut plan = Plan::builder();
    let account = plan.exact::<Account>();
    let key = plan.field(account, AccountType::account_id);
    plan.where_(key.eq(account_id.to_owned()));
    let balance = plan.field(account, AccountType::balance);
    plan.where_(balance.eq(value));
    plan.count();
    plan.build(&models::SCHEMA)
}

/// Count the items whose sku and stock hold the given values.
fn stock_is(sku: &str, value: i64) -> Result<Built> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let key = plan.field(item, ItemType::sku);
    plan.where_(key.eq(sku.to_owned()));
    let stock = plan.field(item, ItemType::stock);
    plan.where_(stock.eq(value));
    plan.count();
    plan.build(&models::SCHEMA)
}

fn lines() -> Result<Built> {
    let mut plan: PlanBuilder<AppSchema> = Plan::builder();
    plan.exact::<OrderLine>();
    plan.count();
    plan.build(&models::SCHEMA)
}

type Steps = &'static [(&'static str, &'static [&'static str])];

const STEPS: Steps = &[
    ("seed", &["seed_accounts", "seed_items"]),
    ("checkout", &["checkout_a1"]),
    ("checked_out", &["a1_paid", "book_sold_out", "pens_left", "lines"]),
    ("sold_out", &["checkout_a2"]),
    ("short", &["checkout_short"]),
    ("unchanged", &["mugs_kept", "a2_kept", "lines"]),
    ("two_carts", &["checkout_a2_mug", "checkout_a2_pens"]),
    ("final", &["a2_spent", "pens_final", "mugs_final", "lines"]),
    ("race", &["race_lamp"]),
    ("raced", &["lamp_sold_out", "r1_paid_once", "lines"]),
];
/// The race step: this many buyers commit its one plan at once.
const BUYERS: usize = 8;
const ATTEMPTS: usize = 20;

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

fn options(kind: TransactionKind) -> TransactionOptions {
    TransactionOptions::default().with_kind(kind)
}

fn kind(step: &str) -> TransactionOptions {
    options(
        if matches!(step, "checked_out" | "unchanged" | "final" | "raced") {
            TransactionKind::Read
        } else {
            TransactionKind::Write
        },
    )
}

fn built<'a>(plans: &'a [(&str, Built)], name: &str) -> &'a Built {
    plans
        .iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, plan)| plan)
        .expect("every step names a built plan")
}

/// Where transactions open: directly or over the proxy. Cloning shares the
/// one connection, so each buyer of the race can open its own transactions.
#[derive(Clone)]
enum Target {
    Direct(Arc<Database<AppSchema>>),
    Proxy(Arc<ProxyDatabase<AppSchema>>),
}

impl Target {
    async fn transaction(&self, options: TransactionOptions) -> Result<Transaction<AppSchema>> {
        match self {
            Self::Direct(db) => Ok(db.transaction(options)),
            Self::Proxy(proxy) => proxy.transaction(options).await,
        }
    }
}

// --8<-- [start:stage]
/// Stage `names` in one transaction, commit once, and read their answers.
async fn run(
    mut tx: Transaction<AppSchema>,
    plans: &[(&str, Built)],
    names: &[&str],
) -> Result<Json> {
    let mut pending = Vec::new();
    for name in names {
        pending.push((*name, tx.stage(built(plans, name))?));
    }
    tx.commit().await?;
    // The transaction committed; each answer is known only now.
    let mut answers = Map::new();
    for (name, item) in pending {
        answers.insert(name.to_owned(), answer(&*item.get()?));
    }
    Ok(Json::Object(answers))
}
// --8<-- [end:stage]

// --8<-- [start:race]
/// Open a write transaction and stage `plan` in it.
async fn stage(
    target: &Target,
    plan: &Built,
) -> Result<(Transaction<AppSchema>, Pending<PlanAnswer>)> {
    let mut tx = target.transaction(options(TransactionKind::Write)).await?;
    let bought = tx.stage(plan)?;
    Ok((tx, bought))
}

/// One buyer: stage `plan`, wait for every buyer on the first attempt, then
/// commit; rerun the whole transaction after a retry-safe conflict. Answers
/// the plan's matched count and how many conflicts it retried.
async fn buyer(target: Target, plan: Arc<Built>, ready: Arc<Barrier>) -> Result<(u64, usize)> {
    let mut conflicts = 0;
    for attempt in 0..ATTEMPTS {
        let staged = stage(&target, &plan).await;
        if attempt == 0 {
            // Wait even when staging failed, so the other buyers never hang.
            ready.wait().await;
        }
        let (mut tx, bought) = staged?;
        match tx.commit().await {
            Ok(()) => {
                return match bought.get()?.as_ref() {
                    PlanAnswer::Matched(count) => Ok((*count, conflicts)),
                    other => panic!("a checkout answered {other:?}"),
                };
            }
            Err(error) => {
                let conflict = error.failure.as_deref().is_some_and(|failure| {
                    failure.retry_safe()
                        && matches!(
                            failure,
                            Failure::Rejected {
                                reason: Reason::Conflict { .. },
                                ..
                            }
                        )
                });
                if !conflict || attempt + 1 == ATTEMPTS {
                    return Err(error.into());
                }
                conflicts += 1;
            }
        }
    }
    unreachable!("the last attempt returns")
}

/// BUYERS buyers commit `plan` at once; answer the sum of their matched counts.
///
/// Each buyer stages the plan in its own transaction and waits until every
/// buyer has staged, so the commits overlap. A buyer that loses the race gets
/// a retry-safe conflict and nothing it staged is stored; it runs the whole
/// transaction again on a fresh snapshot, where the lamp is gone.
async fn race(target: &Target, plan: &Built) -> Result<Json> {
    let plan = Arc::new(plan.clone());
    let ready = Arc::new(Barrier::new(BUYERS));
    let mut buyers = JoinSet::new();
    for _ in 0..BUYERS {
        buyers.spawn(buyer(target.clone(), Arc::clone(&plan), Arc::clone(&ready)));
    }
    let mut matched = 0;
    let mut conflicts = 0;
    let mut first_error = None;
    while let Some(joined) = buyers.join_next().await {
        match joined.expect("a buyer task panicked") {
            Ok((count, retried)) => {
                matched += count;
                conflicts += retried;
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    // Diagnostic only: how many commits lost and were retried varies by run.
    eprintln!("race: {conflicts} conflicts retried");
    Ok(json!(["matched", matched]))
}
// --8<-- [end:race]

/// Run every step; the race step runs its plan once per buyer at once.
async fn steps(target: &Target, plans: &[(&str, Built)]) -> Result<Json> {
    let mut results = Map::new();
    for &(step, names) in STEPS {
        let answers = if step == "race" {
            let &[name] = names else {
                panic!("the race step races one plan");
            };
            let mut answers = Map::new();
            answers.insert(name.to_owned(), race(target, built(plans, name)).await?);
            Json::Object(answers)
        } else {
            run(target.transaction(kind(step)).await?, plans, names).await?
        };
        results.insert(step.to_owned(), answers);
    }
    Ok(Json::Object(results))
}

#[tokio::main]
async fn main() -> Result<()> {
    let plans = vec![
        ("seed_accounts", seed_accounts()?),
        ("seed_items", seed_items()?),
        (
            "checkout_a1",
            checkout("A1", "O1", &[("pen", 2), ("book", 1)])?,
        ),
        ("a1_paid", balance_is("A1", 54)?),
        ("book_sold_out", stock_is("book", 0)?),
        ("pens_left", stock_is("pen", 8)?),
        ("lines", lines()?),
        (
            "checkout_a2",
            checkout("A2", "O2", &[("mug", 1), ("book", 1)])?,
        ),
        ("checkout_short", checkout("A2", "O3", &[("mug", 3)])?),
        ("mugs_kept", stock_is("mug", 5)?),
        ("a2_kept", balance_is("A2", 30)?),
        ("checkout_a2_mug", checkout("A2", "O4", &[("mug", 1)])?),
        ("checkout_a2_pens", checkout("A2", "O5", &[("pen", 6)])?),
        ("a2_spent", balance_is("A2", 0)?),
        ("pens_final", stock_is("pen", 2)?),
        ("mugs_final", stock_is("mug", 4)?),
        ("race_lamp", checkout("R1", "L1", &[("lamp", 1)])?),
        ("lamp_sold_out", stock_is("lamp", 0)?),
        ("r1_paid_once", balance_is("R1", 90)?),
    ];
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
        let target = Target::Direct(Arc::new(db));
        results.insert("direct".to_owned(), steps(&target, &plans).await?);
    }
    if let Some(endpoint) = std::env::var("TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT")
        .ok()
        .filter(|url| !url.is_empty())
    {
        let proxy =
            ProxyDatabase::connect(ProxyConnectionPolicy::new(ProxyEndpoint::new(endpoint)))
                .await?
                .with_schema(models::SCHEMA)?;
        let target = Target::Proxy(Arc::new(proxy));
        results.insert("proxy".to_owned(), steps(&target, &plans).await?);
    }
    let report = json!({"binding": "rust", "plans": encoded, "results": results});
    println!("{report}");
    Ok(())
}
