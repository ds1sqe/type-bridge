//! Typed shop walkthrough. The runner owns schema setup and database cleanup.
use std::future::Future;
use std::sync::{Arc, Mutex};

use models::*;
use serde_json::{Value as Json, json};
use tracing_subscriber::{Layer, layer::SubscriberExt, registry::LookupSpan};
use type_bridge::plan::PlanResult;
use type_bridge::{ConnectionOptions, Database, ProjectedManagerComparison};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Default)]
struct Statements(Arc<Mutex<Vec<(String, String)>>>);
#[derive(Default)]
struct Fields {
    fingerprint: String,
    statement: Option<String>,
}
impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "fingerprint" => self.fingerprint = value.to_owned(),
            "statement" => self.statement = Some(value.to_owned()),
            _ => {}
        }
    }
}
impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Statements {
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields = Fields::default();
        attributes.record(&mut fields);
        if let Some(span) = context.span(id) {
            span.extensions_mut().insert(fields);
        }
    }
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != "type_bridge.io" {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let Some(statement) = fields.statement else {
            return;
        };
        if let Some(scope) = context.event_scope(event) {
            for span in scope {
                if let Some(parent) = span.extensions().get::<Fields>() {
                    if !parent.fingerprint.is_empty() {
                        fields.fingerprint.clone_from(&parent.fingerprint);
                        break;
                    }
                }
            }
        }
        self.0.lock().unwrap().push((fields.fingerprint, statement));
    }
}
impl Statements {
    fn finish(&self, expected: usize) -> Vec<(String, String)> {
        let events = std::mem::take(&mut *self.0.lock().unwrap());
        assert_eq!(events.len(), expected, "provider statement count");
        events
    }
}
fn emit(name: &str, step: &str, value: impl serde::Serialize) {
    println!("{}", json!({"case": name, "step": step, "value": value}));
}
fn trace(name: &str, events: &[(String, String)]) {
    emit(
        name,
        "plan",
        json!({"source":"TRACE", "fingerprints":events.iter().map(|(fingerprint, _)| fingerprint).collect::<Vec<_>>()}),
    );
    emit(
        name,
        "explain",
        json!({"source":"TRACE", "statements":events.iter().map(|(_, statement)| statement).collect::<Vec<_>>()}),
    );
}
trait Record {
    fn record(&self) -> Json;
}
impl Record for Account {
    fn record(&self) -> Json {
        json!({"type":"account", "iid":self.iid().to_string(), "deleted":self.deleted(), "account_id":self.account_id().value(), "balance":self.balance().map(|x| x.value())})
    }
}
impl Record for Item {
    fn record(&self) -> Json {
        json!({"type":"item", "iid":self.iid().to_string(), "deleted":self.deleted(), "sku":self.sku().value(), "price":self.price().map(|x| x.value()), "stock":self.stock().map(|x| x.value())})
    }
}
impl Record for OrderLine {
    fn record(&self) -> Json {
        json!({"type":"order-line", "iid":self.iid().to_string(), "deleted":self.deleted(), "order_id":self.order_id().map(|x| x.value()), "quantity":self.quantity().map(|x| x.value()), "buyer":self.buyer().map(|p| json!({"type":"account","iid":p.iid().to_string()})), "item":self.item().map(|p| json!({"type":"item","iid":p.iid().to_string()}))})
    }
}
impl Record for DocumentFamily {
    fn record(&self) -> Json {
        json!({"type":self.exact_type(), "iid":self.iid().to_string(), "deleted":self.deleted(), "doc_id":self.doc_id().value(), "status":self.status().value()})
    }
}
fn records<T: Record>(values: &[T]) -> Vec<Json> {
    values.iter().map(Record::record).collect()
}
fn shape<R>(plan: &Plan<R>) -> Result<Json> {
    let mut value: Json = serde_json::from_slice(&plan.canonical_bytes()?)?;
    value.as_object_mut().unwrap().remove("given_rows");
    Ok(value)
}
async fn show<T: Record>(
    db: &Database<AppSchema>,
    capture: &Statements,
    name: &str,
    call: &str,
    plan: Plan<Vec<T>>,
    expected: usize,
    split: bool,
) -> Result<Vec<T>>
where
    Vec<T>: PlanResult,
{
    emit(name, "call", call);
    let explanation = if split { None } else { Some(plan.explain()?) };
    if let Some(explanation) = &explanation {
        emit(
            name,
            "plan",
            json!({"source":"canonical_bytes", "plan":shape(&plan)?}),
        );
        let mut value = serde_json::to_value(explanation)?;
        value["source"] = "explain".into();
        emit(name, "explain", value);
    }
    let mut tx = db.transaction(Default::default());
    let pending = tx.stage(&plan)?;
    tx.commit().await?;
    let events = capture.finish(expected);
    if let Some(explanation) = explanation {
        assert_eq!(
            explanation.statements,
            events
                .iter()
                .map(|(_, statement)| statement.clone())
                .collect::<Vec<_>>()
        );
    } else {
        trace(name, &events);
    }
    emit(name, "raw", pending.documents()?);
    let values = pending.result()?;
    emit(name, "models", records(&values));
    Ok(values)
}
async fn manager<T: Record>(
    capture: &Statements,
    name: &str,
    call: &str,
    execute: impl Future<Output = Result<Vec<T>>>,
    expected: usize,
) -> Result<Vec<T>> {
    emit(name, "call", call);
    let values = execute.await?;
    trace(name, &capture.finish(expected));
    emit(name, "raw", "raw documents: not exposed by manager result");
    emit(name, "models", records(&values));
    Ok(values)
}
fn account(key: &str, balance: i64) -> Result<Plan<Vec<Account>>> {
    Ok(Plan::build(|p| {
        Account::insert(
            p,
            AccountInsert {
                account_id: key.into(),
                balance: Some(balance.into()),
            },
        )
    })?)
}
fn item(key: &str, price: i64, stock: i64) -> Result<Plan<Vec<Item>>> {
    Ok(Plan::build(|p| {
        Item::insert(
            p,
            ItemInsert {
                price: Some(price.into()),
                sku: key.into(),
                stock: Some(stock.into()),
            },
        )
    })?)
}
fn purchase() -> Result<Plan<Vec<OrderLine>>> {
    Ok(Plan::build(|p| {
        let item = Item::matching(p, "I1").where_(ItemType::stock.gt(0_i64));
        let buyer = Account::matching(p, "A1");
        let buyer = buyer.clone().where_(buyer.balance().ge(item.price()));
        item.set(p).stock(item.stock() - 1_i64);
        buyer.set(p).balance(buyer.balance() - item.price());
        OrderLine::insert(
            p,
            OrderLineInsert {
                order_id: Some("O1".into()),
                quantity: Some(1_i64.into()),
                buyer: Some(&buyer),
                item: Some(&item),
            },
        )
    })?)
}
const PURCHASE_CALL: &str = r#"Plan::build(|p| {
    let item = Item::matching(p, "I1").where_(ItemType::stock.gt(0_i64));
    let buyer = Account::matching(p, "A1");
    let buyer = buyer.clone().where_(buyer.balance().ge(item.price()));
    item.set(p).stock(item.stock() - 1_i64);
    buyer.set(p).balance(buyer.balance() - item.price());
    OrderLine::insert(p, OrderLineInsert { order_id:Some("O1".into()), quantity:Some(1_i64.into()), buyer:Some(&buyer), item:Some(&item) })
})"#;
fn item_create(stock: i64) -> Result<ItemCreate> {
    Ok(ItemCreate::new(
        Some(Price::try_new(3)?),
        Sku::try_new("M1")?,
        Some(Stock::try_new(stock)?),
    )?)
}
async fn walkthrough(db: &Database<AppSchema>, capture: &Statements) -> Result<()> {
    let mut tx = db.transaction(Default::default());
    tx.stage(&account("A1", 10)?)?;
    tx.stage(&item("I1", 3, 5)?)?;
    let report: Plan<Vec<Report>> = Plan::build(|p| {
        Report::insert(
            p,
            ReportInsert {
                doc_id: "R1".into(),
                status: "archived".into(),
            },
        )
    })?;
    let memo: Plan<Vec<Memo>> = Plan::build(|p| {
        Memo::insert(
            p,
            MemoInsert {
                doc_id: "M1".into(),
                status: "archived".into(),
            },
        )
    })?;
    tx.stage(&report)?;
    tx.stage(&memo)?;
    tx.commit().await?;
    capture.finish(4);

    let bought = show(
        db,
        capture,
        "purchase",
        PURCHASE_CALL,
        purchase()?,
        1,
        false,
    )
    .await?;
    assert_eq!(bought.len(), 1);
    assert_eq!(*bought[0].buyer().unwrap().balance().unwrap().value(), 7);
    assert_eq!(*bought[0].item().unwrap().stock().unwrap().value(), 4);
    let deletion: Plan<Vec<DocumentFamily>> = Plan::build(|p| {
        let document = p
            .subtypes::<Document>(None)
            .where_(DocumentType::status.eq("archived"));
        let selected = document.models(p, &[]);
        document.delete(p);
        selected
    })?;
    let removed = show(
        db,
        capture,
        "delete_subtypes",
        r#"Plan::build(|p| {
    let document = p.subtypes::<Document>(None).where_(DocumentType::status.eq("archived"));
    let selected = document.models(p, &[]);
    document.delete(p);
    selected
})"#,
        deletion,
        2,
        false,
    )
    .await?;
    assert_eq!(removed.len(), 2);
    assert!(removed.iter().all(DocumentFamily::deleted));
    assert_ne!(removed[0].exact_type(), removed[1].exact_type());
    let batch = Plan::build(|p| {
        Item::insert_many(
            p,
            (0..10_000).map(|n| ItemValues {
                price: Some(3),
                sku: format!("B{n}"),
                stock: Some(10),
            }),
        )
    })?;
    let items = show(db, capture, "bulk_insert", r#"Plan::build(|p| Item::insert_many(p, (0..10_000).map(|n| ItemValues { price:Some(3), sku:format!("B{n}"), stock:Some(10) })))"#, batch, 3, true).await?;
    assert_eq!(items.len(), 10_000);
    assert!(
        items
            .iter()
            .enumerate()
            .all(|(n, item)| item.sku().value() == &format!("B{n}"))
    );

    let item_manager = db.entities::<Item>();
    let made = manager(
        capture,
        "insert",
        "db.entities::<Item>().insert(item_create(5)?).await?",
        async { Ok(item_manager.insert(item_create(5)?).await?.into_items()) },
        1,
    )
    .await?;
    let iid = made[0].iid();
    manager(
        capture,
        "put",
        "db.entities::<Item>().put(item_create(6)?).await?",
        async { Ok(item_manager.put(item_create(6)?).await?.into_items()) },
        1,
    )
    .await?;
    let found = manager(
        capture,
        "typed_iid",
        "db.entities::<Item>().filter()?.iid(iid)?.all().await?",
        async { Ok(item_manager.filter()?.iid(iid)?.all().await?) },
        1,
    )
    .await?;
    assert_eq!(found[0].iid(), iid);
    manager(capture, "filtered_fetch", "db.entities::<Item>().where_(ItemType::sku, ProjectedManagerComparison::Eq, &Sku::try_new(\"M1\")?)?.all().await?", async { Ok(item_manager.where_(ItemType::sku, ProjectedManagerComparison::Eq, &Sku::try_new("M1")?)?.all().await?) }, 1).await?;
    manager(
        capture,
        "update",
        "db.entities::<Item>().filter()?.iid(iid)?.update(item_create(8)?).await?",
        async {
            Ok(item_manager
                .filter()?
                .iid(iid)?
                .update(item_create(8)?)
                .await?
                .into_items())
        },
        1,
    )
    .await?;
    manager(
        capture,
        "insert_many",
        "db.relations::<OrderLine>().insert_many(lines).await?",
        async {
            let lines = ["L1", "L2"]
                .into_iter()
                .map(|key| {
                    OrderLineCreate::new(
                        Some(OrderId::try_new(key)?),
                        Some(Quantity::try_new(1)?),
                        Some(bought[0].buyer().unwrap().reference()),
                        Some(bought[0].item().unwrap().reference()),
                    )
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(db
                .relations::<OrderLine>()
                .insert_many(lines)
                .await?
                .into_items())
        },
        1,
    )
    .await?;
    let deleted = manager(
        capture,
        "delete_returning",
        "db.entities::<Item>().delete_returning(iid).await?",
        async { Ok(item_manager.delete_returning(iid).await?.into_items()) },
        2,
    )
    .await?;
    assert!(deleted[0].deleted());

    let new_account = account("A2", 20)?;
    let new_item = item("I2", 4, 3)?;
    let new_order: Plan<Vec<OrderLine>> = Plan::build(|p| {
        let buyer = Account::matching(p, "A2");
        let item = Item::matching(p, "I2");
        OrderLine::insert(
            p,
            OrderLineInsert {
                order_id: Some("O2".into()),
                quantity: Some(1_i64.into()),
                buyer: Some(&buyer),
                item: Some(&item),
            },
        )
    })?;
    emit(
        "staged_transaction",
        "call",
        r#"let new_account = account("A2", 20)?;
let new_item = item("I2", 4, 3)?;
let new_order = Plan::build(|p| {
    let buyer = Account::matching(p, "A2");
    let item = Item::matching(p, "I2");
    OrderLine::insert(p, OrderLineInsert { order_id:Some("O2".into()), quantity:Some(1_i64.into()), buyer:Some(&buyer), item:Some(&item) })
})?;
let mut tx = db.transaction(Default::default());
let account_pending = tx.stage(&new_account)?;
let item_pending = tx.stage(&new_item)?;
let order_pending = tx.stage(&new_order)?;
tx.commit().await?;"#,
    );
    emit(
        "staged_transaction",
        "plan",
        json!({"source":"canonical_bytes", "plans":[shape(&new_account)?,shape(&new_item)?,shape(&new_order)?]}),
    );
    let explanations = [
        new_account.explain()?,
        new_item.explain()?,
        new_order.explain()?,
    ];
    emit(
        "staged_transaction",
        "explain",
        json!({"source":"explain", "plans":explanations}),
    );
    let mut tx = db.transaction(Default::default());
    let account_pending = tx.stage(&new_account)?;
    let item_pending = tx.stage(&new_item)?;
    let order_pending = tx.stage(&new_order)?;
    tx.commit().await?;
    assert_eq!(
        capture
            .finish(3)
            .into_iter()
            .map(|(_, statement)| statement)
            .collect::<Vec<_>>(),
        explanations
            .into_iter()
            .flat_map(|explanation| explanation.statements)
            .collect::<Vec<_>>()
    );
    emit(
        "staged_transaction",
        "raw",
        [
            account_pending.documents()?,
            item_pending.documents()?,
            order_pending.documents()?,
        ]
        .concat(),
    );
    let orders = order_pending.result()?;
    assert_eq!(orders[0].buyer().unwrap().account_id().value(), "A2");
    assert_eq!(orders[0].item().unwrap().sku().value(), "I2");
    emit(
        "staged_transaction",
        "models",
        [
            records(&account_pending.result()?),
            records(&item_pending.result()?),
            records(&orders),
        ]
        .concat(),
    );
    emit(
        "raw_unchecked",
        "call",
        "db.execute_query(\"match $x isa order-line; reduce $n = count;\", TransactionKind::Read).await?",
    );
    emit("raw_unchecked", "unchecked", "no plan, no hydration");
    emit(
        "raw_unchecked",
        "raw",
        db.execute_query(
            "match $x isa order-line; reduce $n = count;",
            type_bridge::raw::TransactionKind::Read,
        )
        .await?,
    );
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let capture = Statements::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(capture.clone()))?;
    let env = |name: &str, fallback: &str| std::env::var(name).unwrap_or_else(|_| fallback.into());
    let db = Database::connect(
        ConnectionOptions::new(
            std::env::var("TYPEDB_ADDRESS")?,
            std::env::var("TYPE_BRIDGE_WALKTHROUGH_DATABASE")?,
        )
        .credentials(
            env("TYPEDB_USERNAME", "admin"),
            env("TYPEDB_PASSWORD", "password"),
        )
        .http_port(std::env::var("TYPEDB_HTTP_PORT")?.parse()?),
    )
    .await?
    .with_schema(SCHEMA)?;
    let result = walkthrough(&db, &capture).await;
    db.close()?;
    result
}
