//! TypeDB implementation of the provider-neutral migration execution seam.
//!
//! Fencing model: every provider schema transaction reads the exact managed
//! fence-mirror row first. The pinned TypeDB 3.12.1 exclusion semantics then
//! block a competing takeover until this transaction finishes, so the
//! in-transaction fence recheck immediately before commit satisfies the
//! same-transaction fencing contract rather than an advisory check.

use std::sync::Arc;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration::CONDITIONAL_RESOLUTION_CAPABILITY;
use type_bridge_contract::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{DocumentId, ManagedSchemaState};
use type_bridge_contract::schema_delta::{
    SCHEMA_REDEFINE_CAPABILITY, schema_transition_capability_vocabulary,
};
use type_bridge_orm::migration_assertion::{
    MigrationAssertionExecutionContext, MigrationAssertionExecutionError,
    execute_migration_assertion,
};
use type_bridge_orm::{
    ClassifiedCommitError, CommitFailureCertainty, Database, OrmError, Transaction,
};
use type_bridge_query::ValidatedMigrationAssertionPlan;
use type_bridge_schema::{BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext};
use type_bridge_schema_migration::{
    ExecutionBindingToken, ExecutionFuture, GroupCommitCertainty, GroupCommitFailure,
    GroupCommitFuture, MigrationExecutionProvider, MigrationLease, PreparedMigrationGroup,
    StatementUnit, typedb_3_12_1_profile,
};

use crate::observation::{
    ManagedObservationAuthority, observe_managed_state_from_export_with_authority,
};
use crate::runner::LegacyExecutionBinding;
use crate::store::{require_active_managed_fence, require_migration_database_pair_identity};

const SUPPORTED_SERVER: (u32, u32, u32) = (3, 12, 1);

/// One validated, process-local TypeDB migration execution binding.
///
/// The binding owns the exact managed/journal handles and managed context used
/// by both [`TypeDbMigrationProvider`] and
/// [`crate::TypeDbMigrationStore`]. Its local token is intentionally opaque,
/// non-serializable, and shared only by components created from this value.
/// Clones preserve that local identity. Calling [`Self::new`] again, even with
/// identical arguments, creates a different identity whose leases are rejected.
#[derive(Clone)]
pub struct TypeDbExecutionBinding {
    managed_database: Arc<Database>,
    journal_database: Arc<Database>,
    context: ManagedDeltaContext,
    local_token: ExecutionBindingToken,
}

impl TypeDbExecutionBinding {
    /// Validate and bind one exact TypeDB 3.12.1 database pair and context.
    ///
    /// Reuse this value (or a clone) for every provider and store that must
    /// participate in one execution. Each successful call creates a fresh
    /// process-local lease identity without changing persisted journal records.
    pub fn new(
        managed_database: Arc<Database>,
        journal_database: Arc<Database>,
        context: ManagedDeltaContext,
    ) -> Result<Self, Diagnostic> {
        require_supported_migration_execution_binding(
            &managed_database,
            &journal_database,
            &context,
        )?;
        Ok(Self::new_unchecked(
            managed_database,
            journal_database,
            context,
        ))
    }

    pub(crate) fn new_unchecked(
        managed_database: Arc<Database>,
        journal_database: Arc<Database>,
        context: ManagedDeltaContext,
    ) -> Self {
        Self {
            managed_database,
            journal_database,
            context,
            local_token: ExecutionBindingToken::fresh(),
        }
    }

    pub(crate) fn require_supported(&self) -> Result<(), Diagnostic> {
        require_supported_migration_execution_binding(
            &self.managed_database,
            &self.journal_database,
            &self.context,
        )
    }

    pub(crate) fn require_lease(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        require_lease_binding(&self.local_token, lease)
    }

    pub(crate) fn managed_database(&self) -> &Arc<Database> {
        &self.managed_database
    }

    pub(crate) fn journal_database(&self) -> &Arc<Database> {
        &self.journal_database
    }

    pub(crate) const fn context(&self) -> &ManagedDeltaContext {
        &self.context
    }

    pub(crate) fn local_token(&self) -> &ExecutionBindingToken {
        &self.local_token
    }
}

fn require_lease_binding(
    expected: &ExecutionBindingToken,
    lease: &MigrationLease,
) -> Result<(), Diagnostic> {
    if lease.is_bound_to(expected) {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_execution_binding_mismatch",
            "migration lease belongs to a different TypeDB execution binding",
        ))
    }
}

/// TypeDB 3.12.1 execution provider bound to one managed/journal pair and context.
///
/// Every execution method rejects unbound leases and leases issued under a
/// different [`TypeDbExecutionBinding`] before opening a TypeDB transaction.
pub struct TypeDbMigrationProvider {
    binding: TypeDbExecutionBinding,
    legacy_binding: Option<(LegacyExecutionBinding, String)>,
    observation_authority: ManagedObservationAuthority,
}

impl TypeDbMigrationProvider {
    /// Construct a provider from one shared exact TypeDB execution binding.
    ///
    /// Pass the same binding used to construct the cooperating
    /// [`crate::TypeDbMigrationStore`].
    pub fn new(binding: &TypeDbExecutionBinding) -> Result<Self, Diagnostic> {
        binding.require_supported()?;
        Ok(Self {
            binding: binding.clone(),
            legacy_binding: None,
            observation_authority: ManagedObservationAuthority::ExactPortable,
        })
    }

    /// Bind the runner-validated legacy pair state into every managed SCHEMA
    /// transaction opened by this provider.
    pub(crate) fn new_with_legacy_binding(
        binding: &TypeDbExecutionBinding,
        legacy_binding: LegacyExecutionBinding,
        managed_scope: String,
        observation_authority: ManagedObservationAuthority,
    ) -> Result<Self, Diagnostic> {
        let mut provider = Self::new(binding)?;
        provider.legacy_binding = Some((legacy_binding, managed_scope));
        provider.observation_authority = observation_authority;
        Ok(provider)
    }

    pub(crate) fn new_with_observation_authority(
        binding: &TypeDbExecutionBinding,
        observation_authority: ManagedObservationAuthority,
    ) -> Result<Self, Diagnostic> {
        let mut provider = Self::new(binding)?;
        provider.observation_authority = observation_authority;
        Ok(provider)
    }

    async fn observe_in_transaction(
        &self,
        transaction: &mut Transaction,
        lease: &MigrationLease,
        source_candidate: &ManagedSchemaState,
        target_candidate: &ManagedSchemaState,
    ) -> Result<ManagedSchemaState, Diagnostic> {
        require_active_managed_fence(transaction, lease).await?;
        if let Some((binding, managed_scope)) = &self.legacy_binding {
            binding
                .validate_contents(transaction, managed_scope)
                .await?;
        }
        let export = self
            .binding
            .managed_database
            .schema_text()
            .await
            .map_err(map_orm_error)?;
        require_active_managed_fence(transaction, lease).await?;
        if let Some((binding, managed_scope)) = &self.legacy_binding {
            binding
                .validate_contents(transaction, managed_scope)
                .await?;
        }
        observe_managed_state_from_export_with_authority(
            observation_document()?,
            &export,
            self.binding.context.available_capabilities(),
            source_candidate,
            target_candidate,
            &self.observation_authority,
        )
    }
}

impl MigrationExecutionProvider for TypeDbMigrationProvider {
    fn available_capabilities(&self) -> &CapabilitySet {
        self.binding.context.available_capabilities()
    }

    fn observe_managed_state<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source_candidate: &'a ManagedSchemaState,
        target_candidate: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, ManagedSchemaState> {
        Box::pin(async move {
            self.binding.require_lease(lease)?;
            require_managed_state_execution_context(
                source_candidate,
                &self.binding.context,
                "observation_source",
            )?;
            require_managed_state_execution_context(
                target_candidate,
                &self.binding.context,
                "observation_target",
            )?;
            let mut transaction = self
                .binding
                .managed_database
                .schema_transaction()
                .await
                .map_err(map_orm_error)?;
            let observed = self
                .observe_in_transaction(&mut transaction, lease, source_candidate, target_candidate)
                .await;
            finish_provider_schema_guard(&mut transaction, observed, "managed-state observation")
                .await
        })
    }

    fn prepare_group<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source: &'a ManagedSchemaState,
        target: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, Box<dyn PreparedMigrationGroup + 'a>> {
        Box::pin(async move {
            self.binding.require_lease(lease)?;
            require_managed_state_execution_context(source, &self.binding.context, "group_source")?;
            require_managed_state_execution_context(target, &self.binding.context, "group_target")?;
            let mut transaction = self
                .binding
                .managed_database
                .schema_transaction()
                .await
                .map_err(map_orm_error)?;
            let observed = self
                .observe_in_transaction(&mut transaction, lease, source, target)
                .await;
            let observed = match observed {
                Ok(observed) => observed,
                Err(error) => {
                    return finish_provider_schema_guard(
                        &mut transaction,
                        Err(error),
                        "transaction-group source observation",
                    )
                    .await;
                }
            };
            if observed != *source {
                let primary = failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_prepare_source_mismatch",
                    "live managed state is not the exact transaction-group source",
                );
                return finish_provider_schema_guard(
                    &mut transaction,
                    Err(primary),
                    "transaction-group source mismatch",
                )
                .await;
            }
            Ok(Box::new(TypeDbPreparedGroup {
                transaction,
                capabilities: self.binding.context.available_capabilities(),
                source: observed,
                local_token: self.binding.local_token.clone(),
            }) as Box<dyn PreparedMigrationGroup + 'a>)
        })
    }
}

struct TypeDbPreparedGroup<'a> {
    transaction: Transaction,
    capabilities: &'a CapabilitySet,
    source: ManagedSchemaState,
    local_token: ExecutionBindingToken,
}

impl PreparedMigrationGroup for TypeDbPreparedGroup<'_> {
    fn execute_assertion<'a>(
        &'a mut self,
        plan: &'a ValidatedMigrationAssertionPlan,
    ) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            execute_migration_assertion(
                &mut self.transaction,
                plan,
                MigrationAssertionExecutionContext::new(
                    &self.source,
                    self.capabilities,
                    StructuralLimits::CANONICAL,
                ),
            )
            .await
            .map_err(map_assertion_error)
        })
    }

    fn execute_statement_unit<'a>(
        &'a mut self,
        unit: &'a StatementUnit,
    ) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            for statement in unit.statements() {
                self.transaction
                    .query(statement.query())
                    .await
                    .map_err(map_orm_error)?;
            }
            Ok(())
        })
    }

    fn commit<'a>(self: Box<Self>, lease: &'a MigrationLease) -> GroupCommitFuture<'a>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let mut this = *self;
            if let Err(error) = require_lease_binding(&this.local_token, lease) {
                let error = finish_provider_schema_guard::<()>(
                    &mut this.transaction,
                    Err(error),
                    "transaction-group execution binding",
                )
                .await
                .expect_err("an error result remains an error after cleanup");
                return Err(GroupCommitFailure::new(
                    GroupCommitCertainty::DefinitelyAborted,
                    error,
                ));
            }
            if let Err(error) = require_active_managed_fence(&mut this.transaction, lease).await {
                let error = finish_provider_schema_guard::<()>(
                    &mut this.transaction,
                    Err(error),
                    "transaction-group pre-commit fence",
                )
                .await
                .expect_err("an error result remains an error after cleanup");
                return Err(GroupCommitFailure::new(
                    GroupCommitCertainty::DefinitelyAborted,
                    error,
                ));
            }
            this.transaction.commit_classified().await.map_err(|error| {
                let certainty = group_commit_certainty(&error);
                GroupCommitFailure::new(certainty, map_orm_error(error.into_orm_error()))
            })
        })
    }

    fn rollback<'a>(self: Box<Self>) -> ExecutionFuture<'a, ()>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let mut this = *self;
            this.transaction.rollback().await.map_err(map_orm_error)
        })
    }
}

fn group_commit_certainty(error: &ClassifiedCommitError) -> GroupCommitCertainty {
    match error.commit_failure_certainty() {
        Some(CommitFailureCertainty::DefinitelyAborted) => GroupCommitCertainty::DefinitelyAborted,
        Some(CommitFailureCertainty::Unknown) | None => GroupCommitCertainty::Unknown,
    }
}

/// Compose the exact execution capability vocabulary for TypeDB 3.12.1.
pub fn execution_capability_vocabulary() -> Result<CapabilitySet, Diagnostic> {
    let mut capabilities = schema_transition_capability_vocabulary();
    for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
        capabilities.insert(CapabilityId::new(*capability)?);
    }
    for capability in migration_assertion_capability_vocabulary().iter().cloned() {
        capabilities.insert(capability);
    }
    capabilities.insert(CapabilityId::new(SCHEMA_REDEFINE_CAPABILITY)?);
    capabilities.insert(CapabilityId::new(CONDITIONAL_RESOLUTION_CAPABILITY)?);
    Ok(capabilities)
}

/// Require the exact TypeDB server version supported by migration execution.
///
/// This check reads only the version negotiated by the existing connection;
/// it performs no database lookup, creation, transaction, or schema work.
pub fn require_supported_migration_server(database: &Database) -> Result<(), Diagnostic> {
    let version = database.server_version();
    require_supported_server_version(
        version.map(|version| (version.major, version.minor, version.patch)),
    )
}

/// Require the exact semantic profile and negotiated servers supported by
/// TypeDB migration execution over one managed/journal pair.
///
/// The profile gate runs first, followed by pair identity, managed-server
/// identity, and journal-server identity. Every check consumes only
/// already-resolved in-memory identity. This function performs no database
/// lookup, creation, transaction, schema, or journal work.
pub fn require_supported_migration_execution_binding(
    managed_database: &Database,
    journal_database: &Database,
    context: &ManagedDeltaContext,
) -> Result<(), Diagnostic> {
    let supported = &typedb_3_12_1_profile().semantic_profile;
    if context.semantic_profile() != supported {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_semantic_profile_unsupported",
            "migration execution requires exactly the TypeDB 3.12.1 semantic profile",
        )
        .with_detail(
            "semantic_profile",
            context.semantic_profile().as_str().to_owned(),
        )
        .with_detail("expected_semantic_profile", supported.as_str().to_owned()));
    }
    require_migration_database_pair_identity(managed_database, journal_database)?;
    require_supported_pair_server_versions(
        negotiated_server_version(managed_database),
        negotiated_server_version(journal_database),
    )?;
    context
        .available_capabilities()
        .ensure_supported_by(&execution_capability_vocabulary()?)
}

/// Require one managed state to belong to the provider's exact execution context.
///
/// The check is pure and is repeated at every public provider observation and
/// transaction-group boundary so a caller cannot relabel a plan produced for a
/// different scope or semantic profile by supplying a supported constructor
/// context.
pub(crate) fn require_managed_state_execution_context(
    state: &ManagedSchemaState,
    context: &ManagedDeltaContext,
    state_role: &'static str,
) -> Result<(), Diagnostic> {
    if state.scope().id() != context.scope_id() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_state_scope_mismatch",
            "managed migration state differs from the provider execution scope",
        )
        .with_detail("state_role", state_role)
        .with_detail("state_scope", state.scope().id().as_str().to_owned())
        .with_detail("execution_scope", context.scope_id().as_str().to_owned()));
    }
    let state_profile = state
        .managed_semantic_schema()
        .as_fingerprint()
        .semantic_profile();
    if state_profile != Some(context.semantic_profile()) {
        let mut error = failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_state_semantic_profile_mismatch",
            "managed migration state differs from the provider semantic profile",
        )
        .with_detail("state_role", state_role)
        .with_detail(
            "execution_semantic_profile",
            context.semantic_profile().as_str().to_owned(),
        );
        if let Some(state_profile) = state_profile {
            error = error.with_detail("state_semantic_profile", state_profile.as_str().to_owned());
        }
        return Err(error);
    }
    state
        .required_capabilities()
        .ensure_supported_by(context.available_capabilities())
        .map_err(|error| error.with_detail("state_role", state_role))
}

fn negotiated_server_version(database: &Database) -> Option<(u32, u32, u32)> {
    database
        .server_version()
        .map(|version| (version.major, version.minor, version.patch))
}

fn require_supported_pair_server_versions(
    managed_version: Option<(u32, u32, u32)>,
    journal_version: Option<(u32, u32, u32)>,
) -> Result<(), Diagnostic> {
    require_supported_server_version(managed_version)
        .map_err(|error| error.with_detail("database_role", "managed"))?;
    require_supported_server_version(journal_version)
        .map_err(|error| error.with_detail("database_role", "journal"))
}

fn require_supported_server_version(version: Option<(u32, u32, u32)>) -> Result<(), Diagnostic> {
    let version = version.ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_server_version_unknown",
            "migration execution requires a negotiated TypeDB server version",
        )
    })?;
    if version != SUPPORTED_SERVER {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_server_version_unsupported",
            "migration execution requires exactly TypeDB 3.12.1",
        )
        .with_detail(
            "server_version",
            format!("{}.{}.{}", version.0, version.1, version.2),
        ));
    }
    Ok(())
}

fn observation_document() -> Result<DocumentId, Diagnostic> {
    DocumentId::new("typebridge-provider-observation.typeql")
}

fn map_assertion_error(error: MigrationAssertionExecutionError) -> Diagnostic {
    if let Some(diagnostic) = error.diagnostic() {
        return diagnostic.clone();
    }
    failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_assertion_failed",
        "migration assertion execution failed against live provider data",
    )
    .with_detail("assertion", error.to_string())
}

fn map_orm_error(error: OrmError) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "migration_typedb_provider_error",
        "TypeDB migration execution provider operation failed",
    )
    .with_detail("provider", error.to_string())
}

async fn finish_provider_schema_guard<T>(
    transaction: &mut Transaction,
    primary: Result<T, Diagnostic>,
    operation: &'static str,
) -> Result<T, Diagnostic> {
    match (primary, transaction.rollback().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(provider_schema_cleanup_failure(cleanup, None, operation)),
        (Err(primary), Err(cleanup)) => Err(provider_schema_cleanup_failure(
            cleanup,
            Some(&primary),
            operation,
        )),
    }
}

fn provider_schema_cleanup_failure(
    cleanup: OrmError,
    primary: Option<&Diagnostic>,
    operation: &'static str,
) -> Diagnostic {
    let mut diagnostic = failure(
        DiagnosticCategory::Integrity,
        "migration_typedb_schema_guard_cleanup_uncertain",
        "provider schema transaction termination was not acknowledged",
    )
    .with_detail("operation", operation)
    .with_detail("cleanup", cleanup.to_string());
    if let Some(primary) = primary {
        diagnostic = diagnostic
            .with_detail("primary_code", primary.code().as_str().to_owned())
            .with_detail("primary", primary.to_string());
    }
    diagnostic
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_orm::DatabaseConnectionAuthority;
    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};
    use type_bridge_schema_migration::typedb_3_12_1_profile;

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
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn is_open(&self) -> bool {
            self.calls.fetch_add(1, Ordering::SeqCst);
            true
        }

        fn close_connection(&self) -> Result<(), OrmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(OrmError::Connection("unexpected backend I/O".to_owned()))
        }

        fn database_exists(&self, _database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn create_database(&self, _database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn delete_database(&self, _database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(OrmError::Connection("unexpected backend I/O".to_owned())) })
        }
    }

    #[test]
    fn migration_server_gate_is_exact_and_stable() {
        let unknown = require_supported_server_version(None).expect_err("unknown must reject");
        assert_eq!(
            unknown.code().as_str(),
            "migration_typedb_server_version_unknown"
        );

        for version in [(3, 11, 5), (3, 12, 0), (3, 12, 2)] {
            let error = require_supported_server_version(Some(version))
                .expect_err("every non-exact server must reject");
            assert_eq!(
                error.code().as_str(),
                "migration_typedb_server_version_unsupported"
            );
        }
        require_supported_server_version(Some((3, 12, 1)))
            .expect("only the exact migration server is supported");

        let managed = require_supported_pair_server_versions(None, Some((3, 12, 1)))
            .expect_err("managed identity is checked first");
        assert_eq!(
            managed.details().get("database_role"),
            Some(
                &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(
                    "managed".to_owned()
                )
            )
        );
        let journal = require_supported_pair_server_versions(Some((3, 12, 1)), Some((3, 11, 5)))
            .expect_err("journal identity is checked after the managed identity");
        assert_eq!(
            journal.details().get("database_role"),
            Some(
                &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(
                    "journal".to_owned()
                )
            )
        );
        require_supported_pair_server_versions(Some((3, 12, 1)), Some((3, 12, 1)))
            .expect("both pair members carry the exact negotiated version");
    }

    #[test]
    fn migration_server_gate_performs_no_database_io() {
        let calls = Arc::new(AtomicUsize::new(0));
        let database = Database::with_backend(
            Box::new(NoIoBackend {
                calls: Arc::clone(&calls),
            }),
            "missing",
        );
        let error = require_supported_migration_server(&database)
            .expect_err("a backend without version identity must reject");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_server_version_unknown"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn migration_execution_binding_gates_profile_before_server_without_database_io() {
        let calls = Arc::new(AtomicUsize::new(0));
        let authority = DatabaseConnectionAuthority::isolated();
        let database = Database::with_backend_authority(
            Box::new(NoIoBackend {
                calls: Arc::clone(&calls),
            }),
            "missing",
            authority.clone(),
        );
        let journal = Database::with_backend_authority(
            Box::new(NoIoBackend {
                calls: Arc::clone(&calls),
            }),
            "missing__tbv2_journal",
            authority,
        );
        let unsupported =
            SemanticProfileId::new("typedb-3.11.5/v1").expect("valid unsupported profile");
        let unsupported_context = ManagedDeltaContext::new(
            ManagedScopeId::new("provider-binding-gate").expect("managed scope"),
            unsupported,
            CapabilitySet::new(),
        );

        let profile_error = require_supported_migration_execution_binding(
            &database,
            &journal,
            &unsupported_context,
        )
        .expect_err("an unsupported semantic profile must reject first");
        assert_eq!(
            profile_error.code().as_str(),
            "migration_typedb_semantic_profile_unsupported"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let supported_context = ManagedDeltaContext::new(
            ManagedScopeId::new("provider-binding-gate").expect("managed scope"),
            typedb_3_12_1_profile().semantic_profile.clone(),
            CapabilitySet::new(),
        );
        let server_error =
            require_supported_migration_execution_binding(&database, &journal, &supported_context)
                .expect_err("an exact profile still requires negotiated server identity");
        assert_eq!(
            server_error.code().as_str(),
            "migration_typedb_server_version_unknown"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn execution_capability_vocabulary_is_a_closed_superset() {
        let capabilities = execution_capability_vocabulary().expect("capability vocabulary");
        let authority_capabilities = type_bridge_schema::schema_authority_capability_vocabulary();
        for capability in typedb_3_12_1_profile().required_capabilities.iter() {
            assert!(capabilities.contains(capability));
        }
        for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
            let id = CapabilityId::new(*capability).expect("builtin capability");
            assert!(capabilities.contains(&id));
        }
        for capability in migration_assertion_capability_vocabulary().iter() {
            assert!(capabilities.contains(capability));
        }
        for extra in [
            SCHEMA_REDEFINE_CAPABILITY,
            CONDITIONAL_RESOLUTION_CAPABILITY,
        ] {
            let id = CapabilityId::new(extra).expect("extra capability");
            assert!(capabilities.contains(&id));
        }
        let expected: std::collections::BTreeSet<String> = typedb_3_12_1_profile()
            .required_capabilities
            .iter()
            .map(ToString::to_string)
            .chain(
                BUILTIN_SCHEMA_CAPABILITY_IDS
                    .iter()
                    .map(ToString::to_string),
            )
            .chain(
                migration_assertion_capability_vocabulary()
                    .iter()
                    .map(ToString::to_string),
            )
            .chain(
                [
                    SCHEMA_REDEFINE_CAPABILITY,
                    CONDITIONAL_RESOLUTION_CAPABILITY,
                ]
                .iter()
                .map(ToString::to_string),
            )
            .collect();
        let actual: std::collections::BTreeSet<String> =
            capabilities.iter().map(ToString::to_string).collect();
        assert_eq!(actual, expected);
        capabilities
            .ensure_supported_by(&authority_capabilities)
            .expect("schema authority consumers understand every workspace execution requirement");
    }

    #[test]
    fn classified_commit_evidence_maps_without_widening_legacy_orm_errors() {
        let aborted = ClassifiedCommitError::Driver {
            certainty: CommitFailureCertainty::DefinitelyAborted,
            message: "server rejected commit".to_owned(),
        };
        assert_eq!(
            group_commit_certainty(&aborted),
            GroupCommitCertainty::DefinitelyAborted,
        );
        assert!(matches!(
            aborted.into_orm_error(),
            OrmError::Transaction(message) if message == "Commit failed: server rejected commit"
        ));

        let unknown = ClassifiedCommitError::Driver {
            certainty: CommitFailureCertainty::Unknown,
            message: "connection dropped".to_owned(),
        };
        assert_eq!(
            group_commit_certainty(&unknown),
            GroupCommitCertainty::Unknown,
        );
        let lifecycle = ClassifiedCommitError::from(OrmError::Transaction("consumed".to_owned()));
        assert_eq!(
            group_commit_certainty(&lifecycle),
            GroupCommitCertainty::Unknown,
        );
    }
}
