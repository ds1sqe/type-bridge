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
fn generated_field_cache_and_implicit_inputs_equal_the_explicit_builder() {
    let mut implicit = Plan::<PlanAnswer, Shop>::builder();
    let item = implicit.exact::<Item>();
    let stock = implicit.field_value(item, ItemType::stock);
    assert_eq!(
        stock.binding,
        implicit.field_value(item, ItemType::stock).binding
    );
    let minimum = implicit.bind_value(item, ItemType::stock, 2_i64);
    implicit.where_(stock.ge(minimum)).count();

    let mut explicit = Plan::<PlanAnswer, Shop>::builder();
    let item = explicit.exact::<Item>();
    let stock = explicit.field(item, ItemType::stock);
    let minimum = explicit.input::<i64>("item_stock_2");
    explicit
        .where_(stock.ge(minimum))
        .row([minimum.value(2)])
        .count();
    assert_eq!(
        implicit
            .build(&package())
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        explicit
            .build(&package())
            .unwrap()
            .canonical_bytes()
            .unwrap()
    );
}

#[test]
fn typed_recording_operators_and_class_tokens_equal_the_raw_builder() {
    use super::recording::Recording;
    use type_bridge_contract::limits::QueryLimits;
    let mut recording = Recording::<Shop>::new(QueryLimits::CANONICAL);
    let item = recording
        .matching::<Item>(false, None)
        .where_(ItemType::stock.gt(0_i64));
    let stock = item.value::<i64>(ItemType::stock.owns_id_json());
    item.set(
        &mut recording,
        ItemType::stock.owns_id_json(),
        stock - 1_i64,
    );
    let built = recording.build(&package()).unwrap();

    let mut raw = Plan::<PlanAnswer, Shop>::builder();
    let item = raw.exact::<Item>();
    let stock = raw.field(item, ItemType::stock);
    let minimum = raw.input::<i64>("item_stock_2");
    raw.where_(stock.gt(minimum));
    let quantity = raw.input::<i64>("item_stock_3");
    let next = raw.let_("item_stock_4", stock - quantity);
    raw.update(item, ItemType::stock, next);
    raw.row([minimum.value(0), quantity.value(1)]);
    assert_eq!(
        built.canonical_bytes().unwrap(),
        raw.build(&package()).unwrap().canonical_bytes().unwrap()
    );
}

#[test]
fn recording_insert_roles_and_key_inputs_equal_the_raw_builder() {
    use super::recording::Recording;
    let mut recording = Recording::<Shop>::new(Default::default());
    let item = recording
        .matching::<Item>(false, None)
        .key(ItemType::item_id.owns_id_json(), "I1".to_owned());
    let buyer = recording.matching::<Account>(false, None);
    let purchase = recording.create::<Purchase>(false);
    purchase.field::<String>(
        &mut recording,
        PurchaseType::order_id.owns_id_json(),
        "O1".to_owned(),
    );
    purchase.player(&mut recording, PurchaseType::buyer, &buyer);
    purchase.player(&mut recording, PurchaseType::item, &item);
    let built = recording.build(&package()).unwrap();

    let mut raw = Plan::<PlanAnswer, Shop>::builder();
    let item = raw.exact::<Item>();
    let key = raw.field(item, ItemType::item_id);
    let key_input = raw.core.bind_value(
        super::Site::new(item.binding, "key"),
        type_bridge_contract::value::CanonicalValue::String(
            type_bridge_contract::value::CanonicalString::new("I1").unwrap(),
        ),
    );
    raw.core.compare(
        super::QueryOperand::Binding {
            binding: key.binding,
        },
        super::ValueComparator::Equal,
        key_input,
    );
    let buyer = raw.exact::<Account>();
    let purchase = raw.insert::<Purchase>().thing();
    let order = raw.bind_value(purchase, PurchaseType::order_id, "O1".to_owned());
    raw.core.thing_field(
        purchase.binding,
        super::owns_id(PurchaseType::order_id.owns_id_json()).unwrap(),
        order.0.unwrap(),
    );
    raw.core.thing_player(
        purchase.binding,
        super::role_id(PurchaseType::buyer.role_id_json()).unwrap(),
        buyer.binding,
    );
    raw.core.thing_player(
        purchase.binding,
        super::role_id(PurchaseType::item.role_id_json()).unwrap(),
        item.binding,
    );
    assert_eq!(
        built.canonical_bytes().unwrap(),
        raw.build(&package()).unwrap().canonical_bytes().unwrap()
    );
}

#[tokio::test]
async fn empty_bulk_keeps_the_commit_barrier_without_provider_io() {
    let built: Plan<Vec<Item>, Shop> =
        Plan::build(|p| p.bulk::<Item>(false, &[], vec![]).models(p, &[])).unwrap();
    assert!(built.writes());
    assert!(
        <super::PlanInner as AsRef<[type_bridge_contract::query_plan::QueryPlan]>>::as_ref(
            built.shared_inner().unwrap().as_ref()
        )
        .is_empty()
    );
    let raw: Plan<PlanAnswer, Shop> = Plan {
        inner: built.inner,
        marker: std::marker::PhantomData,
    };
    let (db, log) = database();
    let mut tx = db.transaction(TransactionOptions::default());
    let answer = tx.stage(&raw).unwrap();
    assert!(matches!(answer.get(), Err(PendingError::NotCommitted)));
    tx.commit().await.unwrap();
    assert_eq!(*answer.get().unwrap(), PlanAnswer::Documents(vec![]));
    assert!(answer.documents().unwrap().is_empty());
    assert!(log.lock().unwrap().0.is_empty());
}

#[test]
fn typed_recording_refuses_foreign_and_closed_values() {
    use super::recording::Recording;
    use type_bridge_contract::limits::QueryLimits;
    let mut first = Recording::<Shop>::new(QueryLimits::CANONICAL);
    let first_item = first.matching::<Item>(false, None);
    let value = first_item.value::<i64>(ItemType::stock.owns_id_json());
    let mut second = Recording::<Shop>::new(QueryLimits::CANONICAL);
    let second_item = second.matching::<Item>(false, None);
    second.require(
        second_item
            .value::<i64>(ItemType::stock.owns_id_json())
            .ge(value.clone()),
    );
    second.count();
    assert_eq!(
        second.build(&package()).unwrap_err().code(),
        Some("plan_context_missing")
    );
    first.count();
    first.build(&package()).unwrap();
    let mut third = Recording::<Shop>::new(QueryLimits::CANONICAL);
    third.matching::<Item>(false, None);
    third.require(value.gt(0_i64));
    third.count();
    assert_eq!(
        third.build(&package()).unwrap_err().code(),
        Some("plan_context_missing")
    );
}

#[tokio::test]
async fn closure_records_typed_count_and_default_matched_results() {
    let count: Plan<i64, Shop> = Plan::build(|p| {
        p.matching::<Item>(false, None);
        p.count()
    })
    .unwrap();
    let matched: Plan<super::Matched, Shop> = Plan::build(|p| {
        p.matching::<Item>(false, None);
    })
    .unwrap();
    let (db, _) = database();
    let mut tx = db.transaction(TransactionOptions::default());
    let count = tx.stage(&count).unwrap();
    let matched = tx.stage(&matched).unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count.result().unwrap(), 1);
    assert_eq!(matched.result().unwrap().count, 1);
}

#[test]
fn closure_limits_and_foreign_selection_fail_during_build() {
    let mut limits = type_bridge_contract::limits::QueryLimits::CANONICAL;
    limits.codec.max_bytes = 512;
    limits.codec.max_string_bytes = 512;
    let result = Plan::<i64, Shop>::with_limits(limits).build(|p| {
        p.matching::<Item>(false, None);
        p.count()
    });
    assert_eq!(result.unwrap_err().code(), Some("canonical_json_too_large"));
    let mut other = super::recording::Recording::<Shop>::new(Default::default());
    let foreign = other.count();
    let result = Plan::<i64, Shop>::build(|p| {
        p.matching::<Item>(false, None);
        foreign
    });
    assert_eq!(result.unwrap_err().code(), Some("plan_context_missing"));
}

#[test]
fn held_results_allocate_only_when_the_closure_finishes() {
    let mut leaked = None;
    let plan: Plan<i64, Shop> = Plan::build(|p| {
        p.matching::<Item>(false, None);
        let held = p.count();
        assert!(matches!(held.inner, super::PlanState::Held(_)));
        leaked = Some(held);
        p.count()
    })
    .unwrap();
    assert!(matches!(plan.inner, super::PlanState::Built(_)));
    let unbuilt = leaked.unwrap();
    assert_eq!(
        unbuilt.canonical_bytes().unwrap_err().code(),
        Some("plan_context_missing")
    );
    let (db, _) = database();
    let mut tx = db.transaction(TransactionOptions::default());
    assert!(matches!(
        tx.stage(&unbuilt),
        Err(PendingError::InvalidPlan(_))
    ));
}

#[test]
fn recording_uses_caller_query_limits() {
    let mut limits = type_bridge_contract::limits::QueryLimits::CANONICAL;
    limits.codec.max_bytes = 512;
    limits.codec.max_string_bytes = 512;
    let mut plan = super::PlanBuilder::<Shop>::with_limits(limits);
    plan.exact::<Item>();
    plan.count();
    assert_eq!(
        plan.build(&package()).unwrap_err().code(),
        Some("canonical_json_too_large")
    );
}

#[test]
fn iid_match_uses_the_shared_builder() {
    let mut plan = Plan::<PlanAnswer, Shop>::builder();
    let item = plan.by_iid::<Item>(&crate::Iid::new("0x01").unwrap());
    plan.returning([item]);
    let bytes = plan.build(&package()).unwrap().canonical_bytes().unwrap();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["pipeline"][0]["patterns"][1]["iid"], "0x01");
}

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
    let mut plan = Plan::<PlanAnswer, Shop>::builder();
    let item = plan.exact::<Item>();
    plan.input::<String>("item");
    plan.returning([item]);
    let error = plan.build(&package()).unwrap_err();
    assert_eq!(error.code(), Some("query_plan_builder_duplicate_name"));

    // A package without compiled schema authority cannot build plans.
    let full = package();
    let without_authority = crate::SchemaPackage::<Shop>::new(
        full.semantic_fingerprint_json(),
        full.projection_fingerprint_json(),
        full.runtime_projection_json(),
    );
    let mut plan = Plan::<PlanAnswer, Shop>::builder();
    let item = plan.exact::<Item>();
    plan.returning([item]);
    assert!(plan.build(&without_authority).is_err());

    // Plan validation runs at build: a text value cannot replace stock.
    let mut plan = Plan::<PlanAnswer, Shop>::builder();
    let item = plan.exact::<Item>();
    plan.update(item, ItemType::stock, "none".to_owned());
    assert!(plan.build(&package()).is_err());
}

#[test]
fn builds_verify_a_package_authority_once_and_a_tampered_one_every_time() {
    let verifications = || crate::schema::AUTHORITY_VERIFICATIONS.with(std::cell::Cell::get);
    let item_plan = || {
        let mut plan = Plan::<PlanAnswer, Shop>::builder();
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
        let mut plan = Plan::<PlanAnswer, Shop>::builder();
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
struct Log(
    Vec<String>,
    Option<Vec<serde_json::Value>>,
    Option<Vec<serde_json::Value>>,
);

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
        let documents = if q.contains("fetch") {
            self.0.lock().unwrap().1.clone()
        } else {
            None
        };
        let rows = self
            .0
            .lock()
            .unwrap()
            .2
            .clone()
            .unwrap_or_else(|| vec![serde_json::json!({})]);
        Box::pin(async move {
            Ok(documents
                .map(QueryResult::Documents)
                .unwrap_or(QueryResult::Rows(rows)))
        })
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

#[derive(Debug)]
struct ReturnedItem {
    iid: String,
    origin: crate::__codegen::ReferenceOrigin,
}
impl crate::schema::sealed::Sealed for ReturnedItem {}
impl crate::__codegen::Model for ReturnedItem {
    type Schema = Shop;
    const TYPE_ID_JSON: &'static str = <Item as crate::__codegen::Model>::TYPE_ID_JSON;
}
impl crate::__codegen::ThingModel for ReturnedItem {
    fn thing_kind() -> crate::__codegen::ThingKind {
        crate::__codegen::ThingKind::Entity
    }
}
impl crate::__codegen::MaterializeModel for ReturnedItem {
    fn materialize(
        row: &crate::__codegen::HydratedRow,
        _cap: &crate::__codegen::HydrationCapability,
    ) -> std::result::Result<Self, crate::__codegen::ValidationError> {
        Ok(Self {
            iid: row.iid().to_owned(),
            origin: row.origin().clone(),
        })
    }
}

#[tokio::test]
async fn model_pending_consumes_the_core_proof_once_and_preserves_direct_deleted_origins() {
    for deleted in [false, true] {
        let plan: Plan<Vec<ReturnedItem>, Shop> = Plan::build(|p| {
            let item = p.matching::<ReturnedItem>(false, None);
            if deleted {
                item.delete(p);
            }
            item.models(p, &[])
        })
        .unwrap();
        let (db, log) = database();
        log.lock().unwrap().1 = Some(vec![
            serde_json::json!({"_iid":"0x01", "_type":"item", "attributes":{"item-id":"I1", "price":3, "stock":2}}),
        ]);
        let mut tx = db.transaction(Default::default());
        let pending = tx.stage(&plan).unwrap();
        assert!(pending.result().is_err());
        assert!(pending.documents().is_err());
        tx.commit().await.unwrap();
        let documents = pending.documents().unwrap();
        assert_eq!(documents[0]["_iid"], "0x01");
        assert_eq!(documents.as_ptr(), pending.documents().unwrap().as_ptr());
        assert_eq!(
            documents.as_ptr(),
            pending.clone().documents().unwrap().as_ptr()
        );
        let calls = log.lock().unwrap().0.len();
        let values = pending.result().unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].iid, "0x01");
        assert!(values[0].origin.projected().is_some());
        assert_eq!(values[0].origin.deleted(), deleted);
        let super::PendingAnswers::Models {
            projected, answer, ..
        } = &pending.answer
        else {
            panic!("core model pending")
        };
        let original = std::ptr::from_ref(projected.get().unwrap());
        let proof = answer.get().unwrap();
        let PlanAnswer::Documents(raw) = proof.selection() else {
            panic!("document selection")
        };
        assert_eq!(documents.as_ptr(), raw.as_ptr());
        assert!(Arc::ptr_eq(&proof, &answer.get().unwrap()));
        assert_eq!(pending.result().unwrap()[0].iid, "0x01");
        assert_eq!(original, std::ptr::from_ref(projected.get().unwrap()));
        assert_eq!(calls, log.lock().unwrap().0.len());
    }
}

#[tokio::test]
async fn returning_mixed_and_scalar_tuples_preserves_types_duplicates_and_origins() {
    let mixed: Plan<Vec<(ReturnedItem, i64)>, Shop> = Plan::build(|p| {
        let item = p.matching::<ReturnedItem>(false, None);
        let stock = item.value::<i64>(ItemType::stock.owns_id_json());
        p.returning((&item, stock))
    })
    .unwrap();
    let scalars: Plan<Vec<(i64,)>, Shop> = Plan::build(|p| {
        let item = p.matching::<ReturnedItem>(false, None);
        let stock = item.value::<i64>(ItemType::stock.owns_id_json());
        p.returning((stock,))
    })
    .unwrap();
    let (db, log) = database();
    let mut row = serde_json::Map::new();
    let plan = mixed.inner().unwrap();
    for binding in plan.bindings() {
        let value = if plan.scalar_value_type(binding.id()).is_some() {
            type_bridge_contract::query_remote::RemoteValue::Value {
                value: type_bridge_contract::value::CanonicalValue::Long(2),
            }
        } else {
            type_bridge_contract::query_remote::RemoteValue::Thing {
                iid: "0x01".into(),
                type_id: type_bridge_contract::id::TypeId::new(
                    type_bridge_contract::id::TypeKind::Entity,
                    "item",
                )
                .unwrap(),
            }
        };
        row.insert(
            binding.variable().as_str().to_owned(),
            serde_json::to_value(value).unwrap(),
        );
    }
    let row = serde_json::Value::Object(row);
    log.lock().unwrap().2 = Some(vec![row.clone(), row]);
    log.lock().unwrap().1 = Some(vec![
        serde_json::json!({"_iid":"0x01", "_type":"item", "attributes":{"item-id":"I1", "price":3, "stock":2}}),
    ]);
    let mut tx = db.transaction(Default::default());
    let pending = tx.stage(&mixed).unwrap();
    let scalar_pending = tx.stage(&scalars).unwrap();
    tx.commit().await.unwrap();
    let values = pending.result().unwrap();
    assert_eq!(values.len(), 2);
    for (model, stock) in values {
        assert_eq!(model.iid, "0x01");
        assert_eq!(stock, 2);
        assert!(model.origin.projected().is_some());
    }
    assert_eq!(scalar_pending.result().unwrap(), vec![(2,), (2,)]);
}

/// A restock without inputs, so the mock backend needs no `given` support.
fn restock() -> Plan<PlanAnswer, Shop> {
    let mut plan = Plan::builder();
    let item = plan.exact::<Item>();
    let stock = plan.field(item, ItemType::stock);
    plan.where_(stock.eq(0_i64))
        .update(item, ItemType::stock, 10_i64);
    plan.build(&package()).unwrap()
}

#[test]
fn erasing_a_brand_reuses_the_non_generic_inner_plan() {
    let original = restock();
    let shared = original.clone();
    let erased = original.erase();
    assert!(Arc::ptr_eq(
        shared.shared_inner().unwrap(),
        erased.shared_inner().unwrap()
    ));
    let branded = erased.validate::<Shop>().unwrap();
    assert!(Arc::ptr_eq(
        shared.shared_inner().unwrap(),
        branded.shared_inner().unwrap()
    ));
    let (db, _) = database();
    let mut tx = db.transaction(TransactionOptions::default());
    let pending = tx.stage(&branded).unwrap();
    assert!(Arc::ptr_eq(branded.shared_inner().unwrap(), &pending.plan));
}

#[tokio::test]
async fn typed_pending_decodes_count_and_keeps_commit_visibility() {
    let mut builder = Plan::<PlanAnswer, Shop>::builder();
    builder.exact::<Item>();
    builder.count();
    let raw = builder.build(&package()).unwrap();
    let plan: Plan<i64, Shop> = Plan {
        inner: raw.inner,
        marker: std::marker::PhantomData,
    };
    let (db, _) = database();
    let mut tx = db.transaction(TransactionOptions::default());
    let answer = tx.stage(&plan).unwrap();
    assert!(answer.result().is_err());
    assert!(answer.documents().is_err());
    assert!(Arc::ptr_eq(plan.shared_inner().unwrap(), &answer.plan));
    tx.commit().await.unwrap();
    assert_eq!(answer.result().unwrap(), 1_i64);
    assert_eq!(
        answer.documents().unwrap_err().code(),
        Some("plan_result_shape")
    );
}

#[test]
fn typed_results_refuse_the_wrong_answer_without_values() {
    use super::PlanResult;
    let plan = restock();
    assert_eq!(
        bool::decode(plan.inner().unwrap(), &PlanAnswer::Matched(99))
            .unwrap_err()
            .code(),
        Some("plan_result_shape")
    );
    assert_eq!(
        super::Matched::decode(plan.inner().unwrap(), &PlanAnswer::Matched(99))
            .unwrap()
            .count,
        99
    );
}

#[test]
fn unbranded_staging_has_no_schema_preflight_and_validation_is_explicit() {
    let (db, _) = database();
    let (declared, semantics) = authority();
    let foreign =
        type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint::compute(
            semantics
                .as_fingerprint()
                .semantic_profile()
                .unwrap()
                .clone(),
            b"another-scope",
        )
        .unwrap();
    let mut core = type_bridge_contract::query_plan::PlanBuilder::new();
    core.exact(super::type_id::<Item>().unwrap());
    core.count();
    let plan: Plan = Plan::from_core(core.build(&declared, foreign).unwrap());
    let before = crate::schema::AUTHORITY_VERIFICATIONS.with(std::cell::Cell::get);
    let mut tx = db.transaction(TransactionOptions::default());
    tx.stage(&plan).unwrap();
    assert_eq!(
        crate::schema::AUTHORITY_VERIFICATIONS.with(std::cell::Cell::get),
        before
    );
    tx.rollback().unwrap();
    assert_eq!(
        plan.brand::<Shop>().unwrap_err().code(),
        Some("plan_schema_mismatch")
    );
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
    let mut stock = Plan::<PlanAnswer, Shop>::builder();
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

#[test]
fn fetch_bytes_equal_the_core_builder_and_conflicts_surface() {
    use crate::plan::FetchRole;
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tests/fixtures/plan_builder/purchase.json"
    )))
    .unwrap();
    for required in [false, true] {
        let mut plan = Plan::<PlanAnswer, Shop>::builder();
        let purchase = plan.exact::<Purchase>();
        let key = plan.field(purchase, PurchaseType::order_id);
        let order = plan.input::<String>("order_id");
        plan.where_(key.eq(order)).fetch(
            purchase,
            [
                FetchRole::new(PurchaseType::buyer, true),
                FetchRole::new(PurchaseType::item, required),
            ],
        );
        plan.row([order.value("O1".to_owned())]);
        let bytes = plan.build(&package()).unwrap().canonical_bytes().unwrap();
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            fixture[if required {
                "read_back_hex"
            } else {
                "read_back_optional_hex"
            }]
        );
    }
    for fetch_first in [false, true] {
        let mut plan = Plan::<PlanAnswer, Shop>::builder();
        let thing = plan.exact::<Purchase>();
        if fetch_first {
            plan.fetch(thing, []);
        }
        plan.count();
        if !fetch_first {
            plan.fetch(thing, []);
        }
        assert_eq!(
            plan.build(&package()).unwrap_err().code(),
            Some("query_plan_builder_terminal_conflict")
        );
    }
    let mut plan = Plan::<PlanAnswer, Shop>::builder();
    let thing = plan.exact::<Purchase>();
    plan.fetch(thing, []).fetch(thing, []);
    assert_eq!(
        plan.build(&package()).unwrap_err().code(),
        Some("query_plan_builder_fetch_once")
    );
}

#[test]
fn bulk_introspection_uses_one_real_chunk_and_refuses_ambiguous_programs() {
    use super::{PlanInner, PlanProgram, PlanState};
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::query_plan::PlanBuilder as CoreBuilder;
    let authority = package().plan_authority().unwrap();
    for count in [0, 1, 2] {
        let mut core = CoreBuilder::new();
        let item = core.insert(TypeId::new(TypeKind::Entity, "item").unwrap());
        core.bind_model_rows(item, vec![vec![]; count]);
        let built = core
            .build_plans(
                authority.declared_schema(),
                authority.managed_state().managed_semantic_schema().clone(),
                true,
                type_bridge_contract::split::SplitLimits::CANONICAL,
            )
            .unwrap();
        let expected = built
            .plans()
            .first()
            .map(|plan| plan.canonical_bytes().unwrap());
        let plan = Plan::<super::Matched, Shop> {
            inner: PlanState::Built(Arc::new(PlanInner {
                program: PlanProgram::Bulk(built),
            })),
            marker: std::marker::PhantomData,
        };
        if count == 1 {
            assert_eq!(plan.canonical_bytes().unwrap(), expected.unwrap());
            assert_eq!(plan.explain().unwrap().write_statements, 1);
        } else {
            assert_eq!(
                plan.canonical_bytes().unwrap_err().code(),
                Some("plan_batch_split")
            );
            assert_eq!(plan.explain().unwrap_err().code(), Some("plan_batch_split"));
        }
        assert!(plan.writes());
    }
}

#[test]
fn authored_string_conditions_equal_the_raw_core_comparisons() {
    use super::recording::Recording;
    let mut recording = Recording::<Shop>::new(Default::default());
    let item = recording
        .matching::<Item>(false, None)
        .where_(ItemType::item_id.contains("I"))
        .where_(ItemType::item_id.starts_with("I"))
        .where_(ItemType::item_id.ends_with("I"))
        .where_(ItemType::item_id.like("I"));
    let value = item.value::<String>(ItemType::item_id.owns_id_json());
    item.where_(value.contains("I"))
        .where_(value.starts_with("I"))
        .where_(value.ends_with("I"))
        .where_(value.like("I"));
    let authored = recording.build(&package()).unwrap();
    let mut raw = Plan::<PlanAnswer, Shop>::builder();
    let item = raw.exact::<Item>();
    let field = raw.field_value(item, ItemType::item_id);
    for comparator in [
        super::ValueComparator::Contains,
        super::ValueComparator::StartsWith,
        super::ValueComparator::EndsWith,
        super::ValueComparator::Like,
    ]
    .into_iter()
    .cycle()
    .take(8)
    {
        let right = raw.bind_value(item, ItemType::item_id, "I".to_owned());
        raw.core.compare(
            super::QueryOperand::Binding {
                binding: field.binding,
            },
            comparator,
            right.0.unwrap(),
        );
    }
    raw.count();
    assert_eq!(
        authored.canonical_bytes().unwrap(),
        raw.build(&package()).unwrap().canonical_bytes().unwrap()
    );
}

#[tokio::test]
async fn authored_iid_stack_binds_inside_one_transaction_and_keeps_commit_visibility() {
    use type_bridge_contract::query_remote::RemoteValue;
    let (db, log) = database();
    log.lock().unwrap().2 = Some(vec![serde_json::json!({"item": RemoteValue::Thing {
        iid: "0x10".into(), type_id: super::type_id::<Item>().unwrap(),
    }})]);
    let mut source = Plan::<PlanAnswer, Shop>::builder();
    let item = source.exact::<Item>();
    source.returning([item]);
    let source = source.build_staged(&package(), vec![], vec![]).unwrap();
    let mut target = Plan::<PlanAnswer, Shop>::builder();
    let iid = target.input::<crate::Iid>("target_iid");
    let item = target.by_iid_input::<Item>(iid);
    target
        .update(item, ItemType::stock, 7_i64)
        .returning([item]);
    let reference = iid.result_ref::<Item>(0, 0).unwrap();
    let target = target
        .build_staged(&package(), vec![vec![iid.absent()]; 3], vec![reference])
        .unwrap();
    assert_eq!(target.result_refs()[0].source_plan, 0);
    let mut transaction = db.transaction(TransactionOptions::default());
    let pending = transaction.stage_stack(&[source, target]).unwrap();
    assert!(log.lock().unwrap().0.is_empty());
    assert!(
        pending
            .iter()
            .all(|answer| answer.get().unwrap_err() == PendingError::NotCommitted)
    );
    transaction.commit().await.unwrap();
    assert!(pending.iter().all(
        |answer| matches!(answer.get().unwrap().as_ref(), PlanAnswer::Rows(rows) if rows.len() == 1)
    ));
    let events = &log.lock().unwrap().0;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.starts_with("open "))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.starts_with("query "))
            .count(),
        2
    );
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("query ") && event.contains("0x10"))
    );
    assert_eq!(events.iter().filter(|event| *event == "commit").count(), 1);
}

#[test]
fn nominal_iid_authoring_preserves_literal_and_input_plan_bytes() {
    for input in [false, true] {
        let iid = crate::Iid::new("0xAb").unwrap();
        let mut typed = Plan::<PlanAnswer, Shop>::builder();
        let mut original = Plan::<PlanAnswer, Shop>::builder();
        if input {
            let column = typed.input::<crate::Iid>("iid");
            let thing = typed.by_iid_input::<Item>(column);
            typed.row([column.value(iid)]).returning([thing]);
            let column = original.input::<String>("iid");
            let thing = original.subtypes::<Item>();
            original.core.by_iid_input(thing.binding, column.column);
            original
                .row([column.value("0xAb".into())])
                .returning([thing]);
        } else {
            let thing = typed.by_iid::<Item>(&iid);
            typed.returning([thing]);
            let thing = original.subtypes::<Item>();
            original.core.by_iid(thing.binding, "0xAb");
            original.returning([thing]);
        }
        assert_eq!(
            typed.build(&package()).unwrap().canonical_bytes().unwrap(),
            original
                .build(&package())
                .unwrap()
                .canonical_bytes()
                .unwrap()
        );
    }
}
