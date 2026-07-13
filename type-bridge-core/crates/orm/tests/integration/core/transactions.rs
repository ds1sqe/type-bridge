//! Dynamic transaction-bound manager integration tests against TypeDB.
//!

use std::collections::BTreeSet;
use std::env;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::common::dynamic_crud::*;
use crate::common::typedb::connect_options_from_env;
use tokio::sync::Notify;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan,
};
use type_bridge_orm::session::backend::{
    AnswerCancellation, AnswerConsumer, BoundedAnswerLimits, BoundedAnswerStats, BoxFuture,
    DriverBackend, QueryResult, TransactionOps,
};
use type_bridge_orm::session::real_driver::RealBackend;
use type_bridge_orm::*;

#[derive(Default)]
struct LifecycleCounts {
    opens: AtomicUsize,
    closes: AtomicUsize,
    commits: AtomicUsize,
    rollbacks: AtomicUsize,
}

#[derive(Clone)]
struct RootScanBarrier {
    root_selected: Arc<Notify>,
    writer_finished: Arc<Notify>,
}

struct ObservedRealBackend {
    inner: RealBackend,
    lifecycle: Arc<LifecycleCounts>,
    root_scan_barrier: Option<RootScanBarrier>,
}

impl DriverBackend for ObservedRealBackend {
    fn match_capabilities(&self) -> CapabilitySet {
        self.inner.match_capabilities()
    }

    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>>> {
        let database = database.to_owned();
        let lifecycle = Arc::clone(&self.lifecycle);
        let root_scan_barrier = self.root_scan_barrier.clone();
        Box::pin(async move {
            let inner = self.inner.open_transaction(&database, tx_type).await?;
            lifecycle.opens.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(ObservedRealTransaction {
                inner,
                lifecycle,
                root_scan_barrier,
                paused: false,
            }) as Box<dyn TransactionOps>)
        })
    }

    fn is_open(&self) -> bool {
        self.inner.is_open()
    }

    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        self.inner.server_version()
    }
}

struct ObservedRealTransaction {
    inner: Box<dyn TransactionOps>,
    lifecycle: Arc<LifecycleCounts>,
    root_scan_barrier: Option<RootScanBarrier>,
    paused: bool,
}

impl TransactionOps for ObservedRealTransaction {
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult>> {
        self.inner.query(typeql)
    }

    fn query_bounded<'a>(
        &'a mut self,
        typeql: &'a str,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats>> {
        self.inner.query_bounded(typeql, limits, consumer)
    }

    fn query_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats>> {
        self.inner.query_typed_bounded(query, limits, consumer)
    }

    fn query_root_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats>> {
        Box::pin(async move {
            let result = self
                .inner
                .query_root_typed_bounded(query, limits, consumer)
                .await;
            // A page-selection scan always carries a window offset (including
            // `Some(0)`), while the optional total scan does not. Pause only
            // after the selected root identities have been captured.
            if query.offset.is_some()
                && !self.paused
                && let Some(barrier) = &self.root_scan_barrier
            {
                self.paused = true;
                barrier.root_selected.notify_one();
                barrier.writer_finished.notified().await;
            }
            result
        })
    }

    fn rematch_page_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats>> {
        self.inner
            .rematch_page_typed_bounded(query, limits, consumer)
    }

    fn hydrate_typed_bounded<'a>(
        &'a mut self,
        query: &'a TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &'a mut dyn AnswerConsumer,
    ) -> BoxFuture<'a, Result<BoundedAnswerStats>> {
        self.inner.hydrate_typed_bounded(query, limits, consumer)
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.lifecycle.commits.fetch_add(1, Ordering::SeqCst);
            self.inner.commit().await
        })
    }

    fn rollback(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.lifecycle.rollbacks.fetch_add(1, Ordering::SeqCst);
            self.inner.rollback().await
        })
    }

    fn close(&mut self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.lifecycle.closes.fetch_add(1, Ordering::SeqCst);
            self.inner.close().await
        })
    }
}

async fn observed_database(
    database_name: &str,
    root_scan_barrier: Option<RootScanBarrier>,
) -> (Database, Arc<LifecycleCounts>) {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_owned());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let inner = RealBackend::connect(&address, &username, &password, connect_options_from_env())
        .await
        .expect("observed real backend should connect");
    let lifecycle = Arc::new(LifecycleCounts::default());
    let database = Database::with_backend(
        Box::new(ObservedRealBackend {
            inner,
            lifecycle: Arc::clone(&lifecycle),
            root_scan_barrier,
        }),
        database_name,
    );
    (database, lifecycle)
}

fn selected_person_request(
    registry: &DescriptorRegistry,
    schema: &DynamicCrudSchema,
) -> ValidatedMatchRequest {
    let root = BindingId::new(0);
    validate_match_request(
        registry,
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: root,
                    descriptor: registry.descriptor_id(&schema.person_type).unwrap(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: root }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
    )
    .expect("selected person request should validate")
}

fn selected_person_string_predicate_request(
    registry: &DescriptorRegistry,
    schema: &DynamicCrudSchema,
) -> ValidatedMatchRequest {
    let root = BindingId::new(0);
    let person = registry.descriptor_id(&schema.person_type).unwrap();
    let name = BoundFieldId::new(root, registry.field_id(&person, "name").unwrap());
    validate_match_request(
        registry,
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: root,
                    descriptor: person,
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: Some(MatchExpr::And {
                    expressions: vec![
                        MatchExpr::FieldValue {
                            field: name.clone(),
                            operator: ComparisonOp::StartsWith,
                            value: AttributeValue::String("Al".into()),
                        },
                        MatchExpr::FieldValue {
                            field: name.clone(),
                            operator: ComparisonOp::Contains,
                            value: AttributeValue::String("LIC".into()),
                        },
                        MatchExpr::FieldValue {
                            field: name.clone(),
                            operator: ComparisonOp::EndsWith,
                            value: AttributeValue::String("ice".into()),
                        },
                        MatchExpr::FieldValue {
                            field: name,
                            operator: ComparisonOp::Regex,
                            value: AttributeValue::String("^A.*e$".into()),
                        },
                    ],
                }),
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: root }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        ),
    )
    .expect("selected string-predicate request should validate")
}

fn adversarial_limits() -> Vec<(&'static str, MatchExecutionLimits, &'static str)> {
    let cancellation = AnswerCancellation::default();
    cancellation.cancel();
    vec![
        (
            "pre-cancelled",
            MatchExecutionLimits::tightened(100, 64 * 1024, Duration::from_secs(5), cancellation),
            "provider_cancelled",
        ),
        (
            "zero-deadline",
            MatchExecutionLimits::tightened(
                100,
                64 * 1024,
                Duration::ZERO,
                AnswerCancellation::default(),
            ),
            "transaction_deadline_exceeded",
        ),
        (
            "response-byte-limit",
            MatchExecutionLimits::tightened(
                100,
                1,
                Duration::from_secs(5),
                AnswerCancellation::default(),
            ),
            "response_byte_limit",
        ),
    ]
}

fn assert_resource_error(error: OrmError, expected_code: &str) {
    let OrmError::Match(error) = error else {
        panic!("expected canonical match error, got {error}")
    };
    assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
    assert_eq!(error.code().as_str(), expected_code);
    assert_eq!(
        error.path().segments(),
        &[MatchErrorPathSegment::ProviderEvidence]
    );
}

fn assert_selected_person(result: &ValidatedMatchResult, expected_iid: &str) {
    let MatchResult::Rows { rows } = result.result() else {
        panic!("expected selected rows result")
    };
    assert_eq!(rows.len(), 1);
    let SlotValue::One(person) = &rows[0].slots()[0] else {
        panic!("expected singular selected person")
    };
    assert_eq!(person.concept_id().as_str(), expected_iid);
}

#[tokio::test]
async fn dynamic_entity_transaction_commit_and_rollback_against_typedb() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("tx").await;
    let descriptor = schema.person_descriptor();
    let db_manager = DynamicEntityManager::new(&db, descriptor.clone());

    let rollback_tx = db
        .transaction_context(TxType::Write)
        .await
        .expect("write transaction should open");
    let rollback_manager =
        DynamicEntityManager::with_transaction(rollback_tx.clone(), descriptor.clone());
    rollback_manager
        .insert(&person_attrs("Rollback", 20))
        .await
        .expect("transaction-bound insert should succeed");
    assert_eq!(
        rollback_manager
            .count_with_filters(&[Filter::string_eq("name", "Rollback")])
            .await
            .expect("transaction-bound read should see uncommitted write"),
        1
    );
    rollback_tx
        .rollback()
        .await
        .expect("rollback should succeed");
    assert!(
        db_manager
            .get(&[Filter::string_eq("name", "Rollback")])
            .await
            .expect("post-rollback lookup should succeed")
            .is_empty()
    );

    let commit_tx = db
        .transaction_context(TxType::Write)
        .await
        .expect("write transaction should open");
    let commit_manager = DynamicEntityManager::with_transaction(commit_tx.clone(), descriptor);
    commit_manager
        .insert(&person_attrs("Commit", 21))
        .await
        .expect("transaction-bound insert should succeed");
    commit_tx.commit().await.expect("commit should succeed");

    let committed = db_manager
        .get(&[Filter::string_eq("name", "Commit")])
        .await
        .expect("post-commit lookup should succeed");
    assert_eq!(committed.len(), 1);
}

#[tokio::test]
async fn typed_string_predicates_execute_across_given_and_inline_provider_bands() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("typed-string-parameters").await;
    let manager = DynamicEntityManager::new(&db, schema.person_descriptor());
    let alice_iid = manager
        .insert(&person_attrs("Alice", 33))
        .await
        .expect("typed string-predicate fixture should commit");

    let registry = DescriptorRegistry::new();
    registry
        .register_entity(schema.person_descriptor().as_ref().clone())
        .expect("typed string-predicate descriptor should register");
    let request = selected_person_string_predicate_request(&registry, &schema);

    let given_feature_supported = db.server_version().is_some_and(|server| {
        type_bridge_core_lib::version::check_feature_supported(
            type_bridge_core_lib::version::Feature::GivenStage,
            &server,
        )
        .is_ok()
    });
    assert_eq!(db.supports_given_stage(), given_feature_supported);

    let result = db
        .execute_match(&registry, &request)
        .await
        .expect("typed string predicates should execute on the negotiated provider");
    assert_selected_person(&result, &alice_iid);
}

#[tokio::test]
async fn selected_page_and_total_remain_on_one_read_snapshot_across_a_concurrent_commit() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("selected-snapshot").await;
    let descriptor = schema.person_descriptor();
    let manager = DynamicEntityManager::new(&db, descriptor.clone());
    let selected_iid = manager
        .insert(&person_attrs("BeforeSnapshot", 20))
        .await
        .expect("initial entity should commit");

    let registry = DescriptorRegistry::new();
    registry
        .register_entity(descriptor.as_ref().clone())
        .expect("integration descriptor should register");
    let root = BindingId::new(0);
    let plan = MatchPlan {
        bindings: vec![MatchBinding {
            id: root,
            descriptor: registry.descriptor_id(&schema.person_type).unwrap(),
            thing_kind: ThingKind::Entity,
            match_mode: MatchMode::Exact,
        }],
        predicate: None,
        allowed_cross_joins: BTreeSet::new(),
    };
    let page = validate_match_request(
        &registry,
        MatchRequest::v1(
            plan,
            MatchOperation::PageBy {
                root,
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: root }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 10,
                },
                include_total: true,
            },
        ),
    )
    .expect("page request should validate");

    let root_selected = Arc::new(Notify::new());
    let writer_finished = Arc::new(Notify::new());
    let (barrier_db, lifecycle) = observed_database(
        db.database_name(),
        Some(RootScanBarrier {
            root_selected: Arc::clone(&root_selected),
            writer_finished: Arc::clone(&writer_finished),
        }),
    )
    .await;

    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(
            async {
                barrier_db
                    .execute_match(&registry, &page)
                    .await
                    .expect("barrier page should execute")
            },
            async {
                root_selected.notified().await;
                let update = manager
                    .update(Some(&selected_iid), &person_attrs("BeforeSnapshot", 99))
                    .await;
                writer_finished.notify_one();
                update.expect("inter-stage writer should commit");
            },
        )
    })
    .await
    .expect("inter-stage snapshot barrier should not deadlock");
    let MatchResult::Page { entries, total, .. } = result.result() else {
        panic!("expected page result")
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(*total, Some(1));
    assert_eq!(lifecycle.opens.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.closes.load(Ordering::SeqCst), 1);
    let SlotValue::One(thing) = &entries[0].slots()[0] else {
        panic!("expected one singular hydrated root")
    };
    let snapshot_age = thing
        .attributes()
        .iter()
        .find(|attribute| attribute.field().name == "age")
        .expect("snapshot root should include age");
    assert_eq!(snapshot_age.values(), &[AttributeValue::Long(20)]);

    let fresh = manager
        .get_by_iid(&selected_iid)
        .await
        .expect("fresh read should execute")
        .expect("updated root should still exist");
    assert_eq!(
        attr_value(&fresh, &schema.age_attr),
        Some(&AttributeValue::Long(99))
    );
}

#[tokio::test]
async fn selected_owned_execution_recovers_after_real_resource_failures() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("selected-owned-resource").await;
    let descriptor = schema.person_descriptor();
    let manager = DynamicEntityManager::new(&db, descriptor.clone());
    let selected_iid = manager
        .insert(&person_attrs("OwnedResource", 31))
        .await
        .expect("owned resource fixture should commit");
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(descriptor.as_ref().clone())
        .expect("owned resource descriptor should register");
    let validated = selected_person_request(&registry, &schema);
    let (observed, lifecycle) = observed_database(db.database_name(), None).await;

    for (case, limits, expected_code) in adversarial_limits() {
        let opens_before_failure = lifecycle.opens.load(Ordering::SeqCst);
        let closes_before_failure = lifecycle.closes.load(Ordering::SeqCst);
        let error = match observed
            .execute_match_with_limits(&registry, &validated, limits)
            .await
        {
            Ok(_) => panic!("{case} execution must not expose a partial result"),
            Err(error) => error,
        };
        assert_resource_error(error, expected_code);
        let failure_transactions = usize::from(case == "response-byte-limit");
        assert_eq!(
            lifecycle.opens.load(Ordering::SeqCst),
            opens_before_failure + failure_transactions,
            "{case} opened an unexpected transaction"
        );
        assert_eq!(
            lifecycle.closes.load(Ordering::SeqCst),
            closes_before_failure + failure_transactions,
            "{case} closed an unexpected transaction"
        );

        let fresh = observed
            .execute_match(&registry, &validated)
            .await
            .unwrap_or_else(|error| panic!("fresh execution after {case} must succeed: {error}"));
        assert_selected_person(&fresh, &selected_iid);
    }

    // Pre-cancelled and already-expired operations fail before opening a
    // transaction. The byte-limit failure opens and closes one, and every
    // recovery probe owns one more: 1 + 3 = 4 total.
    assert_eq!(lifecycle.opens.load(Ordering::SeqCst), 4);
    assert_eq!(lifecycle.closes.load(Ordering::SeqCst), 4);
    assert_eq!(lifecycle.commits.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn selected_borrowed_context_survives_real_resource_failures_until_caller_close() {
    let _guard = crate::common::integration_test_guard().await;
    let (db, schema) = setup_dynamic_database("selected-borrowed-resource").await;
    let descriptor = schema.person_descriptor();
    let manager = DynamicEntityManager::new(&db, descriptor.clone());
    let selected_iid = manager
        .insert(&person_attrs("BorrowedResource", 32))
        .await
        .expect("borrowed resource fixture should commit");
    let registry = DescriptorRegistry::new();
    registry
        .register_entity(descriptor.as_ref().clone())
        .expect("borrowed resource descriptor should register");
    let validated = selected_person_request(&registry, &schema);
    let (observed, lifecycle) = observed_database(db.database_name(), None).await;
    let context = observed
        .transaction_context(TxType::Read)
        .await
        .expect("borrowed read context should open");

    for (case, limits, expected_code) in adversarial_limits() {
        let error = match context
            .execute_match_with_limits(&registry, &validated, limits)
            .await
        {
            Ok(_) => panic!("borrowed {case} must not expose a partial result"),
            Err(error) => error,
        };
        assert_resource_error(error, expected_code);

        let fresh = context
            .execute_match(&registry, &validated)
            .await
            .unwrap_or_else(|error| {
                panic!("borrowed context must remain reusable after {case}: {error}")
            });
        assert_selected_person(&fresh, &selected_iid);
    }

    assert_eq!(lifecycle.opens.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.closes.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle.commits.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 0);

    context
        .close()
        .await
        .expect("caller should retain authority to close the borrowed context");
    assert_eq!(lifecycle.closes.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.commits.load(Ordering::SeqCst), 0);
    assert_eq!(lifecycle.rollbacks.load(Ordering::SeqCst), 0);
}
