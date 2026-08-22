//! Provider-bound execution of catalog-authorized migration plans.

use std::sync::Arc;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_orm::Database;
use type_bridge_schema_migration::{
    LeaseHolderId, MigrationCatalog, MigrationExecutionOutcome, MigrationRollbackOutcome,
    VerifiedMigrationApplyPlan, VerifiedMigrationRollbackPlan,
    execute_verified_migration_apply_plan, execute_verified_migration_rollback_plan,
    require_authorized_apply_plan, require_authorized_rollback_plan,
};

use crate::{
    TypeDbExecutionBinding, TypeDbMigrationProvider, TypeDbMigrationStore, VerifiedMigrationCatalog,
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
    let store = TypeDbMigrationStore::new(&binding, verified)?;
    store.ensure_control_schema().await?;
    let store = store.bind_plan(plan)?;
    let provider = TypeDbMigrationProvider::new(&binding)?;
    execute_verified_migration_apply_plan(&store, &provider, holder, plan).await
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
    let store = TypeDbMigrationStore::new(&binding, verified)?;
    store.ensure_control_schema().await?;
    let store = store.bind_rollback_plan(plan)?;
    let provider = TypeDbMigrationProvider::new(&binding)?;
    execute_verified_migration_rollback_plan(&store, &provider, holder, plan).await
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

    use super::execute_catalog_apply_plan;

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
    }
}
