use super::*;
use crate::entity_crud::tests::{COMMIT_SUCCESS, Response, fixture};
use crate::staged::*;
use crate::staged_terminals::type_bridge_matched_pending_result;
use std::sync::atomic::{AtomicUsize, Ordering};
use type_bridge_contract::value::CanonicalString;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps};
use type_bridge_orm::{OrmError, TxType};

fn recording(
    package: &TypeBridgeSchemaPackage,
    rows: Vec<Vec<(OwnsFactId, CanonicalValue)>>,
) -> (TypeBridgePlanBuilder, u32) {
    let mut builder = TypeBridgePlanBuilder::new();
    builder.package = Some(Arc::clone(package.state()));
    let thing = builder
        .core
        .insert(TypeId::new(TypeKind::Entity, "person").unwrap());
    builder.core.bind_model_rows(thing, rows);
    let selection = builder.hold(Selection::Matched).unwrap();
    (builder, selection)
}

#[test]
fn empty_bulk_is_a_commit_barrier_and_obeys_write_admission() {
    for outcome in ["commit", "failure", "rollback", "close", "read"] {
        let fixture = fixture(
            if outcome == "failure" {
                vec![Response::Error]
            } else {
                vec![]
            },
            COMMIT_SUCCESS,
        );
        let (mut builder, selection) = recording(&fixture.package, vec![]);
        let (mut tx, mut diagnostics, mut pending) =
            (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
        // SAFETY: fixtures own live, disjoint inputs and writable owner outputs.
        unsafe {
            assert_eq!(
                type_bridge_database_transaction(
                    &*fixture.database,
                    if outcome == "read" {
                        TYPE_BRIDGE_TRANSACTION_READ
                    } else {
                        TYPE_BRIDGE_TRANSACTION_WRITE
                    },
                    &mut tx,
                    &mut diagnostics
                ),
                TypeBridgeStatus::Ok
            );
            let status = type_bridge_staged_transaction_stage_selection(
                tx,
                &mut builder,
                selection,
                &mut pending,
                &mut diagnostics,
            );
            if outcome == "read" {
                assert_ne!(status, TypeBridgeStatus::Ok);
                assert!(pending.is_null());
                crate::entity_crud::tests::close_diagnostics(&mut diagnostics);
            } else {
                assert_eq!(status, TypeBridgeStatus::Ok);
                let mut value = u64::MAX;
                assert_eq!(
                    type_bridge_matched_pending_result(pending, &mut value),
                    TypeBridgeStatus::InUse
                );
                assert_eq!(value, 0);
                let mut failed = ptr::null_mut();
                match outcome {
                    "commit" => assert_eq!(
                        type_bridge_staged_transaction_commit(tx, &mut diagnostics),
                        TypeBridgeStatus::Ok
                    ),
                    "failure" => {
                        let mut fail = TypeBridgePlanBuilder::new();
                        fail.package = Some(Arc::clone(fixture.package.state()));
                        fail.core
                            .exact(TypeId::new(TypeKind::Entity, "person").unwrap());
                        let count = fail.hold(Selection::Count).unwrap();
                        assert_eq!(
                            type_bridge_staged_transaction_stage_selection(
                                tx,
                                &mut fail,
                                count,
                                &mut failed,
                                &mut diagnostics
                            ),
                            TypeBridgeStatus::Ok
                        );
                        assert_ne!(
                            type_bridge_staged_transaction_commit(tx, &mut diagnostics),
                            TypeBridgeStatus::Ok
                        );
                        crate::entity_crud::tests::close_diagnostics(&mut diagnostics);
                    }
                    "rollback" => assert_eq!(
                        type_bridge_staged_transaction_rollback(tx),
                        TypeBridgeStatus::Ok
                    ),
                    "close" => type_bridge_staged_transaction_close(&mut tx),
                    _ => unreachable!(),
                }
                value = u64::MAX;
                assert_eq!(
                    type_bridge_matched_pending_result(pending, &mut value),
                    if outcome == "commit" {
                        TypeBridgeStatus::Ok
                    } else {
                        TypeBridgeStatus::ExecutionFailed
                    }
                );
                assert_eq!(value, 0);
                type_bridge_pending_free(&mut failed);
            }
            type_bridge_pending_free(&mut pending);
            type_bridge_staged_transaction_close(&mut tx);
        }
        assert_eq!(
            fixture.state.query_count(),
            usize::from(outcome == "failure")
        );
        assert_eq!(fixture.state.commits.load(Ordering::Acquire), 0);
        if outcome != "failure" {
            assert!(fixture.state.opened_transactions().is_empty());
        }
    }
}

struct RefuseSecondChunk {
    admissions: Arc<AtomicUsize>,
    opens: Arc<AtomicUsize>,
}
impl DriverBackend for RefuseSecondChunk {
    fn open_transaction(
        &self,
        _: &str,
        _: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Box::pin(async { Err(OrmError::QueryExecution("unexpected backend open".into())) })
    }
    fn is_open(&self) -> bool {
        true
    }
    fn supports_given_rows(&self) -> bool {
        self.admissions.fetch_add(1, Ordering::Relaxed) == 0
    }
    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        "3.13.6".parse().ok()
    }
}

#[test]
fn later_chunk_admission_failure_leaves_no_unobserved_staged_write() {
    let package = crate::entity_crud::tests::package("atomic_chunks");
    let owner = TypeId::new(TypeKind::Entity, "person").unwrap();
    let field = |name| OwnsFactId::new(owner.clone(), AttributeId::new(name).unwrap()).unwrap();
    let text = |value| CanonicalValue::String(CanonicalString::new(value).unwrap());
    let (mut builder, selection) = recording(
        &package,
        vec![
            vec![(field("identifier"), text("a"))],
            vec![
                (field("identifier"), text("b")),
                (field("department"), text("x")),
            ],
        ],
    );
    assert_eq!(
        AsRef::<[QueryPlan]>::as_ref(builder.finish_selection(selection).unwrap().as_ref()).len(),
        2
    );
    let admissions = Arc::new(AtomicUsize::new(0));
    let opens = Arc::new(AtomicUsize::new(0));
    let db = crate::runtime::test_orm_database(
        Box::new(RefuseSecondChunk {
            admissions: Arc::clone(&admissions),
            opens: Arc::clone(&opens),
        }),
        "atomic",
        None,
    );
    let db =
        crate::runtime::TypeBridgeDatabase::from_test_database(Arc::clone(package.state()), db);
    let (mut tx, mut diagnostics, mut pending) =
        (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
    // SAFETY: all input owners remain live and every output slot is distinct.
    unsafe {
        assert_eq!(
            type_bridge_database_transaction(
                &db,
                TYPE_BRIDGE_TRANSACTION_WRITE,
                &mut tx,
                &mut diagnostics
            ),
            TypeBridgeStatus::Ok
        );
        assert_ne!(
            type_bridge_staged_transaction_stage_selection(
                tx,
                &mut builder,
                selection,
                &mut pending,
                &mut diagnostics
            ),
            TypeBridgeStatus::Ok
        );
        assert!(pending.is_null());
        crate::entity_crud::tests::close_diagnostics(&mut diagnostics);
        assert_eq!(admissions.load(Ordering::Relaxed), 2);
        assert_eq!(
            type_bridge_staged_transaction_commit(tx, &mut diagnostics),
            TypeBridgeStatus::Ok
        );
        assert_eq!(opens.load(Ordering::Relaxed), 0);
        type_bridge_staged_transaction_close(&mut tx);
    }
}

#[test]
fn failed_unbound_selection_does_not_freeze_or_cache_its_failure() {
    let package = crate::entity_crud::tests::package("retry_selection");
    let mut builder = TypeBridgePlanBuilder::new();
    builder
        .core
        .exact(TypeId::new(TypeKind::Entity, "person").unwrap());
    let selection = builder.hold(Selection::Count).unwrap();
    assert_eq!(
        builder
            .finish_selection(selection)
            .err()
            .unwrap()
            .code()
            .as_str(),
        "plan_context_missing"
    );
    assert!(!builder.frozen);
    assert!(
        builder.selections[selection as usize]
            .compiled
            .get()
            .is_none()
    );
    assert_eq!(
        unsafe { type_bridge_plan_builder_exists(&mut builder) },
        TypeBridgeStatus::Ok
    );
    builder.package = Some(Arc::clone(package.state()));
    assert!(builder.finish_selection(selection).is_ok());
    assert!(builder.frozen);
}

#[test]
fn frozen_unbound_raw_cache_refuses_another_package_brand() {
    let first = crate::entity_crud::tests::package("cache_first");
    let second = crate::entity_crud::tests::package_from_source(
        "cache_second",
        "format: typebridge.schema/v2\nentities:\n  other: {}\n",
    );
    for frozen in [false, true] {
        let mut builder = TypeBridgePlanBuilder::new();
        builder
            .core
            .exact(TypeId::new(TypeKind::Entity, "person").unwrap());
        builder.core.count();
        builder.frozen = frozen;
        let (mut plan, mut wrong, mut diagnostics) =
            (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
        // SAFETY: live owners and distinct writable outputs, including failed builds.
        unsafe {
            assert_eq!(
                type_bridge_plan_builder_build(&builder, &first, &mut plan, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert_ne!(
                type_bridge_plan_builder_build(&builder, &second, &mut wrong, &mut diagnostics),
                TypeBridgeStatus::Ok
            );
            assert!(wrong.is_null());
            if frozen {
                assert_eq!(
                    crate::entity_crud::tests::diagnostic(diagnostics).1,
                    "generated_token_package_mismatch"
                );
            }
            crate::entity_crud::tests::close_diagnostics(&mut diagnostics);
            type_bridge_plan_free(&mut plan);
        }
    }
}
