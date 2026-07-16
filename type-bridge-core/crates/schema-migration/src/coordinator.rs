//! Provider-neutral fenced execution of verified migration apply plans.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::ManagedSchemaState;
use type_bridge_query::ValidatedMigrationAssertionPlan;

use crate::execution::{
    AppliedRecord, ExecutionFuture, GroupCommitCertainty, GroupEventRecord,
    GroupJournalEventKind, GroupRecoveryDecision, GroupRecoveryObservation,
    LeaseHolderId, MigrationExecutionJournal, MigrationLease,
    MigrationLeaseStore, OpenPlanRecord, PlanRecord, decide_group_recovery,
};
use crate::{
    StatementUnit, VerifiedMigrationApplyManifest, VerifiedMigrationApplyPlan,
    VerifiedMigrationTransactionGroup,
};

/// Future returned by a consuming provider commit operation.
pub type GroupCommitFuture<'a> = Pin<
    Box<dyn Future<Output = Result<(), GroupCommitFailure>> + Send + 'a>,
>;

/// A provider commit failure with explicit asymmetric certainty.
#[derive(Debug)]
pub struct GroupCommitFailure {
    certainty: GroupCommitCertainty,
    diagnostic: Diagnostic,
}

impl GroupCommitFailure {
    /// Construct a privacy-safe typed commit failure.
    pub const fn new(
        certainty: GroupCommitCertainty,
        diagnostic: Diagnostic,
    ) -> Self {
        Self { certainty, diagnostic }
    }

    /// Return whether the provider proved non-commit or cannot know.
    pub const fn certainty(&self) -> GroupCommitCertainty {
        self.certainty
    }

    /// Return the provider diagnostic without runtime row evidence.
    pub const fn diagnostic(&self) -> &Diagnostic {
        &self.diagnostic
    }

    fn into_parts(self) -> (GroupCommitCertainty, Diagnostic) {
        (self.certainty, self.diagnostic)
    }
}

/// One provider transaction prepared for an exact verifier-owned group.
pub trait PreparedMigrationGroup: Send {
    /// Execute one retained validated assertion in manifest order.
    fn execute_assertion<'a>(
        &'a mut self,
        plan: &'a ValidatedMigrationAssertionPlan,
    ) -> ExecutionFuture<'a, ()>;

    /// Execute one provider-lowered statement unit in exact order.
    fn execute_statement_unit<'a>(
        &'a mut self,
        unit: &'a StatementUnit,
    ) -> ExecutionFuture<'a, ()>;

    /// Commit once after atomically rechecking the supplied active fence.
    ///
    /// The check must participate in the same provider transaction as the
    /// schema statements. A lease takeover racing commit must therefore abort
    /// that commit or surface an unknown outcome, never commit under a stale
    /// fence after an advisory out-of-transaction check.
    fn commit<'a>(
        self: Box<Self>,
        lease: &'a MigrationLease,
    ) -> GroupCommitFuture<'a>
    where
        Self: 'a;

    /// Abort an uncommitted prepared group.
    fn rollback<'a>(self: Box<Self>) -> ExecutionFuture<'a, ()>
    where
        Self: 'a;
}

/// Provider seam for exact managed-state observation and group transactions.
pub trait MigrationExecutionProvider: Send + Sync {
    /// Return the exact capabilities negotiated by this execution provider.
    fn available_capabilities(&self) -> &CapabilitySet;

    /// Independently observe managed state under the current active fence.
    ///
    /// The source and target candidates supply the already-verified scope,
    /// profiles, and possible managed selections used for observation. Their
    /// fingerprint claims are not observation input.
    fn observe_managed_state<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source_candidate: &'a ManagedSchemaState,
        target_candidate: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, ManagedSchemaState>;

    /// Begin a transaction that rechecks exact source state and the active fence.
    fn prepare_group<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source: &'a ManagedSchemaState,
        target: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, Box<dyn PreparedMigrationGroup + 'a>>;
}

/// Terminal result of one coordinator invocation.
#[derive(Debug)]
pub enum MigrationExecutionOutcome {
    /// Every planned manifest is durably checkpointed as applied.
    Applied,
    /// A later invocation may safely retry or repair journal-only progress.
    RetrySafe {
        /// Migration containing the interrupted group.
        migration_id: MigrationId,
        /// Zero-based verifier-owned group position.
        group_ordinal: usize,
        /// Privacy-safe provider or journal failure.
        diagnostic: Diagnostic,
    },
    /// Evidence cannot distinguish replay from duplication.
    RequiresExplicitRecovery {
        /// Migration containing the ambiguous group.
        migration_id: MigrationId,
        /// Zero-based verifier-owned group position.
        group_ordinal: usize,
        /// Privacy-safe reason automatic progress was refused.
        diagnostic: Diagnostic,
    },
}

/// Execute one complete verified plan under a store-backed fenced lease.
///
/// Planning, lowering, assertion validation, and grouping must already be
/// complete. This function never re-groups steps and never retries a commit
/// failure inside the same invocation. A verified no-op plan is deliberately
/// rejected because it has no execution scope or journal identity to acquire.
pub async fn execute_verified_migration_apply_plan<S, P>(
    store: &S,
    provider: &P,
    holder: &LeaseHolderId,
    plan: &VerifiedMigrationApplyPlan,
) -> Result<MigrationExecutionOutcome, Diagnostic>
where
    S: MigrationLeaseStore + MigrationExecutionJournal,
    P: MigrationExecutionProvider,
{
    plan.required_capabilities()
        .ensure_supported_by(provider.available_capabilities())?;
    let source = plan.source_state().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_execution_empty_plan",
            "an executable migration plan requires a source state",
        )
    })?;
    if plan.migrations().is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_execution_empty_plan",
            "an executable migration plan requires at least one manifest",
        ));
    }
    let scope = crate::ExecutionScope::new(source.scope().id().clone());
    let lease = store.acquire(&scope, holder).await?;
    let result = execute_under_lease(store, provider, &lease, plan).await;
    let release = store.release(&lease).await;
    match result {
        Ok(MigrationExecutionOutcome::Applied) => {
            release?;
            Ok(MigrationExecutionOutcome::Applied)
        }
        Ok(outcome) => {
            let _ = release;
            Ok(outcome)
        }
        Err(error) => {
            let _ = release;
            Err(error)
        }
    }
}

async fn execute_under_lease<S, P>(
    store: &S,
    provider: &P,
    lease: &MigrationLease,
    plan: &VerifiedMigrationApplyPlan,
) -> Result<MigrationExecutionOutcome, Diagnostic>
where
    S: MigrationExecutionJournal,
    P: MigrationExecutionProvider,
{
    let applied = store.load_applied(lease).await?;
    let open = store.load_open_plan(lease).await?;
    let completed_manifests = match &open {
        Some(open) => validate_open_plan(plan, lease, open, &applied)?,
        None => 0,
    };
    let mut observed = None;

    if open.is_none() {
        let source = plan.source_state().expect("non-empty plan has source state");
        let live_source = provider
            .observe_managed_state(lease, source, source)
            .await?;
        let applied_migrations = loaded_applied_migrations(&applied)?;
        let record = PlanRecord::from_verified_plan(
            lease,
            plan,
            &applied_migrations,
            &live_source,
        )?;
        store.begin_plan(lease, record).await?;
        observed = Some(live_source);
    }

    for (manifest_index, migration) in plan.migrations().iter().enumerate() {
        if manifest_index < completed_manifests {
            continue;
        }
        let snapshot = open.as_ref();
        for group in migration.transaction_groups() {
            let last_event = snapshot.and_then(|open| {
                last_group_event(open, migration, group)
            });
            if last_event.is_some_and(is_completion_event) {
                continue;
            }
            let (source, target, lowering) = group_evidence(migration, group)?;
            if observed.is_none() {
                observed = Some(
                    provider
                        .observe_managed_state(lease, source, target)
                        .await?,
                );
            }
            let observation = exact_observation(
                observed.as_ref().expect("group observation"),
                source,
                target,
            );
            match decide_group_recovery(
                last_event,
                &observation,
                source.managed_semantic_schema(),
                target.managed_semantic_schema(),
            ) {
                GroupRecoveryDecision::RequiresExplicitRecovery => {
                    return Ok(explicit_recovery(
                        migration,
                        group,
                        failure(
                            DiagnosticCategory::Integrity,
                            "migration_execution_ambiguous_group_state",
                            "live managed state cannot prove whether the group may replay",
                        ),
                    ));
                }
                GroupRecoveryDecision::RepairCheckpoint => {
                    let committed = GroupEventRecord::new(
                        lease,
                        migration,
                        group,
                        GroupJournalEventKind::Committed,
                        Some(target.managed_semantic_schema().clone()),
                    )?;
                    if let Err(error) = store
                        .record_group_event(lease, committed)
                        .await
                    {
                        return Ok(retry_safe(migration, group, error));
                    }
                    observed = Some(target.clone());
                    continue;
                }
                GroupRecoveryDecision::ExecuteNormally => {}
            }

            if group.assertion_count() == 0
                && lowering.units().is_empty()
                && source.managed_semantic_schema()
                    == target.managed_semantic_schema()
            {
                let event = GroupEventRecord::new(
                    lease,
                    migration,
                    group,
                    GroupJournalEventKind::FormalOnlyAdvanced,
                    None,
                )?;
                if let Err(error) = store.record_group_event(lease, event).await {
                    return Ok(retry_safe(migration, group, error));
                }
                observed = Some(target.clone());
                continue;
            }

            let mut transaction = provider
                .prepare_group(lease, source, target)
                .await?;
            let steps = &migration.steps()[
                group.first_step_index()..group.end_step_index()
            ];
            for step in &steps[..group.assertion_count()] {
                let validated = step.validated_assertion().ok_or_else(|| {
                    failure(
                        DiagnosticCategory::Integrity,
                        "migration_execution_group_assertion_missing",
                        "stored transaction group lost validated assertion evidence",
                    )
                })?;
                if let Err(error) = transaction.execute_assertion(validated).await {
                    let _ = transaction.rollback().await;
                    return Err(error);
                }
            }
            for unit in lowering.units() {
                if let Err(error) = transaction.execute_statement_unit(unit).await {
                    let _ = transaction.rollback().await;
                    return Err(error);
                }
            }
            let before = GroupEventRecord::new(
                lease,
                migration,
                group,
                GroupJournalEventKind::BeforeCommit,
                None,
            )?;
            if let Err(error) = store.record_group_event(lease, before).await {
                let _ = transaction.rollback().await;
                return Err(error);
            }

            match transaction.commit(lease).await {
                Ok(()) => {
                    observed = match provider
                        .observe_managed_state(lease, source, target)
                        .await
                    {
                        Ok(observed) if observed == *target => observed,
                        Ok(_) => {
                            return Ok(explicit_recovery(
                                migration,
                                group,
                                failure(
                                    DiagnosticCategory::Integrity,
                                    "migration_execution_commit_target_mismatch",
                                    "commit succeeded but exact target state was not observed",
                                ),
                            ));
                        }
                        Err(error) => {
                            return Ok(retry_safe(migration, group, error));
                        }
                    }.into();
                    let committed = GroupEventRecord::new(
                        lease,
                        migration,
                        group,
                        GroupJournalEventKind::Committed,
                        Some(target.managed_semantic_schema().clone()),
                    )?;
                    if let Err(error) = store
                        .record_group_event(lease, committed)
                        .await
                    {
                        return Ok(retry_safe(migration, group, error));
                    }
                }
                Err(commit_failure) => {
                    let (certainty, diagnostic) = commit_failure.into_parts();
                    let event = GroupEventRecord::new(
                        lease,
                        migration,
                        group,
                        certainty.journal_event(),
                        None,
                    )?;
                    if let Err(error) = store.record_group_event(lease, event).await {
                        return Ok(match certainty {
                            GroupCommitCertainty::DefinitelyAborted => {
                                retry_safe(migration, group, error)
                            }
                            GroupCommitCertainty::Unknown => {
                                explicit_recovery(migration, group, error)
                            }
                        });
                    }
                    if certainty == GroupCommitCertainty::DefinitelyAborted {
                        return Ok(retry_safe(migration, group, diagnostic));
                    }
                    let after = match provider
                        .observe_managed_state(lease, source, target)
                        .await
                    {
                        Ok(value) => value,
                        Err(error) => {
                            return Ok(explicit_recovery(migration, group, error));
                        }
                    };
                    let after_observation = exact_observation(&after, source, target);
                    match decide_group_recovery(
                        Some(GroupJournalEventKind::CommitOutcomeUnknown),
                        &after_observation,
                        source.managed_semantic_schema(),
                        target.managed_semantic_schema(),
                    ) {
                        GroupRecoveryDecision::ExecuteNormally => {
                            return Ok(retry_safe(migration, group, diagnostic));
                        }
                        GroupRecoveryDecision::RequiresExplicitRecovery => {
                            return Ok(explicit_recovery(
                                migration,
                                group,
                                diagnostic,
                            ));
                        }
                        GroupRecoveryDecision::RepairCheckpoint => {
                            let committed = GroupEventRecord::new(
                                lease,
                                migration,
                                group,
                                GroupJournalEventKind::Committed,
                                Some(target.managed_semantic_schema().clone()),
                            )?;
                            if let Err(error) = store
                                .record_group_event(lease, committed)
                                .await
                            {
                                return Ok(retry_safe(migration, group, error));
                            }
                            observed = Some(target.clone());
                        }
                    }
                }
            }
        }

        if observed.is_none() {
            let target = migration.manifest().target_state();
            observed = Some(
                provider
                    .observe_managed_state(lease, target, target)
                    .await?,
            );
        }
        if observed.as_ref() != Some(migration.manifest().target_state()) {
            return Ok(MigrationExecutionOutcome::RequiresExplicitRecovery {
                migration_id: migration.manifest().id().clone(),
                group_ordinal: last_group_ordinal(migration)?,
                diagnostic: failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_manifest_target_mismatch",
                    "completed groups do not have the exact manifest target state",
                ),
            });
        }
        let applied = AppliedRecord::from_verified_manifest(lease, migration)?;
        if let Err(error) = store.record_applied(lease, applied).await {
            return Ok(MigrationExecutionOutcome::RetrySafe {
                migration_id: migration.manifest().id().clone(),
                group_ordinal: last_group_ordinal(migration)?,
                diagnostic: error,
            });
        }
    }
    Ok(MigrationExecutionOutcome::Applied)
}

fn validate_open_plan(
    plan: &VerifiedMigrationApplyPlan,
    lease: &MigrationLease,
    open: &OpenPlanRecord,
    applied: &[crate::JournalEntry<AppliedRecord>],
) -> Result<usize, Diagnostic> {
    validate_plan_identity(open.plan().record(), plan)?;
    if open.plan().record().scope() != lease.scope()
        || open.plan().record().fence() > lease.fence()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_execution_open_plan_fence_mismatch",
            "open plan is not owned by this scope or predates no active fence",
        ));
    }

    let mut applied_flags = vec![false; plan.migrations().len()];
    let mut previous_sequence = None;
    for entry in applied {
        if previous_sequence.is_some_and(|previous| previous >= entry.sequence()) {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_applied_order_mismatch",
                "applied ledger entries are not strictly ordered",
            ));
        }
        previous_sequence = Some(entry.sequence());
        let position = plan.migrations().iter().position(|migration| {
            migration.manifest().id() == entry.record().migration_id()
        });
        let Some(position) = position else {
            if entry.sequence() > open.plan().sequence() {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_foreign_applied_progress",
                    "an applied record outside the open plan was appended during execution",
                ));
            }
            continue;
        };
        if entry.sequence() <= open.plan().sequence()
            || applied_flags[position]
            || !applied_matches(entry.record(), &plan.migrations()[position])
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_applied_evidence_mismatch",
                "applied progress differs from the exact open plan",
            ));
        }
        applied_flags[position] = true;
    }
    let completed = applied_flags.iter().take_while(|value| **value).count();
    if applied_flags[completed..].iter().any(|value| *value) {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_execution_non_prefix_applied_progress",
            "open-plan migrations are not applied as an ordered prefix",
        ));
    }

    for event in open.events() {
        let record = event.record();
        let migration_index = plan.migrations().iter().position(|migration| {
            migration.digest() == record.manifest_digest()
                && migration.manifest().id() == record.migration_id()
        }).ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_execution_foreign_group_event",
                "open-plan event has no exact verified manifest",
            )
        })?;
        let migration = &plan.migrations()[migration_index];
        let group_index = usize::try_from(record.group_ordinal()).map_err(|_| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "migration_execution_group_position_limit",
                "journal group position exceeds this platform",
            )
        })?;
        let group = migration.transaction_groups().get(group_index).ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_execution_group_position_mismatch",
                "journal event group is absent from the verified manifest",
            )
        })?;
        let historical_lease = MigrationLease::new(
            lease.scope().clone(),
            lease.holder().clone(),
            record.fence(),
        );
        let expected = GroupEventRecord::new(
            &historical_lease,
            migration,
            group,
            record.kind(),
            record.observed_target().cloned(),
        )?;
        if &expected != record {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_group_event_mismatch",
                "journal event differs from exact verified group evidence",
            ));
        }
    }

    for (migration_index, migration) in plan.migrations().iter().enumerate() {
        let mut progress_closed = false;
        let mut all_complete = true;
        for group in migration.transaction_groups() {
            let events = group_events(open, migration, group);
            validate_group_transitions(&events)?;
            if events.is_empty() {
                progress_closed = true;
                all_complete = false;
                continue;
            }
            if progress_closed {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_non_prefix_group_progress",
                    "transaction-group journal progress is not a prefix",
                ));
            }
            let complete = events
                .last()
                .is_some_and(|event| is_completion_event(event.kind()));
            if !complete {
                progress_closed = true;
                all_complete = false;
            }
        }
        if applied_flags[migration_index] != all_complete {
            if applied_flags[migration_index] || migration_index < completed {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_applied_group_mismatch",
                    "applied record and group completion evidence disagree",
                ));
            }
        }
        if migration_index > completed
            && migration.transaction_groups().iter().any(|group| {
                !group_events(open, migration, group).is_empty()
            })
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_future_manifest_progress",
                "a later manifest has journal progress before its predecessor",
            ));
        }
    }
    Ok(completed)
}

fn validate_plan_identity(
    record: &PlanRecord,
    plan: &VerifiedMigrationApplyPlan,
) -> Result<(), Diagnostic> {
    let source = plan.source_state().ok_or_else(|| failure(
        DiagnosticCategory::InvalidContract,
        "migration_execution_empty_plan",
        "an executable migration plan requires a source state",
    ))?;
    let target = plan.target_state().ok_or_else(|| failure(
        DiagnosticCategory::InvalidContract,
        "migration_execution_empty_plan",
        "an executable migration plan requires a target state",
    ))?;
    let first = plan.migrations().first().ok_or_else(|| failure(
        DiagnosticCategory::InvalidContract,
        "migration_execution_empty_plan",
        "an executable migration plan requires at least one manifest",
    ))?;
    let ids: Vec<_> = plan.migrations().iter()
        .map(|migration| migration.manifest().id().clone()).collect();
    let digests: Vec<_> = plan.migrations().iter()
        .map(VerifiedMigrationApplyManifest::digest).collect();
    let fingerprints: Vec<_> = plan.migrations().iter()
        .map(|migration| migration.manifest().plan_fingerprint().clone()).collect();
    let matches = record.source_frontier() == plan.applied_frontier()
        && record.source_applied() == plan.applied_migrations()
        && record.target_frontier() == plan.target_frontier()
        && record.migration_ids() == ids
        && record.manifest_digests() == digests
        && record.manifest_plan_fingerprints() == fingerprints
        && record.scope().managed_scope_id() == source.scope().id()
        && source.scope() == target.scope()
        && record.source_declared() == source.managed_declared_identity()
        && record.target_declared() == target.managed_declared_identity()
        && record.source_semantics() == source.managed_semantic_schema()
        && record.target_semantics() == target.managed_semantic_schema()
        && record.observed_live_source() == source.managed_semantic_schema()
        && record.semantic_profile()
            == first.manifest().semantic_profile().fingerprint()
        && record.lowering_profile()
            == first.manifest().lowering_profile().fingerprint();
    if !matches {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_execution_open_plan_identity_mismatch",
            "open journal plan differs from the freshly verified apply plan",
        ));
    }
    Ok(())
}

fn applied_matches(
    record: &AppliedRecord,
    migration: &VerifiedMigrationApplyManifest,
) -> bool {
    let source = migration.manifest().source_state();
    let target = migration.manifest().target_state();
    record.scope().managed_scope_id() == source.scope().id()
        && record.migration_id() == migration.manifest().id()
        && record.manifest_digest() == migration.digest()
        && record.source_declared() == source.managed_declared_identity()
        && record.target_declared() == target.managed_declared_identity()
        && record.source_semantics() == source.managed_semantic_schema()
        && record.target_semantics() == target.managed_semantic_schema()
}

fn group_events<'a>(
    open: &'a OpenPlanRecord,
    migration: &VerifiedMigrationApplyManifest,
    group: &VerifiedMigrationTransactionGroup,
) -> Vec<&'a GroupEventRecord> {
    open.events().iter()
        .map(crate::JournalEntry::record)
        .filter(|event| {
            event.manifest_digest() == migration.digest()
                && event.group_ordinal() as usize == group.ordinal()
        })
        .collect()
}

fn last_group_event(
    open: &OpenPlanRecord,
    migration: &VerifiedMigrationApplyManifest,
    group: &VerifiedMigrationTransactionGroup,
) -> Option<GroupJournalEventKind> {
    group_events(open, migration, group)
        .last()
        .map(|event| event.kind())
}

fn validate_group_transitions(events: &[&GroupEventRecord]) -> Result<(), Diagnostic> {
    let mut previous: Option<&GroupEventRecord> = None;
    for event in events {
        let valid = match previous {
            None => matches!(
                event.kind(),
                GroupJournalEventKind::BeforeCommit
                    | GroupJournalEventKind::FormalOnlyAdvanced
            ),
            Some(prior) => match (prior.kind(), event.kind()) {
                (
                    GroupJournalEventKind::BeforeCommit,
                    GroupJournalEventKind::Committed
                        | GroupJournalEventKind::CommitOutcomeUnknown
                        | GroupJournalEventKind::DefinitelyAborted,
                ) => true,
                (
                    GroupJournalEventKind::BeforeCommit
                        | GroupJournalEventKind::CommitOutcomeUnknown
                        | GroupJournalEventKind::DefinitelyAborted,
                    GroupJournalEventKind::BeforeCommit,
                ) => event.fence() > prior.fence(),
                (
                    GroupJournalEventKind::CommitOutcomeUnknown,
                    GroupJournalEventKind::Committed,
                ) => event.fence() >= prior.fence(),
                _ => false,
            },
        };
        if !valid {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_invalid_event_transition",
                "group journal event order is not a valid commit state machine",
            ));
        }
        previous = Some(event);
    }
    Ok(())
}

fn group_evidence<'a>(
    migration: &'a VerifiedMigrationApplyManifest,
    group: &VerifiedMigrationTransactionGroup,
) -> Result<(
    &'a ManagedSchemaState,
    &'a ManagedSchemaState,
    &'a crate::SchemaLoweringPlan,
), Diagnostic> {
    let step = migration.steps().get(group.schema_delta_step_index())
        .ok_or_else(|| failure(
            DiagnosticCategory::Integrity,
            "migration_execution_group_position_mismatch",
            "transaction group delta is outside the verified manifest",
        ))?;
    let delta = step.step().as_schema_delta().ok_or_else(|| failure(
        DiagnosticCategory::Integrity,
        "migration_execution_group_step_kind_mismatch",
        "transaction group does not terminate in a schema delta",
    ))?.delta();
    let lowering = step.lowering().ok_or_else(|| failure(
        DiagnosticCategory::Integrity,
        "migration_execution_group_lowering_missing",
        "transaction group delta has no verified lowering",
    ))?;
    Ok((delta.source(), delta.target(), lowering))
}

fn exact_observation(
    observed: &ManagedSchemaState,
    source: &ManagedSchemaState,
    target: &ManagedSchemaState,
) -> GroupRecoveryObservation {
    if observed == source || observed == target {
        GroupRecoveryObservation::ManagedSemantics(
            observed.managed_semantic_schema().clone(),
        )
    } else {
        GroupRecoveryObservation::Unavailable
    }
}

fn loaded_applied_migrations(
    applied: &[crate::JournalEntry<AppliedRecord>],
) -> Result<Vec<MigrationId>, Diagnostic> {
    let mut previous_sequence = None;
    let mut identities = BTreeSet::new();
    for entry in applied {
        if previous_sequence.is_some_and(|previous| previous >= entry.sequence())
            || !identities.insert(entry.record().migration_id().clone())
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_applied_ledger_mismatch",
                "applied ledger identities are duplicated or not strictly ordered",
            ));
        }
        previous_sequence = Some(entry.sequence());
    }
    Ok(identities.into_iter().collect())
}

fn is_completion_event(event: GroupJournalEventKind) -> bool {
    matches!(
        event,
        GroupJournalEventKind::Committed
            | GroupJournalEventKind::FormalOnlyAdvanced
    )
}

fn retry_safe(
    migration: &VerifiedMigrationApplyManifest,
    group: &VerifiedMigrationTransactionGroup,
    diagnostic: Diagnostic,
) -> MigrationExecutionOutcome {
    MigrationExecutionOutcome::RetrySafe {
        migration_id: migration.manifest().id().clone(),
        group_ordinal: group.ordinal(),
        diagnostic,
    }
}

fn explicit_recovery(
    migration: &VerifiedMigrationApplyManifest,
    group: &VerifiedMigrationTransactionGroup,
    diagnostic: Diagnostic,
) -> MigrationExecutionOutcome {
    MigrationExecutionOutcome::RequiresExplicitRecovery {
        migration_id: migration.manifest().id().clone(),
        group_ordinal: group.ordinal(),
        diagnostic,
    }
}

fn last_group_ordinal(
    migration: &VerifiedMigrationApplyManifest,
) -> Result<usize, Diagnostic> {
    let ordinal = migration.transaction_groups().last().ok_or_else(|| failure(
        DiagnosticCategory::Integrity,
        "migration_execution_manifest_without_group",
        "verified migration manifest has no transaction group",
    ))?.ordinal();
    Ok(ordinal)
}

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code)
            .expect("static migration coordinator diagnostic code"),
        message,
    )
}
