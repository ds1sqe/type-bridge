use std::sync::{Arc, Mutex};

use type_bridge_orm::OrmError;
use type_bridge_orm::session::backend::{
    BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
};

#[path = "../../tests/support/purchase_models.rs"]
mod models;
use models::*;

use crate::Database;
use crate::plan::Plan;
use crate::staged::{PendingError, PlanAnswer, TransactionKind, TransactionOptions};

#[test]
fn builder_purchase_bytes_equal_the_hand_built_plan() {
    let built = purchase_plan(&package(), "O1", "I1", "A1").unwrap();
    let hand = hand_built_purchase("O1", "I1", "A1");
    assert_eq!(
        built.canonical_bytes().unwrap(),
        hand.canonical_bytes().unwrap()
    );
    assert!(built.writes());
}

#[test]
fn builder_reports_invalid_calls_and_unbuildable_packages() {
    let mut plan = Plan::<Shop>::builder();
    let item = plan.exact::<Item>();
    plan.input::<String>("item");
    plan.returning([item]);
    let error = plan.build(&package()).unwrap_err();
    assert_eq!(error.code(), Some("query_plan_builder_duplicate_name"));

    // A package without compiled schema authority cannot build plans.
    let full = package();
    let legacy = crate::SchemaPackage::<Shop>::new(
        full.semantic_fingerprint_json(),
        full.projection_fingerprint_json(),
        full.runtime_projection_json(),
    );
    let mut plan = Plan::<Shop>::builder();
    let item = plan.exact::<Item>();
    plan.returning([item]);
    assert!(plan.build(&legacy).is_err());

    // Plan validation runs at build: a text value cannot replace stock.
    let mut plan = Plan::<Shop>::builder();
    let item = plan.exact::<Item>();
    plan.update(item, ItemType::stock, "none".to_owned());
    assert!(plan.build(&package()).is_err());
}

#[test]
fn builds_verify_a_package_authority_once_and_a_tampered_one_every_time() {
    let verifications = || crate::schema::AUTHORITY_VERIFICATIONS.with(std::cell::Cell::get);
    let item_plan = || {
        let mut plan = Plan::<Shop>::builder();
        let item = plan.exact::<Item>();
        plan.returning([item]);
        plan
    };

    // `package()` leaks fresh evidence, so no other test has verified it.
    let package = package();
    let start = verifications();
    item_plan().build(&package).unwrap();
    item_plan().build(&package).unwrap();
    // Copies of a package share its evidence and its verification.
    let copy = package;
    item_plan().build(&copy).unwrap();
    assert_eq!(verifications() - start, 1);

    let tampered = tampered_package();
    let start = verifications();
    let first = item_plan().build(&tampered).unwrap_err();
    let second = item_plan().build(&tampered).unwrap_err();
    assert_eq!(verifications() - start, 2);
    assert_eq!(first.to_string(), second.to_string());
    assert!(
        first
            .to_string()
            .contains("disagrees with its extracted query evidence"),
        "{first}"
    );
}

#[test]
fn reads_choose_rows_count_and_exists() {
    let read = |terminal: fn(&mut crate::plan::PlanBuilder<Shop>)| {
        let mut plan = Plan::<Shop>::builder();
        let item = plan.exact::<Item>();
        let stock = plan.field(item, ItemType::stock);
        plan.where_(stock.ge(1_i64)).returning([stock]);
        terminal(&mut plan);
        plan.build(&package()).unwrap()
    };
    assert!(!read(|_| {}).writes());
    assert!(
        !read(|plan| {
            plan.count();
        })
        .writes()
    );
    let exists = read(|plan| {
        plan.exists();
    });
    assert_ne!(
        exists.canonical_bytes().unwrap(),
        read(|_| {}).canonical_bytes().unwrap()
    );
}

#[derive(Default)]
struct Log(Vec<String>);

struct Backend(Arc<Mutex<Log>>);
struct Tx(Arc<Mutex<Log>>);

impl DriverBackend for Backend {
    fn open_transaction(
        &self,
        _db: &str,
        ty: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.0.lock().unwrap().0.push(format!("open {ty:?}"));
        let tx = Tx(Arc::clone(&self.0));
        Box::pin(async move { Ok(Box::new(tx) as Box<dyn TransactionOps>) })
    }
    fn is_open(&self) -> bool {
        true
    }
}

impl TransactionOps for Tx {
    fn query(&mut self, q: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        self.0.lock().unwrap().0.push(format!("query {q}"));
        Box::pin(async { Ok(QueryResult::Rows(vec![serde_json::json!({})])) })
    }
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.0.lock().unwrap().0.push("commit".into());
        Box::pin(async { Ok(()) })
    }
    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.0.lock().unwrap().0.push("rollback".into());
        Box::pin(async { Ok(()) })
    }
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
        self.0.lock().unwrap().0.push("close".into());
        Box::pin(async { Ok(()) })
    }
}

fn database() -> (Database<Shop>, Arc<Mutex<Log>>) {
    let log = Arc::new(Mutex::new(Log::default()));
    let inner = type_bridge_orm::Database::with_backend(Box::new(Backend(Arc::clone(&log))), "t");
    let database = Database::from_orm_database(inner)
        .with_schema(package())
        .unwrap();
    (database, log)
}

/// A restock without inputs, so the mock backend needs no `given` support.
fn restock() -> Plan<Shop> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let stock = plan.field(item, ItemType::stock);
    plan.where_(stock.eq(0_i64))
        .update(item, ItemType::stock, 10_i64);
    plan.build(&package()).unwrap()
}

#[tokio::test]
async fn direct_transaction_stages_until_commit_and_rollback_keeps_it() {
    let (db, log) = database();
    let mut tx = db.transaction(TransactionOptions::default());
    let discarded = tx.stage(&restock()).unwrap();
    tx.rollback().unwrap();
    assert_eq!(discarded.get().unwrap_err(), PendingError::RolledBack);
    assert!(
        log.lock().unwrap().0.is_empty(),
        "nothing opens before commit"
    );

    let answer = tx.stage(&restock()).unwrap();
    assert_eq!(answer.get().unwrap_err(), PendingError::NotCommitted);
    tx.commit().await.unwrap();
    assert_eq!(*answer.get().unwrap(), PlanAnswer::Matched(1));
    let events = log.lock().unwrap().0.clone();
    assert_eq!(events.first().map(String::as_str), Some("open Write"));
    assert!(events.iter().any(|event| event == "commit"));
    assert!(!tx.has_unknown_outcome());
    assert!(
        tx.commit_retry().await.is_err(),
        "a direct commit has no retry"
    );

    tx.close().unwrap();
    assert_eq!(tx.stage(&restock()).unwrap_err(), PendingError::Closed);
}

#[tokio::test]
async fn transaction_kinds_refuse_plans_they_cannot_run() {
    let (db, _) = database();
    let mut read = db.transaction(TransactionOptions::default().with_kind(TransactionKind::Read));
    assert_eq!(read.stage(&restock()).unwrap_err(), PendingError::ReadOnly);

    let mut schema =
        db.transaction(TransactionOptions::default().with_kind(TransactionKind::Schema));
    let mut stock = Plan::<Shop>::builder();
    let item = stock.exact::<Item>();
    stock.returning([item]).count();
    let stock = stock.build(&package()).unwrap();
    assert!(matches!(
        schema.stage(&stock).unwrap_err(),
        PendingError::NotAllowedInKind(_)
    ));
    assert!(
        read.stage(&stock).is_ok(),
        "a read transaction stages reads"
    );
}
