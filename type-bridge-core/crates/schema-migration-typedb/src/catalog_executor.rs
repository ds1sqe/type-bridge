//! Provider-bound execution of catalog-authorized migration plans.

use std::sync::Arc;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::diagnostic::{DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::Database;
use type_bridge_schema_migration::{
    ExecutionScope, LeaseHolderId, MigrationCatalog, MigrationExecutionControl,
    MigrationExecutionOutcome, MigrationRollbackOutcome, MigrationVerifyReport,
    VerifiedMigrationApplyPlan, VerifiedMigrationRollbackPlan, await_interruptible_operation,
    execute_verified_migration_apply_plan, execute_verified_migration_apply_plan_controlled,
    execute_verified_migration_rollback_plan, execute_verified_migration_rollback_plan_controlled,
    require_authorized_apply_plan, require_authorized_rollback_plan, verify_migration_state,
};

use crate::{
    TypeDbExecutionBinding, TypeDbMigrationProvider, TypeDbMigrationStore,
    VerifiedMigrationCatalog, rebuild_live_managed_state,
};

/// Execute one catalog-authorized forward plan through its exact TypeDB pair.
pub async fn execute_catalog_apply_plan(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationApplyPlan,
) -> Result<MigrationExecutionOutcome, Diagnostic> {
    require_authorized_apply_plan(plan)?;
    let binding = catalog_binding(managed_database, catalog)?;
    let verified =
        VerifiedMigrationCatalog::new(catalog.graph().manifests().map(|(_, value)| value))?;
    let store = TypeDbMigrationStore::new(&binding, verified)?
        .with_catalog_import_binding()
        .await?;
    let store = store.bind_guarded_bridge_plan(plan)?;
    let provider = TypeDbMigrationProvider::new(&binding, plan.schema_authorities())?
        .for_replayed_profile_bridge(catalog.graph(), plan);
    execute_verified_migration_apply_plan(&store, &provider, holder, plan).await
}

/// Execute one catalog-authorized forward plan under explicit shared controls.
pub async fn execute_catalog_apply_plan_controlled(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationApplyPlan,
    control: &MigrationExecutionControl,
) -> Result<MigrationExecutionOutcome, Diagnostic> {
    execute_catalog_apply_plan_controlled_with_user_steps(
        managed_database,
        catalog,
        holder,
        plan,
        control,
        &crate::MigrationUserSteps::default(),
    )
    .await
}

/// Execute a catalog plan with registered process-local user callbacks.
pub async fn execute_catalog_apply_plan_controlled_with_user_steps(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationApplyPlan,
    control: &MigrationExecutionControl,
    user_steps: &crate::MigrationUserSteps,
) -> Result<MigrationExecutionOutcome, Diagnostic> {
    control.check()?;
    require_authorized_apply_plan(plan)?;
    require_apply_group_limit(plan, control)?;
    let binding = catalog_binding(managed_database, catalog)?;
    let verified =
        VerifiedMigrationCatalog::new(catalog.graph().manifests().map(|(_, value)| value))?;
    let store = await_interruptible_operation(
        control,
        TypeDbMigrationStore::new(&binding, verified)?.with_catalog_import_binding(),
    )
    .await?;
    control.check()?;
    let store = store.bind_guarded_bridge_plan(plan)?;
    let provider = TypeDbMigrationProvider::new(&binding, plan.schema_authorities())?
        .for_replayed_profile_bridge(catalog.graph(), plan)
        .with_user_steps(user_steps.clone());
    execute_verified_migration_apply_plan_controlled(&store, &provider, holder, plan, control).await
}

/// Execute one catalog-authorized rollback plan through its exact TypeDB pair.
pub async fn execute_catalog_rollback_plan(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationRollbackPlan,
) -> Result<MigrationRollbackOutcome, Diagnostic> {
    require_authorized_rollback_plan(plan)?;
    let binding = catalog_binding(managed_database, catalog)?;
    let verified =
        VerifiedMigrationCatalog::new(catalog.graph().manifests().map(|(_, value)| value))?;
    let store = TypeDbMigrationStore::new(&binding, verified)?
        .with_catalog_import_binding()
        .await?;
    let store = store.bind_guarded_bridge_rollback_plan(plan)?;
    let provider = TypeDbMigrationProvider::new(&binding, plan.schema_authorities())?;
    execute_verified_migration_rollback_plan(&store, &provider, holder, plan).await
}

/// Execute one catalog-authorized rollback plan under explicit shared controls.
pub async fn execute_catalog_rollback_plan_controlled(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationRollbackPlan,
    control: &MigrationExecutionControl,
) -> Result<MigrationRollbackOutcome, Diagnostic> {
    execute_catalog_rollback_plan_controlled_with_user_steps(
        managed_database,
        catalog,
        holder,
        plan,
        control,
        &crate::MigrationUserSteps::default(),
    )
    .await
}

/// Execute a catalog plan with registered process-local user callbacks.
pub async fn execute_catalog_rollback_plan_controlled_with_user_steps(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationRollbackPlan,
    control: &MigrationExecutionControl,
    user_steps: &crate::MigrationUserSteps,
) -> Result<MigrationRollbackOutcome, Diagnostic> {
    control.check()?;
    require_authorized_rollback_plan(plan)?;
    require_rollback_group_limit(plan, control)?;
    let binding = catalog_binding(managed_database, catalog)?;
    let verified =
        VerifiedMigrationCatalog::new(catalog.graph().manifests().map(|(_, value)| value))?;
    let store = await_interruptible_operation(
        control,
        TypeDbMigrationStore::new(&binding, verified)?.with_catalog_import_binding(),
    )
    .await?;
    control.check()?;
    let store = store.bind_guarded_bridge_rollback_plan(plan)?;
    let provider = TypeDbMigrationProvider::new(&binding, plan.schema_authorities())?
        .with_user_steps(user_steps.clone());
    execute_verified_migration_rollback_plan_controlled(&store, &provider, holder, plan, control)
        .await
}

/// Verify catalog, ledger, and live semantics without setup, lease, or mutation.
pub async fn verify_catalog_state(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
) -> Result<MigrationVerifyReport, Diagnostic> {
    let genesis = catalog
        .graph()
        .topological_order()
        .first()
        .and_then(|id| catalog.graph().manifest(id))
        .map(|manifest| manifest.source_schema())
        .ok_or_else(|| {
            failure(
                "migration_catalog_verify_empty_history",
                "read-only catalog verification requires a replay-verified genesis manifest",
            )
        })?;
    let binding = catalog_binding(managed_database, catalog)?;
    let verified =
        VerifiedMigrationCatalog::new(catalog.graph().manifests().map(|(_, value)| value))?;
    let store = TypeDbMigrationStore::new(&binding, verified)?;
    let scope = ExecutionScope::new(catalog.execution_context()?.scope_id().clone());
    let before = applied_basis(store.load_applied_read_only(&scope).await?);
    let export = binding
        .managed_database()
        .schema_text()
        .await
        .map_err(|_| {
            failure(
                "migration_catalog_verify_export_failed",
                "managed database schema export failed during read-only catalog verification",
            )
        })?;
    let live = rebuild_live_managed_state(
        DocumentId::new("typebridge-catalog-verify-live-export.typeql")?,
        &export,
        genesis,
        catalog.execution_context()?,
    )?;
    let after = applied_basis(store.load_applied_read_only(&scope).await?);
    if before != after {
        return Err(failure(
            "migration_catalog_verify_mixed_authority",
            "applied migration authority changed during read-only catalog verification",
        ));
    }
    verify_migration_state(
        catalog.graph(),
        &before,
        genesis,
        None,
        Some(&live),
        catalog.execution_context()?,
    )
}

/// Load the active applied identities from the exact generated catalog journal.
///
/// This is read-only and does not bootstrap either database or control schema.
pub async fn load_catalog_applied_migrations(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
) -> Result<Vec<MigrationId>, Diagnostic> {
    let binding = catalog_binding(managed_database, catalog)?;
    let verified =
        VerifiedMigrationCatalog::new(catalog.graph().manifests().map(|(_, value)| value))?;
    let store = TypeDbMigrationStore::new(&binding, verified)?;
    let scope = ExecutionScope::new(catalog.execution_context()?.scope_id().clone());
    Ok(applied_basis(store.load_applied_read_only(&scope).await?)
        .into_iter()
        .collect())
}

fn applied_basis(
    entries: Vec<
        type_bridge_schema_migration::JournalEntry<type_bridge_schema_migration::AppliedRecord>,
    >,
) -> std::collections::BTreeSet<MigrationId> {
    entries
        .into_iter()
        .map(|entry| entry.record().migration_id().clone())
        .collect()
}

fn require_apply_group_limit(
    plan: &VerifiedMigrationApplyPlan,
    control: &MigrationExecutionControl,
) -> Result<(), Diagnostic> {
    let groups = plan
        .migrations()
        .iter()
        .try_fold(0usize, |count, migration| {
            count
                .checked_add(migration.transaction_groups().len())
                .and_then(|count| count.checked_add(migration.data_step_indices().len()))
        })
        .ok_or_else(execution_group_limit)?;
    if groups > control.resources().transaction_groups() {
        return Err(execution_group_limit());
    }
    Ok(())
}

fn require_rollback_group_limit(
    plan: &VerifiedMigrationRollbackPlan,
    control: &MigrationExecutionControl,
) -> Result<(), Diagnostic> {
    let groups = plan
        .rollbacks()
        .iter()
        .try_fold(0usize, |count, migration| {
            count.checked_add(migration.operations().len())
        })
        .ok_or_else(execution_group_limit)?;
    if groups > control.resources().transaction_groups() {
        return Err(execution_group_limit());
    }
    Ok(())
}

fn execution_group_limit() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::ResourceLimit,
        DiagnosticCode::new("migration_execution_group_limit")
            .expect("static catalog executor diagnostic code"),
        "migration plan exceeds the transaction-group execution ceiling",
    )
}

fn failure(code: &str, message: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("static catalog executor diagnostic code"),
        message,
    )
}

fn catalog_binding(
    managed_database: Arc<Database>,
    catalog: &MigrationCatalog,
) -> Result<TypeDbExecutionBinding, Diagnostic> {
    let journal_database = Arc::new(managed_database.derived_journal_database());
    TypeDbExecutionBinding::new(
        managed_database,
        journal_database,
        catalog.execution_context()?.clone(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};
    use type_bridge_orm::{Database, OrmError};
    use type_bridge_schema::ManagedDeltaContext;
    use type_bridge_schema_migration::{
        LeaseHolderId, MigrationApplyTarget, MigrationCatalog, MigrationHistoryGraph,
        VerifiedMigrationHistoryBundle, encode_verified_migration_history_bundle,
        migration_runtime_capability_vocabulary,
    };

    use super::{execute_catalog_apply_plan, verify_catalog_state};

    struct NoIoBackend {
        calls: Arc<AtomicUsize>,
    }

    impl DriverBackend for NoIoBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected I/O".into())) })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn catalog_executor_rejects_preview_before_binding_or_provider_io() {
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("catalog-executor-scope").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            migration_runtime_capability_vocabulary().unwrap(),
        );
        let graph = MigrationHistoryGraph::from_verified(std::iter::empty()).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let bytes = encode_verified_migration_history_bundle(&bundle).unwrap();
        let catalog = MigrationCatalog::open(&bytes, &context).unwrap();
        let preview = catalog
            .preview_apply(&BTreeSet::new(), &MigrationApplyTarget::DefaultHead)
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let database = Arc::new(Database::with_backend(
            Box::new(NoIoBackend {
                calls: Arc::clone(&calls),
            }),
            "managed",
        ));

        let error = execute_catalog_apply_plan(
            database,
            &catalog,
            &LeaseHolderId::new("catalog-executor").unwrap(),
            &preview,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "migration_apply_preview_not_executable"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let error = verify_catalog_state(
            Arc::new(Database::with_backend(
                Box::new(NoIoBackend {
                    calls: Arc::clone(&calls),
                }),
                "managed",
            )),
            &catalog,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "migration_catalog_verify_empty_history"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn controlled_catalog_import_preflight_observes_inflight_cancellation() {
        use type_bridge_contract::migration::MigrationId;
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema_migration::{
            LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationReference,
            MigrationExecutionControl, MigrationSafetyPolicy, build_legacy_frontier_bridge,
        };

        struct ReadGuard(Arc<AtomicUsize>);
        impl Drop for ReadGuard {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct HoldingBackend {
            entered: Arc<tokio::sync::Notify>,
            dropped: Arc<AtomicUsize>,
            transactions: Arc<AtomicUsize>,
        }
        impl DriverBackend for HoldingBackend {
            fn is_open(&self) -> bool {
                true
            }
            fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
                Some(type_bridge_core_lib::version::Version::new(3, 12, 3))
            }
            fn open_transaction(
                &self,
                _database: &str,
                _tx_type: TxType,
            ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
                self.transactions.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(OrmError::Connection("unexpected transaction".into())) })
            }
            fn database_exists(&self, _database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
                Box::pin(async move {
                    let _guard = ReadGuard(Arc::clone(&self.dropped));
                    self.entered.notify_one();
                    std::future::pending().await
                })
            }
        }
        let context = ManagedDeltaContext::new(
            ManagedScopeId::new("catalog-cancellation").unwrap(),
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            migration_runtime_capability_vocabulary().unwrap(),
        );
        let schema = type_bridge_schema_compat::typeql_to_declared(
            DocumentId::new("adopted.typeql").unwrap(),
            "define entity person;",
        )
        .unwrap();
        let frontier = vec![LegacyMigrationReference::new(
            MigrationId::new("legacy", "0001_initial").unwrap(),
            LegacyMigrationChecksum::new("11".repeat(8)).unwrap(),
        )];
        let bridge = build_legacy_frontier_bridge(
            MigrationId::new("catalog", "0000_legacy_frontier").unwrap(),
            frontier.clone(),
            LegacyAppliedSetDigest::compute(frontier).unwrap(),
            &schema,
            &context,
        )
        .unwrap();
        let applied = BTreeSet::from([bridge.id().clone()]);
        let graph = MigrationHistoryGraph::from_verified([bridge]).unwrap();
        let bundle = VerifiedMigrationHistoryBundle::from_graph(&graph).unwrap();
        let catalog = MigrationCatalog::open(
            &encode_verified_migration_history_bundle(&bundle).unwrap(),
            &context,
        )
        .unwrap();
        let policy = MigrationSafetyPolicy::default_policy();
        let apply = catalog
            .authorize_apply(&applied, &MigrationApplyTarget::DefaultHead, &policy, &[])
            .unwrap();
        let rollback = catalog
            .authorize_rollback(&applied, &BTreeSet::new(), &policy, &[])
            .unwrap();
        let holder = LeaseHolderId::new("catalog-cancellation").unwrap();
        for reverse in [false, true] {
            let entered = Arc::new(tokio::sync::Notify::new());
            let dropped = Arc::new(AtomicUsize::new(0));
            let transactions = Arc::new(AtomicUsize::new(0));
            let database = Arc::new(Database::with_backend(
                Box::new(HoldingBackend {
                    entered: Arc::clone(&entered),
                    dropped: Arc::clone(&dropped),
                    transactions: Arc::clone(&transactions),
                }),
                "managed",
            ));
            let control = MigrationExecutionControl::default();
            let executing = async {
                if reverse {
                    super::execute_catalog_rollback_plan_controlled(
                        database, &catalog, &holder, &rollback, &control,
                    )
                    .await
                    .map(|_| ())
                } else {
                    super::execute_catalog_apply_plan_controlled(
                        database, &catalog, &holder, &apply, &control,
                    )
                    .await
                    .map(|_| ())
                }
            };
            tokio::pin!(executing);
            tokio::select! {
                result = &mut executing => panic!("preflight returned before pending I/O: {result:?}"),
                () = entered.notified() => {}
            }
            control.cancellation().cancel();
            let error = tokio::time::timeout(std::time::Duration::from_secs(5), &mut executing)
                .await
                .expect("pending import read must observe cancellation")
                .unwrap_err();
            assert_eq!(error.code().as_str(), "migration_execution_cancelled");
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
            assert_eq!(transactions.load(Ordering::SeqCst), 0);
        }
    }
}
