//! Provider-neutral fenced migration journal and recovery contracts.

use std::future::Future;
use std::pin::Pin;

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::managed_scope::{
    ManagedScopeId, SemanticProfileFingerprint,
};
use type_bridge_contract::migration::{
    MigrationId, MigrationManifestDigest, MigrationPlanFingerprint,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::schema_fingerprint::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
};
use type_bridge_contract::schema_lowering::SchemaLoweringProfileFingerprint;

use crate::{
    VerifiedMigrationApplyManifest, VerifiedMigrationApplyPlan,
    VerifiedMigrationTransactionGroup,
};

const MAX_LEASE_HOLDER_BYTES: usize = 128;

/// Boxed future returned by provider-neutral execution stores.
pub type ExecutionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, Diagnostic>> + Send + 'a>>;

/// A monotonically increasing store-issued migration fencing token.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExecutionFence(u64);

impl ExecutionFence {
    /// Construct a non-zero fence.
    pub fn new(value: u64) -> Result<Self, Diagnostic> {
        if value == 0 {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_execution_zero_fence",
                "migration execution fences must be non-zero",
            ));
        }
        Ok(Self(value))
    }

    /// Return the numeric fence value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Derive the next strictly greater fence without wrapping.
    pub fn checked_successor(self) -> Result<Self, Diagnostic> {
        let value = self.0.checked_add(1).ok_or_else(|| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "migration_execution_fence_exhausted",
                "migration execution fence range is exhausted",
            )
        })?;
        Self::new(value)
    }
}

/// One durable managed migration scope.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExecutionScope(ManagedScopeId);

impl ExecutionScope {
    /// Bind execution to an existing managed-scope identity.
    pub const fn new(scope: ManagedScopeId) -> Self {
        Self(scope)
    }

    /// Return the managed-scope identity.
    pub const fn managed_scope_id(&self) -> &ManagedScopeId {
        &self.0
    }
}

/// Caller-supplied lease holder identity with no machine-derived garnish.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LeaseHolderId(String);

impl LeaseHolderId {
    /// Validate a bounded canonical holder label.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_LEASE_HOLDER_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_execution_invalid_holder",
                "lease holder must be bounded non-empty ASCII [A-Za-z0-9._-]",
            ));
        }
        Ok(Self(value))
    }

    /// Return the canonical holder label.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Store-issued authority to mutate one migration scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationLease {
    scope: ExecutionScope,
    holder: LeaseHolderId,
    fence: ExecutionFence,
}

impl MigrationLease {
    /// Construct the lease returned by a store after atomic acquisition.
    pub const fn new(
        scope: ExecutionScope,
        holder: LeaseHolderId,
        fence: ExecutionFence,
    ) -> Self {
        Self {
            scope,
            holder,
            fence,
        }
    }

    /// Return the leased scope.
    pub const fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    /// Return the holder identity.
    pub const fn holder(&self) -> &LeaseHolderId {
        &self.holder
    }

    /// Return the store-issued fence.
    pub const fn fence(&self) -> ExecutionFence {
        self.fence
    }
}

/// Commit certainty owned by the migration journal layer.
///
/// Provider adapters must map absent certainty information to [`Self::Unknown`].
/// Only an explicit provider proof that commit could not have occurred may map
/// to [`Self::DefinitelyAborted`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupCommitCertainty {
    /// The provider proves that the group transaction did not commit.
    DefinitelyAborted,
    /// The provider cannot determine whether the group transaction committed.
    Unknown,
}

impl GroupCommitCertainty {
    /// Convert certainty into its durable journal event.
    pub const fn journal_event(self) -> GroupJournalEventKind {
        match self {
            Self::DefinitelyAborted => GroupJournalEventKind::DefinitelyAborted,
            Self::Unknown => GroupJournalEventKind::CommitOutcomeUnknown,
        }
    }
}

/// Durable commit-boundary event vocabulary for one transaction group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupJournalEventKind {
    /// The group statements completed and the commit call is about to begin.
    BeforeCommit,
    /// The provider commit succeeded and exact live target semantics were observed.
    Committed,
    /// The commit response cannot prove whether durability occurred.
    CommitOutcomeUnknown,
    /// The provider proves that the transaction did not commit.
    DefinitelyAborted,
    /// An empty formal-only group advanced without a provider transaction.
    FormalOnlyAdvanced,
}

/// Optional live managed-semantic evidence used during recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GroupRecoveryObservation {
    /// The provider cannot supply a trustworthy managed-semantic observation.
    Unavailable,
    /// Exact live managed semantics observed under the current fence.
    ManagedSemantics(ManagedSemanticSchemaFingerprint),
}

/// Fail-closed recovery decision for one positional transaction group.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupRecoveryDecision {
    /// The group is proven absent and may execute under the current fence.
    ExecuteNormally,
    /// The target is proven reached and only the journal checkpoint needs repair.
    RepairCheckpoint,
    /// Evidence is ambiguous or contradictory and requires verified operator action.
    RequiresExplicitRecovery,
}

/// Decide recovery from durable event and freshly observed managed semantics.
///
/// Equal source and target fingerprints are intentionally uninformative for
/// unknown commit outcomes. They never authorize replay or checkpoint repair.
pub fn decide_group_recovery(
    last_event: Option<GroupJournalEventKind>,
    observation: &GroupRecoveryObservation,
    source: &ManagedSemanticSchemaFingerprint,
    target: &ManagedSemanticSchemaFingerprint,
) -> GroupRecoveryDecision {
    let distinct = source != target;
    let observed = match observation {
        GroupRecoveryObservation::Unavailable => ObservedRelation::Unavailable,
        GroupRecoveryObservation::ManagedSemantics(value) if value == source && value == target => {
            ObservedRelation::Both
        }
        GroupRecoveryObservation::ManagedSemantics(value) if value == source => {
            ObservedRelation::Source
        }
        GroupRecoveryObservation::ManagedSemantics(value) if value == target => {
            ObservedRelation::Target
        }
        GroupRecoveryObservation::ManagedSemantics(_) => ObservedRelation::Neither,
    };
    match (last_event, distinct, observed) {
        (None, true, ObservedRelation::Source)
        | (Some(GroupJournalEventKind::DefinitelyAborted), true, ObservedRelation::Source)
        | (None, false, ObservedRelation::Both)
        | (
            Some(GroupJournalEventKind::DefinitelyAborted),
            false,
            ObservedRelation::Both,
        ) => GroupRecoveryDecision::ExecuteNormally,
        (
            Some(
                GroupJournalEventKind::BeforeCommit
                | GroupJournalEventKind::CommitOutcomeUnknown,
            ),
            true,
            ObservedRelation::Source,
        ) => GroupRecoveryDecision::ExecuteNormally,
        (
            Some(
                GroupJournalEventKind::BeforeCommit
                | GroupJournalEventKind::CommitOutcomeUnknown,
            ),
            true,
            ObservedRelation::Target,
        )
        | (
            Some(GroupJournalEventKind::Committed),
            true,
            ObservedRelation::Target,
        )
        | (
            Some(
                GroupJournalEventKind::Committed
                | GroupJournalEventKind::FormalOnlyAdvanced,
            ),
            false,
            ObservedRelation::Both,
        ) => GroupRecoveryDecision::RepairCheckpoint,
        _ => GroupRecoveryDecision::RequiresExplicitRecovery,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObservedRelation {
    Source,
    Target,
    Both,
    Neither,
    Unavailable,
}

/// Store-assigned monotonic ordering identity for journal entries.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct JournalSequence(u64);

impl JournalSequence {
    /// Construct a non-zero journal sequence.
    pub fn new(value: u64) -> Result<Self, Diagnostic> {
        if value == 0 {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_execution_zero_sequence",
                "journal sequence numbers must be non-zero",
            ));
        }
        Ok(Self(value))
    }

    /// Return the numeric sequence value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One record after the store atomically assigns its ordering sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalEntry<T> {
    sequence: JournalSequence,
    record: T,
}

impl<T> JournalEntry<T> {
    /// Attach a sequence allocated by the authoritative store.
    pub const fn from_store(sequence: JournalSequence, record: T) -> Self {
        Self { sequence, record }
    }

    /// Return the store ordering sequence.
    pub const fn sequence(&self) -> JournalSequence {
        self.sequence
    }

    /// Return the trusted record.
    pub const fn record(&self) -> &T {
        &self.record
    }

    /// Consume the envelope and return its record.
    pub fn into_record(self) -> T {
        self.record
    }
}

/// One trusted, still-open plan and its ordered group events.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenPlanRecord {
    plan: JournalEntry<PlanRecord>,
    events: Vec<JournalEntry<GroupEventRecord>>,
}

impl OpenPlanRecord {
    /// Rebuild store output while checking sequence, scope, fence, and manifest binding.
    ///
    /// The plan retains its original fence while recovery events may be written
    /// under later fences. Event fences therefore must be monotonic and no older
    /// than the plan fence; requiring equality would hide durable recovery work
    /// after the lease rolls forward.
    pub fn from_store(
        plan: JournalEntry<PlanRecord>,
        events: Vec<JournalEntry<GroupEventRecord>>,
    ) -> Result<Self, Diagnostic> {
        let mut previous = plan.sequence();
        let mut previous_fence = plan.record().fence();
        for event in &events {
            let manifest_index = plan
                .record()
                .manifest_digests()
                .iter()
                .position(|digest| digest == &event.record().manifest_digest());
            if event.sequence() <= previous
                || event.record().scope() != plan.record().scope()
                || event.record().fence() < previous_fence
                || manifest_index.is_none_or(|index| {
                    plan.record().migration_ids().get(index)
                        != Some(event.record().migration_id())
                })
            {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_invalid_open_plan",
                    "loaded open-plan events are not ordered and bound to the plan",
                ));
            }
            previous = event.sequence();
            previous_fence = event.record().fence();
        }
        Ok(Self { plan, events })
    }

    /// Return the sequenced plan record.
    pub const fn plan(&self) -> &JournalEntry<PlanRecord> {
        &self.plan
    }

    /// Return ordered sequenced group events.
    pub fn events(&self) -> &[JournalEntry<GroupEventRecord>] {
        &self.events
    }
}

/// Identity-only journal record for one complete verified apply plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanRecord {
    scope: ExecutionScope,
    fence: ExecutionFence,
    source_applied: Vec<MigrationId>,
    source_frontier: Vec<MigrationId>,
    target_frontier: Vec<MigrationId>,
    migration_ids: Vec<MigrationId>,
    manifest_digests: Vec<MigrationManifestDigest>,
    manifest_plan_fingerprints: Vec<MigrationPlanFingerprint>,
    source_declared: ManagedDeclaredIdentityFingerprint,
    target_declared: ManagedDeclaredIdentityFingerprint,
    source_semantics: ManagedSemanticSchemaFingerprint,
    target_semantics: ManagedSemanticSchemaFingerprint,
    semantic_profile: SemanticProfileFingerprint,
    lowering_profile: SchemaLoweringProfileFingerprint,
    observed_live_source: ManagedSemanticSchemaFingerprint,
}

impl PlanRecord {
    /// Bind a fresh-lease ledger and live-state precondition to verified plan identities.
    pub fn from_verified_plan(
        lease: &MigrationLease,
        plan: &VerifiedMigrationApplyPlan,
        observed_applied_migrations: &[MigrationId],
        observed_live_source: &ManagedSchemaState,
    ) -> Result<Self, Diagnostic> {
        let source = plan.source_state().ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_execution_empty_plan",
                "an executable migration plan requires a source state",
            )
        })?;
        let target = plan.target_state().ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_execution_empty_plan",
                "an executable migration plan requires a target state",
            )
        })?;
        let first = plan.migrations().first().ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_execution_empty_plan",
                "an executable migration plan requires at least one manifest",
            )
        })?;
        if observed_applied_migrations != plan.applied_migrations() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_stale_applied_set",
                "applied ledger changed after migration planning; rebuild the plan",
            ));
        }
        if observed_live_source != source {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_stale_source_state",
                "live managed state differs from the planned source; rebuild the plan",
            ));
        }
        let scope = ExecutionScope::new(source.scope().id().clone());
        if lease.scope() != &scope || target.scope() != source.scope() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_scope_mismatch",
                "lease, source, and target must bind the same managed scope",
            ));
        }
        let semantic_profile = first.manifest().semantic_profile().fingerprint().clone();
        let lowering_profile = first.manifest().lowering_profile().fingerprint().clone();
        for migration in plan.migrations() {
            if migration.manifest().managed_scope().id() != scope.managed_scope_id()
                || migration.manifest().semantic_profile().fingerprint()
                    != &semantic_profile
                || migration.manifest().lowering_profile().fingerprint()
                    != &lowering_profile
            {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_plan_binding_mismatch",
                    "planned manifests do not share exact scope and profile bindings",
                ));
            }
        }
        Ok(Self {
            scope,
            fence: lease.fence(),
            source_applied: plan.applied_migrations().to_vec(),
            source_frontier: plan.applied_frontier().to_vec(),
            target_frontier: plan.target_frontier().to_vec(),
            migration_ids: plan
                .migrations()
                .iter()
                .map(|migration| migration.manifest().id().clone())
                .collect(),
            manifest_digests: plan
                .migrations()
                .iter()
                .map(VerifiedMigrationApplyManifest::digest)
                .collect(),
            manifest_plan_fingerprints: plan
                .migrations()
                .iter()
                .map(|migration| migration.manifest().plan_fingerprint().clone())
                .collect(),
            source_declared: source.managed_declared_identity().clone(),
            target_declared: target.managed_declared_identity().clone(),
            source_semantics: source.managed_semantic_schema().clone(),
            target_semantics: target.managed_semantic_schema().clone(),
            semantic_profile,
            lowering_profile,
            observed_live_source: observed_live_source.managed_semantic_schema().clone(),
        })
    }

    /// Return the execution scope.
    pub const fn scope(&self) -> &ExecutionScope { &self.scope }
    /// Return the fence bound into this record.
    pub const fn fence(&self) -> ExecutionFence { self.fence }
    /// Return the planned source frontier.
    pub fn source_frontier(&self) -> &[MigrationId] { &self.source_frontier }
    /// Return the complete canonically ordered source applied set.
    pub fn source_applied(&self) -> &[MigrationId] { &self.source_applied }
    /// Return the planned target frontier.
    pub fn target_frontier(&self) -> &[MigrationId] { &self.target_frontier }
    /// Return ordered migration identities.
    pub fn migration_ids(&self) -> &[MigrationId] { &self.migration_ids }
    /// Return ordered canonical manifest digests.
    pub fn manifest_digests(&self) -> &[MigrationManifestDigest] { &self.manifest_digests }
    /// Return ordered manifest plan fingerprints.
    pub fn manifest_plan_fingerprints(&self) -> &[MigrationPlanFingerprint] {
        &self.manifest_plan_fingerprints
    }
    /// Return planned source managed-declared identity.
    pub const fn source_declared(&self) -> &ManagedDeclaredIdentityFingerprint {
        &self.source_declared
    }
    /// Return planned target managed-declared identity.
    pub const fn target_declared(&self) -> &ManagedDeclaredIdentityFingerprint {
        &self.target_declared
    }
    /// Return planned source managed semantics.
    pub const fn source_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.source_semantics
    }
    /// Return planned target managed semantics.
    pub const fn target_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.target_semantics
    }
    /// Return semantic-profile content identity.
    pub const fn semantic_profile(&self) -> &SemanticProfileFingerprint {
        &self.semantic_profile
    }
    /// Return lowering-registry content identity.
    pub const fn lowering_profile(&self) -> &SchemaLoweringProfileFingerprint {
        &self.lowering_profile
    }
    /// Return the pre-mutation observed source semantics.
    pub const fn observed_live_source(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.observed_live_source
    }
}

/// Identity-only journal event for one verified transaction group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupEventRecord {
    scope: ExecutionScope,
    fence: ExecutionFence,
    manifest_digest: MigrationManifestDigest,
    migration_id: MigrationId,
    group_ordinal: u32,
    first_step_index: u32,
    schema_delta_step_index: u32,
    end_step_index: u32,
    kind: GroupJournalEventKind,
    observed_target: Option<ManagedSemanticSchemaFingerprint>,
}

impl GroupEventRecord {
    /// Derive a positional event from exact verified apply evidence.
    pub fn new(
        lease: &MigrationLease,
        migration: &VerifiedMigrationApplyManifest,
        group: &VerifiedMigrationTransactionGroup,
        kind: GroupJournalEventKind,
        observed_target: Option<ManagedSemanticSchemaFingerprint>,
    ) -> Result<Self, Diagnostic> {
        if migration.transaction_groups().get(group.ordinal()) != Some(group) {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_foreign_group",
                "transaction group does not belong to the supplied verified manifest",
            ));
        }
        let step = migration
            .steps()
            .get(group.schema_delta_step_index())
            .ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_group_position_mismatch",
                    "transaction group delta position is outside the verified manifest",
                )
            })?;
        let delta = step
            .step()
            .as_schema_delta()
            .ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_group_position_mismatch",
                    "transaction group does not terminate in a schema delta",
                )
            })?
            .delta();
        let lowering = step.lowering().ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_execution_group_lowering_missing",
                "transaction group delta has no verified lowering",
            )
        })?;
        let scope = ExecutionScope::new(migration.manifest().managed_scope().id().clone());
        if lease.scope() != &scope {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_scope_mismatch",
                "lease scope differs from the verified migration scope",
            ));
        }
        match kind {
            GroupJournalEventKind::Committed
                if observed_target.as_ref()
                    == Some(delta.target().managed_semantic_schema()) => {}
            GroupJournalEventKind::Committed => {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_commit_evidence_mismatch",
                    "committed event requires the exact observed target semantics",
                ));
            }
            GroupJournalEventKind::FormalOnlyAdvanced
                if observed_target.is_none()
                    && group.assertion_count() == 0
                    && lowering.units().is_empty()
                    && delta.source().managed_semantic_schema()
                        == delta.target().managed_semantic_schema() => {}
            GroupJournalEventKind::FormalOnlyAdvanced => {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_execution_invalid_formal_advance",
                    "formal-only advancement requires an assertion-free empty equal-semantic group",
                ));
            }
            _ if observed_target.is_none() => {}
            _ => {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_execution_unexpected_observation",
                    "only committed events may carry observed target semantics",
                ));
            }
        }
        Ok(Self {
            scope,
            fence: lease.fence(),
            manifest_digest: migration.digest(),
            migration_id: migration.manifest().id().clone(),
            group_ordinal: position(group.ordinal())?,
            first_step_index: position(group.first_step_index())?,
            schema_delta_step_index: position(group.schema_delta_step_index())?,
            end_step_index: position(group.end_step_index())?,
            kind,
            observed_target,
        })
    }

    /// Return the execution scope.
    pub const fn scope(&self) -> &ExecutionScope { &self.scope }
    /// Return the event fence.
    pub const fn fence(&self) -> ExecutionFence { self.fence }
    /// Return the canonical manifest digest.
    pub const fn manifest_digest(&self) -> MigrationManifestDigest { self.manifest_digest }
    /// Return the migration identity.
    pub const fn migration_id(&self) -> &MigrationId { &self.migration_id }
    /// Return the group ordinal.
    pub const fn group_ordinal(&self) -> u32 { self.group_ordinal }
    /// Return the first group step index.
    pub const fn first_step_index(&self) -> u32 { self.first_step_index }
    /// Return the terminal delta step index.
    pub const fn schema_delta_step_index(&self) -> u32 { self.schema_delta_step_index }
    /// Return the exclusive group step end.
    pub const fn end_step_index(&self) -> u32 { self.end_step_index }
    /// Return the event kind.
    pub const fn kind(&self) -> GroupJournalEventKind { self.kind }
    /// Return exact target evidence carried only by committed events.
    pub const fn observed_target(&self) -> Option<&ManagedSemanticSchemaFingerprint> {
        self.observed_target.as_ref()
    }
}

/// Identity-only applied-ledger record for one verified manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedRecord {
    scope: ExecutionScope,
    fence: ExecutionFence,
    migration_id: MigrationId,
    manifest_digest: MigrationManifestDigest,
    source_declared: ManagedDeclaredIdentityFingerprint,
    target_declared: ManagedDeclaredIdentityFingerprint,
    source_semantics: ManagedSemanticSchemaFingerprint,
    target_semantics: ManagedSemanticSchemaFingerprint,
}

impl AppliedRecord {
    /// Derive an applied-ledger record from one exact verified manifest.
    pub fn from_verified_manifest(
        lease: &MigrationLease,
        migration: &VerifiedMigrationApplyManifest,
    ) -> Result<Self, Diagnostic> {
        Self::from_verified_manifest_contract(lease, migration.manifest())
    }

    /// Derive an applied-ledger record from one verified manifest contract.
    ///
    /// This is the reconstruction seam for persistent stores. The manifest
    /// remains the trust anchor: its digest is recomputed from verified bytes,
    /// and no persisted record claim enters the trusted value.
    pub fn from_verified_manifest_contract(
        lease: &MigrationLease,
        manifest: &crate::VerifiedSchemaMigrationManifest,
    ) -> Result<Self, Diagnostic> {
        let source = manifest.source_state();
        let target = manifest.target_state();
        let scope = ExecutionScope::new(source.scope().id().clone());
        if lease.scope() != &scope || target.scope() != source.scope() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_execution_scope_mismatch",
                "lease and manifest endpoints must bind the same managed scope",
            ));
        }
        Ok(Self {
            scope,
            fence: lease.fence(),
            migration_id: manifest.id().clone(),
            manifest_digest: crate::verified_manifest_digest(manifest)?,
            source_declared: source.managed_declared_identity().clone(),
            target_declared: target.managed_declared_identity().clone(),
            source_semantics: source.managed_semantic_schema().clone(),
            target_semantics: target.managed_semantic_schema().clone(),
        })
    }

    /// Return the execution scope.
    pub const fn scope(&self) -> &ExecutionScope { &self.scope }
    /// Return the record fence.
    pub const fn fence(&self) -> ExecutionFence { self.fence }
    /// Return the migration identity.
    pub const fn migration_id(&self) -> &MigrationId { &self.migration_id }
    /// Return the canonical manifest digest.
    pub const fn manifest_digest(&self) -> MigrationManifestDigest { self.manifest_digest }
    /// Return source managed-declared identity.
    pub const fn source_declared(&self) -> &ManagedDeclaredIdentityFingerprint {
        &self.source_declared
    }
    /// Return target managed-declared identity.
    pub const fn target_declared(&self) -> &ManagedDeclaredIdentityFingerprint {
        &self.target_declared
    }
    /// Return source managed semantics.
    pub const fn source_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.source_semantics
    }
    /// Return target managed semantics.
    pub const fn target_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.target_semantics
    }
}

/// Store-backed exclusive lease service.
pub trait MigrationLeaseStore: Send + Sync {
    /// Atomically acquire an available scope and issue a fence strictly greater
    /// than every fence previously issued for that scope.
    ///
    /// Lease expiry, liveness, and takeover policy are store-defined. Every
    /// takeover after release, expiry, or failure must issue a strictly greater
    /// fence so a surviving stale holder cannot mutate the journal.
    fn acquire<'a>(
        &'a self,
        scope: &'a ExecutionScope,
        holder: &'a LeaseHolderId,
    ) -> ExecutionFuture<'a, MigrationLease>;

    /// Release only the exact currently active holder and fence.
    fn release<'a>(&'a self, lease: &'a MigrationLease) -> ExecutionFuture<'a, ()>;
}

/// Durable applied ledger and open-plan journal.
///
/// Every write must atomically reject a lease that is not the current active
/// scope, holder, and fence. There is deliberately no advisory validate method.
pub trait MigrationExecutionJournal: Send + Sync {
    /// Begin one fully verified plan after stale-ledger and live-source checks.
    fn begin_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: PlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<PlanRecord>>;

    /// Append one positional commit-boundary event under the active fence.
    fn record_group_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: GroupEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<GroupEventRecord>>;

    /// Add one exact verified manifest from the open plan to the applied ledger.
    ///
    /// The store must reject a new record whose migration identity and digest do
    /// not occur at the same position in the open plan. Importing an existing
    /// ledger is a separate concern and must not use this execution write. An
    /// exact retry is idempotent only under the same fence; seeing the same
    /// migration under a newer fence requires reloading the ledger rather than
    /// treating differently fenced evidence as equal.
    fn record_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: AppliedRecord,
    ) -> ExecutionFuture<'a, JournalEntry<AppliedRecord>>;

    /// Load ordered applied records under the active fence.
    fn load_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<AppliedRecord>>>;

    /// Load the open plan and all ordered events written at its fence or later.
    fn load_open_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenPlanRecord>>;
}

fn position(value: usize) -> Result<u32, Diagnostic> {
    u32::try_from(value).map_err(|_| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "migration_execution_position_limit",
            "migration transaction position exceeds the canonical u32 range",
        )
    })
}

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static migration execution diagnostic code"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};

    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::{
        ManagedScopeId, SemanticProfileBinding,
    };
    use type_bridge_contract::migration::{
        MigrationAppLabel, MigrationName,
    };

    use super::*;
    use crate::schema_lowering_profile_binding;

    #[derive(Default)]
    struct ScopeState {
        highest_fence: u64,
        active_lease: Option<MigrationLease>,
        next_sequence: u64,
        applied: Vec<JournalEntry<AppliedRecord>>,
        open_plan: Option<JournalEntry<PlanRecord>>,
        events: Vec<JournalEntry<GroupEventRecord>>,
    }

    #[derive(Default)]
    struct InMemoryStore {
        scopes: Mutex<BTreeMap<ExecutionScope, ScopeState>>,
    }

    impl InMemoryStore {
        fn check_lease<'a>(
            state: &'a mut ScopeState,
            lease: &MigrationLease,
        ) -> Result<&'a mut ScopeState, Diagnostic> {
            if state.active_lease.as_ref() != Some(lease) {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_stale_fence",
                    "journal write does not carry the current active lease and fence",
                ));
            }
            Ok(state)
        }

        fn sequence(state: &mut ScopeState) -> Result<JournalSequence, Diagnostic> {
            state.next_sequence = state.next_sequence.checked_add(1).ok_or_else(|| {
                failure(
                    DiagnosticCategory::ResourceLimit,
                    "migration_execution_sequence_exhausted",
                    "journal sequence range is exhausted",
                )
            })?;
            JournalSequence::new(state.next_sequence)
        }
    }

    impl MigrationLeaseStore for InMemoryStore {
        fn acquire<'a>(
            &'a self,
            scope: &'a ExecutionScope,
            holder: &'a LeaseHolderId,
        ) -> ExecutionFuture<'a, MigrationLease> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.entry(scope.clone()).or_default();
                if state.active_lease.is_some() {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "migration_execution_lease_contended",
                        "migration scope already has an active lease",
                    ));
                }
                let fence = if state.highest_fence == 0 {
                    ExecutionFence::new(1)?
                } else {
                    ExecutionFence::new(state.highest_fence)?.checked_successor()?
                };
                state.highest_fence = fence.get();
                let lease = MigrationLease::new(scope.clone(), holder.clone(), fence);
                state.active_lease = Some(lease.clone());
                Ok(lease)
            })
        }

        fn release<'a>(&'a self, lease: &'a MigrationLease) -> ExecutionFuture<'a, ()> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.get_mut(lease.scope()).ok_or_else(|| {
                    failure(
                        DiagnosticCategory::Integrity,
                        "migration_execution_stale_fence",
                        "lease scope is not active",
                    )
                })?;
                Self::check_lease(state, lease)?;
                state.active_lease = None;
                Ok(())
            })
        }
    }

    impl MigrationExecutionJournal for InMemoryStore {
        fn begin_plan<'a>(
            &'a self,
            lease: &'a MigrationLease,
            record: PlanRecord,
        ) -> ExecutionFuture<'a, JournalEntry<PlanRecord>> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.get_mut(lease.scope()).ok_or_else(stale_fence)?;
                Self::check_lease(state, lease)?;
                if record.scope() != lease.scope() || record.fence() != lease.fence() {
                    return Err(stale_fence());
                }
                if state.open_plan.is_some() {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "migration_execution_plan_already_open",
                        "migration scope already has an open plan",
                    ));
                }
                let entry = JournalEntry::from_store(Self::sequence(state)?, record);
                state.open_plan = Some(entry.clone());
                Ok(entry)
            })
        }

        fn record_group_event<'a>(
            &'a self,
            lease: &'a MigrationLease,
            record: GroupEventRecord,
        ) -> ExecutionFuture<'a, JournalEntry<GroupEventRecord>> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.get_mut(lease.scope()).ok_or_else(stale_fence)?;
                Self::check_lease(state, lease)?;
                if record.scope() != lease.scope() || record.fence() != lease.fence() {
                    return Err(stale_fence());
                }
                let plan = state.open_plan.as_ref().ok_or_else(|| {
                    failure(
                        DiagnosticCategory::InvalidContract,
                        "migration_execution_no_open_plan",
                        "group event requires an open migration plan",
                    )
                })?;
                if !plan
                    .record()
                    .manifest_digests()
                    .contains(&record.manifest_digest())
                {
                    return Err(failure(
                        DiagnosticCategory::Integrity,
                        "migration_execution_foreign_event",
                        "group event manifest is absent from the open plan",
                    ));
                }
                let entry = JournalEntry::from_store(Self::sequence(state)?, record);
                state.events.push(entry.clone());
                Ok(entry)
            })
        }

        fn record_applied<'a>(
            &'a self,
            lease: &'a MigrationLease,
            record: AppliedRecord,
        ) -> ExecutionFuture<'a, JournalEntry<AppliedRecord>> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.get_mut(lease.scope()).ok_or_else(stale_fence)?;
                Self::check_lease(state, lease)?;
                if record.scope() != lease.scope() || record.fence() != lease.fence() {
                    return Err(stale_fence());
                }
                if let Some(existing) = state
                    .applied
                    .iter()
                    .find(|entry| entry.record().migration_id() == record.migration_id())
                {
                    return if existing.record() == &record {
                        Ok(existing.clone())
                    } else {
                        Err(failure(
                            DiagnosticCategory::Integrity,
                            "migration_execution_applied_identity_conflict",
                            "applied migration identity has different evidence",
                        ))
                    };
                }
                let plan = state.open_plan.as_ref().ok_or_else(|| {
                    failure(
                        DiagnosticCategory::InvalidContract,
                        "migration_execution_no_open_plan",
                        "applied migration requires an open migration plan",
                    )
                })?;
                let manifest_index = plan
                    .record()
                    .migration_ids()
                    .iter()
                    .position(|id| id == record.migration_id());
                if manifest_index.is_none_or(|index| {
                    plan.record().manifest_digests().get(index)
                        != Some(&record.manifest_digest())
                }) {
                    return Err(failure(
                        DiagnosticCategory::Integrity,
                        "migration_execution_foreign_applied_record",
                        "applied migration identity and digest are absent from the open plan",
                    ));
                }
                let entry = JournalEntry::from_store(Self::sequence(state)?, record);
                state.applied.push(entry.clone());
                let complete = state.open_plan.as_ref().is_some_and(|plan| {
                    plan.record().migration_ids().iter().all(|id| {
                        state
                            .applied
                            .iter()
                            .any(|applied| applied.record().migration_id() == id)
                    })
                });
                if complete {
                    state.open_plan = None;
                    state.events.clear();
                }
                Ok(entry)
            })
        }

        fn load_applied<'a>(
            &'a self,
            lease: &'a MigrationLease,
        ) -> ExecutionFuture<'a, Vec<JournalEntry<AppliedRecord>>> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.get_mut(lease.scope()).ok_or_else(stale_fence)?;
                Self::check_lease(state, lease)?;
                Ok(state.applied.clone())
            })
        }

        fn load_open_plan<'a>(
            &'a self,
            lease: &'a MigrationLease,
        ) -> ExecutionFuture<'a, Option<OpenPlanRecord>> {
            Box::pin(async move {
                let mut scopes = self.scopes.lock().expect("store mutex");
                let state = scopes.get_mut(lease.scope()).ok_or_else(stale_fence)?;
                Self::check_lease(state, lease)?;
                let Some(plan) = state.open_plan.clone() else { return Ok(None) };
                Ok(Some(OpenPlanRecord::from_store(plan, state.events.clone())?))
            })
        }
    }

    fn stale_fence() -> Diagnostic {
        failure(
            DiagnosticCategory::Integrity,
            "migration_execution_stale_fence",
            "journal write does not carry the current active lease and fence",
        )
    }

    fn scope() -> ExecutionScope {
        ExecutionScope::new(ManagedScopeId::new("journal-test").expect("scope"))
    }

    fn migration_id() -> MigrationId {
        MigrationId::from_components(
            MigrationAppLabel::new("example").expect("app"),
            MigrationName::new("0001_initial").expect("name"),
        )
    }

    fn semantic_fingerprint(bytes: &[u8]) -> ManagedSemanticSchemaFingerprint {
        ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            bytes,
        )
        .expect("semantic fingerprint")
    }

    fn fake_plan(lease: &MigrationLease) -> PlanRecord {
        let id = migration_id();
        let semantic = SemanticProfileBinding::resolve(
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
        )
        .expect("semantic binding");
        PlanRecord {
            scope: lease.scope().clone(),
            fence: lease.fence(),
            source_applied: Vec::new(),
            source_frontier: Vec::new(),
            target_frontier: vec![id.clone()],
            migration_ids: vec![id],
            manifest_digests: vec![MigrationManifestDigest::compute(b"manifest")],
            manifest_plan_fingerprints: vec![
                MigrationPlanFingerprint::compute(&[]).expect("plan fingerprint"),
            ],
            source_declared: ManagedDeclaredIdentityFingerprint::compute(b"source")
                .expect("source declared"),
            target_declared: ManagedDeclaredIdentityFingerprint::compute(b"target")
                .expect("target declared"),
            source_semantics: semantic_fingerprint(b"source"),
            target_semantics: semantic_fingerprint(b"target"),
            semantic_profile: semantic.fingerprint().clone(),
            lowering_profile: schema_lowering_profile_binding()
                .expect("lowering binding")
                .fingerprint()
                .clone(),
            observed_live_source: semantic_fingerprint(b"source"),
        }
    }

    fn fake_event(lease: &MigrationLease, plan: &PlanRecord) -> GroupEventRecord {
        GroupEventRecord {
            scope: lease.scope().clone(),
            fence: lease.fence(),
            manifest_digest: plan.manifest_digests()[0],
            migration_id: plan.migration_ids()[0].clone(),
            group_ordinal: 0,
            first_step_index: 0,
            schema_delta_step_index: 0,
            end_step_index: 1,
            kind: GroupJournalEventKind::BeforeCommit,
            observed_target: None,
        }
    }

    fn fake_applied(lease: &MigrationLease, plan: &PlanRecord) -> AppliedRecord {
        AppliedRecord {
            scope: lease.scope().clone(),
            fence: lease.fence(),
            migration_id: plan.migration_ids()[0].clone(),
            manifest_digest: plan.manifest_digests()[0],
            source_declared: plan.source_declared().clone(),
            target_declared: plan.target_declared().clone(),
            source_semantics: plan.source_semantics().clone(),
            target_semantics: plan.target_semantics().clone(),
        }
    }

    #[test]
    fn store_assigns_monotonic_sequences_and_rejects_every_stale_fence_write() {
        let store = InMemoryStore::default();
        let scope = scope();
        let holder_a = LeaseHolderId::new("owner-a").expect("holder");
        let holder_b = LeaseHolderId::new("owner-b").expect("holder");
        let lease_a = block_on(store.acquire(&scope, &holder_a)).expect("lease a");
        assert!(block_on(store.acquire(&scope, &holder_b)).is_err());
        let plan_a = fake_plan(&lease_a);
        let plan_entry = block_on(store.begin_plan(&lease_a, plan_a.clone())).expect("plan");
        let event_entry = block_on(store.record_group_event(
            &lease_a,
            fake_event(&lease_a, &plan_a),
        ))
        .expect("event");
        assert_eq!(plan_entry.sequence().get(), 1);
        assert_eq!(event_entry.sequence().get(), 2);
        block_on(store.release(&lease_a)).expect("release a");
        let lease_b = block_on(store.acquire(&scope, &holder_b)).expect("lease b");
        assert!(lease_b.fence() > lease_a.fence());
        assert!(block_on(store.begin_plan(&lease_a, plan_a.clone())).is_err());
        assert!(block_on(store.record_group_event(
            &lease_a,
            fake_event(&lease_a, &plan_a),
        ))
        .is_err());
        assert!(block_on(store.record_applied(
            &lease_a,
            fake_applied(&lease_a, &plan_a),
        ))
        .is_err());
        assert!(block_on(store.release(&lease_a)).is_err());
        let recovery_event = block_on(store.record_group_event(
            &lease_b,
            fake_event(&lease_b, &plan_a),
        ))
        .expect("recovery event");
        assert_eq!(recovery_event.sequence().get(), 3);
        block_on(store.release(&lease_b)).expect("release recovery lease");
        let holder_c = LeaseHolderId::new("owner-c").expect("holder");
        let lease_c = block_on(store.acquire(&scope, &holder_c)).expect("lease c");
        assert!(lease_c.fence() > lease_b.fence());
        let open = block_on(store.load_open_plan(&lease_c))
            .expect("open plan")
            .expect("plan remains open after recovery crash");
        assert_eq!(open.events().len(), 2);
        assert_eq!(open.events()[0].record().fence(), lease_a.fence());
        assert_eq!(open.events()[1].record().fence(), lease_b.fence());
    }

    #[test]
    fn applied_records_are_plan_bound_idempotent_and_close_the_completed_plan() {
        let store = InMemoryStore::default();
        let scope = scope();
        let holder = LeaseHolderId::new("applied-owner").expect("holder");
        let lease = block_on(store.acquire(&scope, &holder)).expect("lease");
        let plan = fake_plan(&lease);
        let applied = fake_applied(&lease, &plan);

        assert!(block_on(store.record_applied(&lease, applied.clone())).is_err());
        block_on(store.begin_plan(&lease, plan.clone())).expect("open plan");

        let mut foreign = applied.clone();
        foreign.manifest_digest = MigrationManifestDigest::compute(b"foreign");
        assert!(block_on(store.record_applied(&lease, foreign)).is_err());

        let first = block_on(store.record_applied(&lease, applied.clone()))
            .expect("apply planned manifest");
        let duplicate = block_on(store.record_applied(&lease, applied))
            .expect("same-fence duplicate is idempotent");
        assert_eq!(duplicate, first);
        assert!(block_on(store.load_open_plan(&lease))
            .expect("load completed plan")
            .is_none());
        assert_eq!(
            block_on(store.load_applied(&lease)).expect("load applied ledger"),
            vec![first],
        );
    }

    #[test]
    fn recovery_decision_table_is_exhaustive_for_distinct_and_equal_semantics() {
        let source = semantic_fingerprint(b"source");
        let target = semantic_fingerprint(b"target");
        let neither = semantic_fingerprint(b"neither");
        let events = [
            None,
            Some(GroupJournalEventKind::BeforeCommit),
            Some(GroupJournalEventKind::Committed),
            Some(GroupJournalEventKind::CommitOutcomeUnknown),
            Some(GroupJournalEventKind::DefinitelyAborted),
            Some(GroupJournalEventKind::FormalOnlyAdvanced),
        ];
        let observations = [
            GroupRecoveryObservation::ManagedSemantics(source.clone()),
            GroupRecoveryObservation::ManagedSemantics(target.clone()),
            GroupRecoveryObservation::ManagedSemantics(neither.clone()),
            GroupRecoveryObservation::Unavailable,
        ];
        for event in events {
            for observation in &observations {
                let expected = expected_distinct(event, observation, &source, &target);
                assert_eq!(
                    decide_group_recovery(event, observation, &source, &target),
                    expected,
                    "distinct case {event:?} {observation:?}",
                );
            }
        }
        let equal_observations = [
            GroupRecoveryObservation::ManagedSemantics(source.clone()),
            GroupRecoveryObservation::ManagedSemantics(neither),
            GroupRecoveryObservation::Unavailable,
        ];
        for event in events {
            for observation in &equal_observations {
                let expected = expected_equal(event, observation, &source);
                assert_eq!(
                    decide_group_recovery(event, observation, &source, &source),
                    expected,
                    "equal case {event:?} {observation:?}",
                );
            }
        }
    }

    fn expected_distinct(
        event: Option<GroupJournalEventKind>,
        observation: &GroupRecoveryObservation,
        source: &ManagedSemanticSchemaFingerprint,
        target: &ManagedSemanticSchemaFingerprint,
    ) -> GroupRecoveryDecision {
        let source_seen = matches!(observation, GroupRecoveryObservation::ManagedSemantics(value) if value == source);
        let target_seen = matches!(observation, GroupRecoveryObservation::ManagedSemantics(value) if value == target);
        match event {
            None | Some(GroupJournalEventKind::DefinitelyAborted) if source_seen => {
                GroupRecoveryDecision::ExecuteNormally
            }
            Some(
                GroupJournalEventKind::BeforeCommit
                | GroupJournalEventKind::CommitOutcomeUnknown,
            ) if source_seen => GroupRecoveryDecision::ExecuteNormally,
            Some(
                GroupJournalEventKind::BeforeCommit
                | GroupJournalEventKind::CommitOutcomeUnknown
                | GroupJournalEventKind::Committed,
            ) if target_seen => GroupRecoveryDecision::RepairCheckpoint,
            _ => GroupRecoveryDecision::RequiresExplicitRecovery,
        }
    }

    fn expected_equal(
        event: Option<GroupJournalEventKind>,
        observation: &GroupRecoveryObservation,
        both: &ManagedSemanticSchemaFingerprint,
    ) -> GroupRecoveryDecision {
        let both_seen = matches!(observation, GroupRecoveryObservation::ManagedSemantics(value) if value == both);
        if !both_seen {
            return GroupRecoveryDecision::RequiresExplicitRecovery;
        }
        match event {
            None | Some(GroupJournalEventKind::DefinitelyAborted) => {
                GroupRecoveryDecision::ExecuteNormally
            }
            Some(
                GroupJournalEventKind::Committed
                | GroupJournalEventKind::FormalOnlyAdvanced,
            ) => GroupRecoveryDecision::RepairCheckpoint,
            _ => GroupRecoveryDecision::RequiresExplicitRecovery,
        }
    }

    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }
}
