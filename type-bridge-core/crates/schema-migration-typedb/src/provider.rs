//! TypeDB implementation of the provider-neutral migration execution seam.
//!
//! Fencing model: every provider schema transaction reads the exact managed
//! fence-mirror row first. The pinned TypeDB 3.12.1 exclusion semantics then
//! block a competing takeover until this transaction finishes, so the
//! in-transaction fence recheck immediately before commit satisfies the
//! same-transaction fencing contract rather than an advisory check.

use std::sync::Arc;

#[cfg(test)]
use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_backfill::AttributeBackfillPlan;
#[cfg(test)]
use type_bridge_contract::migration_backfill::COPY_ATTRIBUTE_BACKFILL_CAPABILITY;
use type_bridge_contract::schema::{DocumentId, ManagedSchemaState};
use type_bridge_core_lib::version::{GeneratedPackageRelease, Version};
use type_bridge_orm::migration_assertion::{
    MigrationAssertionExecutionContext, MigrationAssertionExecutionError,
    execute_migration_assertion,
};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{
    ClassifiedCommitError, CommitFailureCertainty, Database, OrmError,
    OwnedTransaction as Transaction,
};
use type_bridge_query::ValidatedMigrationAssertionPlan;
use type_bridge_schema::ManagedDeltaContext;
use type_bridge_schema_migration::{
    BackfillCompletionEvidence, BackfillExecutionCounts, BackfillExecutionDirection,
    BackfillExecutionFuture, BackfillRecoveryObservation, ExecutionBindingToken, ExecutionFuture,
    GroupCommitCertainty, GroupCommitFailure, GroupCommitFuture, MigrationExecutionProvider,
    MigrationHistoryGraph, MigrationLease, MigrationProfile, PreparedMigrationGroup, StatementUnit,
    VerifiedMigrationApplyPlan, migration_runtime_capability_vocabulary,
};

use crate::backfill::LoweredBackfill;
use crate::observation::{
    ManagedObservationAuthority, observe_managed_state_from_export_with_authority,
};
use crate::runner::LegacyExecutionBinding;
use crate::store::{require_active_managed_fence, require_migration_database_pair_identity};

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
    /// Validate and bind one database pair on the exact TypeDB release the
    /// context's semantic profile requires (see [`migration_server_for_profile`]).
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
    profile_bridge_source: Option<ManagedDeltaContext>,
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
            profile_bridge_source: None,
        })
    }

    /// Also admit managed states on the older profile a profile bridge moves
    /// from, for a plan that replays the bridge's ancestors into a database
    /// on the newer server.
    ///
    /// Callers apply this only after the store bound the plan, which admits
    /// an older-profile manifest only as an ancestor of the catalog's single
    /// bridge.
    /// The server pairing still follows the binding's own profile.
    pub(crate) fn for_replayed_profile_bridge(
        mut self,
        graph: &MigrationHistoryGraph,
        plan: &VerifiedMigrationApplyPlan,
    ) -> Self {
        let context = &self.binding.context;
        self.profile_bridge_source =
            replayed_profile_bridge_source(graph, plan, context).map(|from| {
                ManagedDeltaContext::new(
                    context.scope_id().clone(),
                    from,
                    context.available_capabilities().clone(),
                )
            });
        self
    }

    /// Require one state on the binding's context, or on the admitted
    /// profile-bridge source context when it carries that older profile.
    fn require_execution_state(
        &self,
        state: &ManagedSchemaState,
        state_role: &'static str,
    ) -> Result<(), Diagnostic> {
        let state_profile = state
            .managed_semantic_schema()
            .as_fingerprint()
            .semantic_profile();
        let context = match &self.profile_bridge_source {
            Some(source) if state_profile == Some(source.semantic_profile()) => source,
            _ => &self.binding.context,
        };
        require_managed_state_execution_context(state, context, state_role)
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

    fn supports_backfill_execution(&self) -> bool {
        true
    }

    fn observe_backfill<'a>(
        &'a self,
        lease: &'a MigrationLease,
        plan: &'a AttributeBackfillPlan,
        direction: BackfillExecutionDirection,
    ) -> ExecutionFuture<'a, BackfillRecoveryObservation> {
        Box::pin(async move {
            self.binding.require_lease(lease)?;
            plan.required_capabilities()
                .ensure_supported_by(self.available_capabilities())?;
            let lowered = LoweredBackfill::new(plan, direction);
            let mut transaction = self
                .binding
                .managed_database
                .write_transaction()
                .await
                .map_err(map_orm_error)?;
            let observed = async {
                require_active_managed_fence(&mut transaction, lease).await?;
                require_no_backfill_conflict(&mut transaction, &lowered).await?;
                let incomplete = query_has_answers(
                    &mut transaction,
                    &lowered
                        .incomplete_witness()
                        .map_err(backfill_lowering_error)?,
                )
                .await?;
                require_active_managed_fence(&mut transaction, lease).await?;
                if incomplete {
                    Ok(BackfillRecoveryObservation::Incomplete)
                } else {
                    Ok(BackfillRecoveryObservation::Complete(
                        BackfillCompletionEvidence::new(
                            plan.fingerprint()?,
                            direction,
                            BackfillExecutionCounts::new(0, 0, 0, 1)?,
                        ),
                    ))
                }
            }
            .await;
            finish_provider_data_guard(&mut transaction, observed, "backfill observation").await
        })
    }

    fn execute_backfill<'a>(
        &'a self,
        lease: &'a MigrationLease,
        plan: &'a AttributeBackfillPlan,
        direction: BackfillExecutionDirection,
        control: &'a type_bridge_schema_migration::MigrationExecutionControl,
    ) -> BackfillExecutionFuture<'a> {
        Box::pin(async move {
            self.binding
                .require_lease(lease)
                .map_err(definitely_aborted)?;
            plan.required_capabilities()
                .ensure_supported_by(self.available_capabilities())
                .map_err(definitely_aborted)?;
            let plan_fingerprint = plan.fingerprint().map_err(definitely_aborted)?;
            let lowered = LoweredBackfill::new(plan, direction);
            let mut changed = 0_u64;
            let mut transaction_groups = 0_u32;
            loop {
                control
                    .check()
                    .map_err(|error| backfill_failure(transaction_groups, error))?;
                let mut transaction = self
                    .binding
                    .managed_database
                    .write_transaction()
                    .await
                    .map_err(map_orm_error)
                    .map_err(|error| backfill_failure(transaction_groups, error))?;
                let mutation = async {
                    require_active_managed_fence(&mut transaction, lease).await?;
                    require_no_backfill_conflict(&mut transaction, &lowered).await?;
                    let affected = query_answer_count(
                        &mut transaction,
                        &lowered.mutation_group().map_err(backfill_lowering_error)?,
                    )
                    .await?;
                    if affected > u64::from(plan.partition().batch_rows()) {
                        return Err(failure(
                            DiagnosticCategory::Integrity,
                            "migration_typedb_backfill_batch_overflow",
                            "TypeDB returned more backfill answers than the canonical batch bound",
                        ));
                    }
                    require_active_managed_fence(&mut transaction, lease).await?;
                    Ok(affected)
                }
                .await;
                let affected = match mutation {
                    Ok(0) => {
                        finish_provider_data_guard(
                            &mut transaction,
                            Ok(()),
                            "empty backfill transaction group",
                        )
                        .await
                        .map_err(|error| backfill_failure(transaction_groups, error))?;
                        break;
                    }
                    Ok(affected) => affected,
                    Err(error) => {
                        let error = finish_provider_data_guard::<()>(
                            &mut transaction,
                            Err(error),
                            "backfill transaction group",
                        )
                        .await
                        .expect_err("an error remains an error after cleanup");
                        return Err(backfill_failure(transaction_groups, error));
                    }
                };
                transaction.commit_classified().await.map_err(|error| {
                    let certainty = if transaction_groups == 0 {
                        group_commit_certainty(&error)
                    } else {
                        GroupCommitCertainty::Unknown
                    };
                    GroupCommitFailure::new(certainty, map_orm_error(error.into_orm_error()))
                })?;
                changed = changed.checked_add(affected).ok_or_else(|| {
                    backfill_failure(
                        transaction_groups.saturating_add(1),
                        failure(
                            DiagnosticCategory::ResourceLimit,
                            "migration_typedb_backfill_count_overflow",
                            "backfill changed-row count exceeded the supported range",
                        ),
                    )
                })?;
                transaction_groups = transaction_groups.checked_add(1).ok_or_else(|| {
                    backfill_failure(
                        transaction_groups.saturating_add(1),
                        failure(
                            DiagnosticCategory::ResourceLimit,
                            "migration_typedb_backfill_group_count_overflow",
                            "backfill transaction-group count exceeded the supported range",
                        ),
                    )
                })?;
                control
                    .check()
                    .map_err(|error| backfill_failure(transaction_groups, error))?;
            }
            let observation = self
                .observe_backfill(lease, plan, direction)
                .await
                .map_err(|error| backfill_failure(transaction_groups, error))?;
            if !matches!(observation, BackfillRecoveryObservation::Complete(_)) {
                return Err(backfill_failure(
                    transaction_groups,
                    failure(
                        DiagnosticCategory::Integrity,
                        "migration_typedb_backfill_postcondition_failed",
                        "backfill mutation ended without satisfying its exact postcondition",
                    ),
                ));
            }
            BackfillExecutionCounts::new(changed, changed, 0, transaction_groups.max(1))
                .map(|counts| BackfillCompletionEvidence::new(plan_fingerprint, direction, counts))
                .map_err(definitely_aborted)
        })
    }

    fn observe_managed_state<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source_candidate: &'a ManagedSchemaState,
        target_candidate: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, ManagedSchemaState> {
        Box::pin(async move {
            self.binding.require_lease(lease)?;
            self.require_execution_state(source_candidate, "observation_source")?;
            self.require_execution_state(target_candidate, "observation_target")?;
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
            self.require_execution_state(source, "group_source")?;
            self.require_execution_state(target, "group_target")?;
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
            rollback_and_close(&mut this.transaction)
                .await
                .map_err(map_orm_error)
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
    migration_runtime_capability_vocabulary()
}

fn backfill_lowering_error(_: type_bridge_core_lib::__internal::TqlError) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "migration_typedb_backfill_typeql_shape",
        "validated backfill cannot be lowered to TypeQL",
    )
}

async fn require_no_backfill_conflict(
    transaction: &mut Transaction,
    lowered: &LoweredBackfill<'_>,
) -> Result<(), Diagnostic> {
    let Some(query) = lowered
        .conflict_witness()
        .map_err(backfill_lowering_error)?
    else {
        return Ok(());
    };
    if query_has_answers(transaction, &query).await? {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_backfill_destination_conflict",
            "backfill destination contains a value different from its source",
        ));
    }
    Ok(())
}

async fn query_has_answers(transaction: &mut Transaction, query: &str) -> Result<bool, Diagnostic> {
    Ok(query_answer_count(transaction, query).await? != 0)
}

async fn query_answer_count(transaction: &mut Transaction, query: &str) -> Result<u64, Diagnostic> {
    let count = match transaction.query(query).await.map_err(map_orm_error)? {
        QueryResult::Ok => 0,
        QueryResult::Rows(rows) => rows.len(),
        QueryResult::Documents(documents) => documents.len(),
    };
    u64::try_from(count).map_err(|_| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "migration_typedb_backfill_answer_count_overflow",
            "backfill provider answer count exceeded the supported range",
        )
    })
}

fn definitely_aborted(diagnostic: Diagnostic) -> GroupCommitFailure {
    GroupCommitFailure::new(GroupCommitCertainty::DefinitelyAborted, diagnostic)
}

fn backfill_failure(committed_groups: u32, diagnostic: Diagnostic) -> GroupCommitFailure {
    GroupCommitFailure::new(
        if committed_groups == 0 {
            GroupCommitCertainty::DefinitelyAborted
        } else {
            GroupCommitCertainty::Unknown
        },
        diagnostic,
    )
}

async fn finish_provider_data_guard<T>(
    transaction: &mut Transaction,
    primary: Result<T, Diagnostic>,
    operation: &'static str,
) -> Result<T, Diagnostic> {
    finish_provider_schema_guard(transaction, primary, operation).await
}

/// Return the exact TypeDB release connected migrations require for a
/// workspace semantic profile, or `None` when the profile cannot run them.
///
/// The pairing is the generated-package release map, so a workspace's
/// migrations and its generated packages always target the same server:
///
/// | Semantic profile | Admitted servers (newest returned) |
/// |------------------|------------------------------------|
/// | `typedb-3.12.1/v1` | `3.12.3` |
/// | `typedb-3.13.6/v1` | `3.13.6`, `3.14.0` |
#[must_use]
pub fn migration_server_for_profile(semantic_profile: &SemanticProfileId) -> Option<Version> {
    GeneratedPackageRelease::for_profile(semantic_profile.as_str())
        .map(GeneratedPackageRelease::server)
}

/// Require a semantic profile that connected migrations support and return
/// the newest exact server release it pairs with.
///
/// This check is pure: it performs no credential, filesystem, or database work.
pub fn require_supported_migration_profile(
    semantic_profile: &SemanticProfileId,
) -> Result<Version, Diagnostic> {
    migration_release(semantic_profile).map(GeneratedPackageRelease::server)
}

/// The exact server releases a semantic profile's migrations run on.
fn migration_release(
    semantic_profile: &SemanticProfileId,
) -> Result<GeneratedPackageRelease, Diagnostic> {
    GeneratedPackageRelease::for_profile(semantic_profile.as_str()).ok_or_else(|| {
        let supported = supported_migration_profiles().join(", ");
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_semantic_profile_unsupported",
            format!(
                "semantic profile {} cannot run connected TypeDB migrations; expected one of \
                 {supported}",
                semantic_profile.as_str()
            ),
        )
        .with_detail("semantic_profile", semantic_profile.as_str().to_owned())
        .with_detail("expected_semantic_profiles", supported)
    })
}

fn supported_migration_profiles() -> Vec<String> {
    MigrationProfile::ALL
        .into_iter()
        .map(MigrationProfile::semantic_profile_id)
        .filter(|profile| migration_server_for_profile(profile).is_some())
        .map(|profile| profile.as_str().to_owned())
        .collect()
}

/// Require the exact TypeDB server a semantic profile's migrations run on.
///
/// The profile is checked first. This check reads only the version
/// negotiated by the existing connection; it performs no database lookup,
/// creation, transaction, or schema work.
pub fn require_supported_migration_server_for_profile(
    database: &Database,
    semantic_profile: &SemanticProfileId,
) -> Result<(), Diagnostic> {
    let expected = migration_release(semantic_profile)?;
    require_supported_server_version(
        semantic_profile,
        expected,
        negotiated_server_version(database),
    )
}

/// Require TypeDB 3.12.3, the server for semantic profile `typedb-3.12.1/v1`.
#[deprecated(
    since = "2.3.0",
    note = "use require_supported_migration_server_for_profile; removed in 3.0"
)]
pub fn require_supported_migration_server(database: &Database) -> Result<(), Diagnostic> {
    require_supported_migration_server_for_profile(
        database,
        &MigrationProfile::Typedb3_12_1.semantic_profile_id(),
    )
}

/// Require a supported semantic profile and, on both members of one
/// managed/journal pair, the exact server that profile pairs with.
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
    let expected = migration_release(context.semantic_profile())?;
    require_migration_database_pair_identity(managed_database, journal_database)?;
    require_supported_pair_server_versions(
        context.semantic_profile(),
        expected,
        negotiated_server_version(managed_database),
        negotiated_server_version(journal_database),
    )?;
    context
        .available_capabilities()
        .ensure_supported_by(&execution_capability_vocabulary()?)
}

/// Return the older profile a plan replays below the history's profile
/// bridge, if it plans any manifest off the execution profile.
///
/// The store has already bound the plan, which admits such a manifest only
/// as an ancestor of the catalog's single bridge to the execution profile.
pub(crate) fn replayed_profile_bridge_source(
    graph: &MigrationHistoryGraph,
    plan: &VerifiedMigrationApplyPlan,
    context: &ManagedDeltaContext,
) -> Option<SemanticProfileId> {
    let replays_older = plan.migrations().iter().any(|migration| {
        migration.manifest().semantic_profile().id() != context.semantic_profile()
    });
    if !replays_older {
        return None;
    }
    graph
        .manifests()
        .find_map(|(_, manifest)| manifest.profile_bridge())
        .map(|bridge| bridge.from_semantic_profile().id().clone())
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

fn negotiated_server_version(database: &Database) -> Option<Version> {
    database.server_version()
}

fn require_supported_pair_server_versions(
    semantic_profile: &SemanticProfileId,
    expected: GeneratedPackageRelease,
    managed_version: Option<Version>,
    journal_version: Option<Version>,
) -> Result<(), Diagnostic> {
    require_supported_server_version(semantic_profile, expected, managed_version)
        .map_err(|error| error.with_detail("database_role", "managed"))?;
    require_supported_server_version(semantic_profile, expected, journal_version)
        .map_err(|error| error.with_detail("database_role", "journal"))
}

fn require_supported_server_version(
    semantic_profile: &SemanticProfileId,
    expected: GeneratedPackageRelease,
    version: Option<Version>,
) -> Result<(), Diagnostic> {
    let version = version.ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_server_version_unknown",
            "migration execution requires a negotiated TypeDB server version",
        )
    })?;
    if !expected.admits(&version) {
        let admitted = expected.to_string();
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_server_version_unsupported",
            format!(
                "migration execution on semantic profile {} requires exactly TypeDB {admitted}",
                semantic_profile.as_str()
            ),
        )
        .with_detail("server_version", version.to_string())
        .with_detail("expected_server_version", admitted)
        .with_detail("semantic_profile", semantic_profile.as_str().to_owned()));
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
    match (primary, rollback_and_close(transaction).await) {
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

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: impl Into<String>,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}

pub(crate) async fn rollback_and_close(transaction: &mut Transaction) -> Result<(), OrmError> {
    let rollback = transaction.rollback().await;
    let close = transaction.close().await;
    rollback.and(close)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::migration::CONDITIONAL_RESOLUTION_CAPABILITY;
    use type_bridge_contract::migration_assertion_capability_vocabulary;
    use type_bridge_contract::schema_delta::SCHEMA_REDEFINE_CAPABILITY;
    use type_bridge_orm::DatabaseConnectionAuthority;
    use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps, TxType};
    use type_bridge_schema::BUILTIN_SCHEMA_CAPABILITY_IDS;
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

    fn profile(id: &str) -> SemanticProfileId {
        SemanticProfileId::new(id).expect("valid semantic profile")
    }

    fn version(major: u32, minor: u32, patch: u32) -> Option<Version> {
        Some(Version::new(major, minor, patch))
    }

    #[test]
    fn migration_profiles_pair_with_their_newest_server() {
        assert_eq!(
            migration_server_for_profile(&profile("typedb-3.12.1/v1")),
            Some(Version::new(3, 12, 3))
        );
        assert_eq!(
            migration_server_for_profile(&profile("typedb-3.13.6/v1")),
            Some(Version::new(3, 14, 0))
        );
        for unsupported in ["typedb-3.11.5/v1", "typedb-3.12.3/v1", "typedb-3.14.0/v1"] {
            assert_eq!(migration_server_for_profile(&profile(unsupported)), None);
            let error = require_supported_migration_profile(&profile(unsupported))
                .expect_err("only paired profiles run connected migrations");
            assert_eq!(
                error.code().as_str(),
                "migration_typedb_semantic_profile_unsupported"
            );
        }
        // Every schema-transition registry has a connected server, and no other
        // profile does.
        for migration_profile in MigrationProfile::ALL {
            assert!(
                migration_server_for_profile(&migration_profile.semantic_profile_id()).is_some()
            );
        }
        assert_eq!(
            supported_migration_profiles(),
            ["typedb-3.12.1/v1", "typedb-3.13.6/v1"]
        );
    }

    #[test]
    fn migration_server_gate_accepts_exactly_the_profile_pairs() {
        let accepted = [
            ("typedb-3.12.1/v1", (3, 12, 3)),
            ("typedb-3.13.6/v1", (3, 13, 6)),
            ("typedb-3.13.6/v1", (3, 14, 0)),
        ];
        for (id, (major, minor, patch)) in accepted {
            let semantic_profile = profile(id);
            let expected = migration_release(&semantic_profile).expect("paired profile");
            require_supported_server_version(
                &semantic_profile,
                expected,
                version(major, minor, patch),
            )
            .expect("the exact pair is accepted");

            let unknown = require_supported_server_version(&semantic_profile, expected, None)
                .expect_err("unknown must reject");
            assert_eq!(
                unknown.code().as_str(),
                "migration_typedb_server_version_unknown"
            );
        }

        let refused = [
            ("typedb-3.12.1/v1", (3, 13, 6)),
            ("typedb-3.13.6/v1", (3, 12, 3)),
            ("typedb-3.12.1/v1", (3, 11, 5)),
            ("typedb-3.13.6/v1", (3, 11, 5)),
            ("typedb-3.12.1/v1", (3, 12, 2)),
            ("typedb-3.13.6/v1", (3, 13, 5)),
            ("typedb-3.12.1/v1", (3, 14, 0)),
            ("typedb-3.13.6/v1", (3, 15, 0)),
        ];
        for (id, (major, minor, patch)) in refused {
            let semantic_profile = profile(id);
            let expected = migration_release(&semantic_profile).expect("paired profile");
            let admitted = expected.to_string();
            let error = require_supported_server_version(
                &semantic_profile,
                expected,
                version(major, minor, patch),
            )
            .expect_err("every other server must reject");
            assert_eq!(
                error.code().as_str(),
                "migration_typedb_server_version_unsupported"
            );
            assert!(
                error.message().contains(&admitted),
                "the refusal names the profile's servers: {}",
                error.message()
            );
            assert_eq!(
                error.details().get("expected_server_version"),
                Some(&type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(admitted))
            );
        }

        let semantic_profile = profile("typedb-3.12.1/v1");
        let expected = migration_release(&semantic_profile).expect("paired profile");
        let managed = require_supported_pair_server_versions(
            &semantic_profile,
            expected,
            None,
            version(3, 12, 3),
        )
        .expect_err("managed identity is checked first");
        assert_eq!(
            managed.details().get("database_role"),
            Some(
                &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(
                    "managed".to_owned()
                )
            )
        );
        let journal = require_supported_pair_server_versions(
            &semantic_profile,
            expected,
            version(3, 12, 3),
            version(3, 11, 5),
        )
        .expect_err("journal identity is checked after the managed identity");
        assert_eq!(
            journal.details().get("database_role"),
            Some(
                &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(
                    "journal".to_owned()
                )
            )
        );
        require_supported_pair_server_versions(
            &semantic_profile,
            expected,
            version(3, 12, 3),
            version(3, 12, 3),
        )
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
        let unsupported =
            require_supported_migration_server_for_profile(&database, &profile("typedb-3.11.5/v1"))
                .expect_err("an unsupported profile rejects before the server");
        assert_eq!(
            unsupported.code().as_str(),
            "migration_typedb_semantic_profile_unsupported"
        );
        for id in ["typedb-3.12.1/v1", "typedb-3.13.6/v1"] {
            let error = require_supported_migration_server_for_profile(&database, &profile(id))
                .expect_err("a backend without version identity must reject");
            assert_eq!(
                error.code().as_str(),
                "migration_typedb_server_version_unknown"
            );
        }
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

        for migration_profile in MigrationProfile::ALL {
            let supported_context = ManagedDeltaContext::new(
                ManagedScopeId::new("provider-binding-gate").expect("managed scope"),
                migration_profile.semantic_profile_id(),
                CapabilitySet::new(),
            );
            let server_error = require_supported_migration_execution_binding(
                &database,
                &journal,
                &supported_context,
            )
            .expect_err("a supported profile still requires negotiated server identity");
            assert_eq!(
                server_error.code().as_str(),
                "migration_typedb_server_version_unknown"
            );
        }
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
            COPY_ATTRIBUTE_BACKFILL_CAPABILITY,
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
                    COPY_ATTRIBUTE_BACKFILL_CAPABILITY,
                ]
                .iter()
                .map(ToString::to_string),
            )
            .collect();
        let actual: std::collections::BTreeSet<String> =
            capabilities.iter().map(ToString::to_string).collect();
        assert_eq!(actual, expected);
        let backfill = CapabilityId::new(COPY_ATTRIBUTE_BACKFILL_CAPABILITY).unwrap();
        let schema_only: CapabilitySet = capabilities
            .iter()
            .filter(|capability| *capability != &backfill)
            .cloned()
            .collect();
        schema_only
            .ensure_supported_by(&authority_capabilities)
            .expect("schema authority consumers understand every schema execution requirement");
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

    #[test]
    fn backfill_failure_certainty_accounts_for_prior_commits() {
        let before_commit = backfill_failure(
            0,
            failure(
                DiagnosticCategory::InvalidContract,
                "test_backfill_failure",
                "test backfill failure",
            ),
        );
        assert_eq!(
            before_commit.certainty(),
            GroupCommitCertainty::DefinitelyAborted
        );

        let after_commit = backfill_failure(
            1,
            failure(
                DiagnosticCategory::InvalidContract,
                "test_backfill_failure",
                "test backfill failure",
            ),
        );
        assert_eq!(after_commit.certainty(), GroupCommitCertainty::Unknown);
    }
}
