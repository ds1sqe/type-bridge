//! Directory-to-database apply orchestration over one TypeDB pair.
//!
//! The runner is the first non-test caller of the verified execution stack:
//! it discovers and replay-verifies the canonical migration chain on disk,
//! reads the applied basis from the authoritative journal database under a
//! lease, derives the apply plan offline, and executes it through the fenced
//! store and provider. It adds no semantics of its own — every decision is
//! delegated to the discovery, planning, and coordination layers it wires.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, ManagedSchemaState};
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{CommitFailureCertainty, Database, OrmError};
use type_bridge_schema::{DeltaError, ManagedDeltaContext, managed_schema_state};
use type_bridge_schema_migration::{
    AppliedRecord, CanonicalMigrationHistoryEvidence, ExecutionFuture, ExecutionScope,
    GroupEventRecord, JournalEntry, LeaseHolderId, MigrationApplyApproval, MigrationApplyPlanError,
    MigrationApplyTarget, MigrationDirectory, MigrationExecutionJournal, MigrationExecutionOutcome,
    MigrationHistoryGraph, MigrationLease, MigrationLeaseStore, MigrationRollbackOutcome,
    MigrationSafetyPolicy, MigrationVerifyReport, OpenPlanRecord, OpenRollbackPlanRecord,
    PlanRecord, RollbackPlanRecord, RollbackStepEventRecord, RolledBackRecord,
    SchemaLoweringBinding, VerifiedMigrationApplyPlan, VerifiedSchemaMigrationManifest,
    build_verified_migration_apply_plan, build_verified_migration_rollback_plan,
    canonical_history_declared_legacy_bridge_count_in, discover_verified_migration_chain_in,
    discover_verified_migration_chain_with_evidence_in, execute_verified_migration_apply_plan,
    execute_verified_migration_rollback_plan, require_adoption_authority_pair,
    require_adoption_authority_pair_state, verified_manifest_digest, verify_migration_state,
};

use type_bridge_migration::{
    AppliedMigrationRecord, LegacyAdoptionHistory, LegacyCutoverSentinelError,
    LegacyCutoverSentinelExpectation, TypeDbStateStore, VerifiedLegacyAppliedPartition,
    VerifiedLegacyHead, load_adoption_history, reconstruct_legacy_head,
};
use type_bridge_schema_compat::{
    ADOPTED_GENESIS_FILE_NAME, AdoptedGenesisAuthority, MAX_TYPEQL_SCHEMA_BYTES,
    parse_adopted_genesis_authority, parse_adopted_genesis_authority_with_internal,
    released_typeql_to_declared_projection,
};

use crate::control_schema::{
    LEGACY_CUTOVER_ENTITY, LEGACY_CUTOVER_FINGERPRINT, LEGACY_CUTOVER_KEY, LEGACY_CUTOVER_SCOPE,
    LEGACY_CUTOVER_SINGLETON_KEY, MANAGED_FENCE_SCHEMA_TYPEQL,
};
use crate::legacy_import::{
    digest_legacy_applied_records, extract_legacy_applied_set_digest, extract_legacy_frontier,
    verify_legacy_continuity,
};
use crate::observation::{
    ManagedObservationAuthority, observe_managed_state_from_export_with_authority,
    rebuild_live_managed_state_with_authority,
};
use crate::provider::{TypeDbMigrationProvider, execution_capability_vocabulary};
use crate::store::{TypeDbMigrationStore, VerifiedMigrationCatalog, require_active_managed_fence};

/// Result of one directory apply pass.
#[derive(Debug)]
pub enum MigrationDirectoryApplyOutcome {
    /// The applied ledger already covers the requested target frontier.
    UpToDate,
    /// The coordinator executed the derived plan and reported this outcome.
    Executed(MigrationExecutionOutcome),
}

/// Result of one directory rollback pass.
#[derive(Debug)]
pub enum MigrationDirectoryRollbackOutcome {
    /// None of the requested removals is active in the applied ledger.
    UpToDate,
    /// The coordinator executed the derived rollback and reported this outcome.
    Executed(MigrationRollbackOutcome),
}

/// Failure while orchestrating one directory apply pass.
#[derive(Debug)]
pub enum MigrationDirectoryApplyError {
    /// Discovery, storage, provider, or execution failed with a diagnostic.
    Diagnostic(Diagnostic),
    /// Offline plan derivation failed before any provider write.
    Plan(MigrationApplyPlanError),
}

impl fmt::Display for MigrationDirectoryApplyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(error) => write!(formatter, "{error}"),
            Self::Plan(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for MigrationDirectoryApplyError {}

impl From<Diagnostic> for MigrationDirectoryApplyError {
    fn from(value: Diagnostic) -> Self {
        Self::Diagnostic(value)
    }
}

impl From<MigrationApplyPlanError> for MigrationDirectoryApplyError {
    fn from(value: MigrationApplyPlanError) -> Self {
        Self::Plan(value)
    }
}

/// Apply orchestrator bound to one managed/journal database pair.
pub struct TypeDbMigrationRunner {
    managed_database: Arc<Database>,
    journal_database: Arc<Database>,
    genesis_source: DeclaredSchema,
    context: ManagedDeltaContext,
    lowering_binding: SchemaLoweringBinding,
    policy: MigrationSafetyPolicy,
}

/// Managed-side binding that every bridge-rooted provider and terminal
/// journal transaction must revalidate.
#[derive(Clone)]
pub(crate) enum LegacyExecutionBinding {
    /// No active bridge exists, so no cutover anchor may exist either.
    Absent,
    /// One active bridge permanently binds the V1 ledger and exact anchor.
    Active(LegacyBridgeBinding),
}

#[derive(Clone)]
pub(crate) struct LegacyBridgeBinding {
    expected_applied_set: type_bridge_schema_migration::LegacyAppliedSetDigest,
    managed_scope: String,
    anchor_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LegacyBindingSnapshot {
    applied_set: Option<type_bridge_schema_migration::LegacyAppliedSetDigest>,
    anchor_fingerprint: Option<String>,
    sentinel_fingerprint: Option<String>,
}

#[derive(Debug)]
struct LegacyBindingReadInspection {
    drift: Option<Diagnostic>,
    snapshot: Option<LegacyBindingSnapshot>,
}

impl LegacyExecutionBinding {
    fn from_applied_graph(
        graph: &MigrationHistoryGraph,
        applied_basis: &BTreeSet<MigrationId>,
        authority: Option<&AdoptedGenesisAuthority>,
        managed_database: &Database,
        journal_database: &Database,
        managed_scope: &str,
    ) -> Result<Self, Diagnostic> {
        let Some((bridge_id, bridge)) = graph
            .manifests()
            .find(|(_, manifest)| manifest.is_legacy_bridge())
        else {
            return Ok(Self::Absent);
        };
        if !applied_basis.contains(bridge_id) {
            return Ok(Self::Absent);
        }
        let authority = authority.ok_or_else(|| {
            legacy_import_failure(
                "migration_legacy_adopted_genesis_missing",
                "an active legacy bridge requires the retained adopted-genesis authority",
            )
        })?;
        Ok(Self::Active(LegacyBridgeBinding::from_bridge(
            bridge,
            authority,
            managed_database,
            journal_database,
            managed_scope,
        )?))
    }

    pub(crate) async fn validate_contents(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
        managed_scope: &str,
    ) -> Result<(), Diagnostic> {
        match self {
            Self::Absent => {
                load_verified_legacy_partition(
                    transaction,
                    LegacyCutoverSentinelExpectation::Absent,
                    "bridge-free execution cannot inspect the released applied ledger",
                )
                .await?;
                if load_legacy_cutover_anchor(transaction, managed_scope)
                    .await?
                    .is_some()
                {
                    return Err(legacy_import_failure(
                        "migration_legacy_import_anchor_without_bridge",
                        "managed database carries a cutover anchor without an active bridge",
                    ));
                }
                Ok(())
            }
            Self::Active(binding) => binding.validate_exact_contents(transaction).await,
        }
    }

    async fn inspect_read_only(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
        managed_scope: &str,
    ) -> Result<LegacyBindingReadInspection, Diagnostic> {
        match self {
            Self::Absent => {
                if let Err(error) = load_verified_legacy_partition(
                    transaction,
                    LegacyCutoverSentinelExpectation::Absent,
                    "bridge-free read-only verification cannot inspect the released ledger",
                )
                .await
                {
                    if error.code().as_str() == "migration_legacy_cutover_sentinel_invalid" {
                        return Ok(LegacyBindingReadInspection {
                            drift: Some(error),
                            snapshot: None,
                        });
                    }
                    return Err(error);
                }
                match observe_legacy_cutover_anchor(transaction, managed_scope).await {
                    Ok(anchor_fingerprint) => {
                        let drift = anchor_fingerprint.as_ref().map(|_| {
                            legacy_import_failure(
                                "migration_legacy_import_anchor_without_bridge",
                                "managed database carries a cutover anchor without an active bridge",
                            )
                        });
                        Ok(LegacyBindingReadInspection {
                            drift,
                            snapshot: Some(LegacyBindingSnapshot {
                                applied_set: None,
                                anchor_fingerprint,
                                sentinel_fingerprint: None,
                            }),
                        })
                    }
                    Err(LegacyAnchorObservationError::Drift(diagnostic)) => {
                        Ok(LegacyBindingReadInspection {
                            drift: Some(diagnostic),
                            snapshot: None,
                        })
                    }
                    Err(LegacyAnchorObservationError::Infrastructure(diagnostic)) => {
                        Err(diagnostic)
                    }
                }
            }
            Self::Active(binding) => binding.inspect_read_only(transaction).await,
        }
    }
}

impl LegacyBridgeBinding {
    fn from_bridge(
        bridge: &VerifiedSchemaMigrationManifest,
        authority: &AdoptedGenesisAuthority,
        managed_database: &Database,
        journal_database: &Database,
        managed_scope: &str,
    ) -> Result<Self, Diagnostic> {
        if !bridge.is_legacy_bridge() {
            return Err(legacy_import_failure(
                "migration_legacy_import_guard_plan_mismatch",
                "legacy binding requires the verified bridge manifest",
            ));
        }
        let expected_applied_set = bridge.legacy_applied_set().cloned().ok_or_else(|| {
            legacy_import_failure(
                "migration_legacy_applied_set_missing",
                "verified legacy bridge has no complete applied-set binding",
            )
        })?;
        let anchor_fingerprint = legacy_cutover_fingerprint(
            &expected_applied_set,
            &verified_manifest_digest(bridge)?.to_hex(),
            managed_database.database_name(),
            journal_database.database_name(),
            managed_scope,
            authority.legacy_identity(),
        )?;
        Ok(Self {
            expected_applied_set,
            managed_scope: managed_scope.to_owned(),
            anchor_fingerprint,
        })
    }

    async fn validate_legacy_ledger(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
    ) -> Result<(), Diagnostic> {
        let current = load_verified_legacy_partition(
            transaction,
            LegacyCutoverSentinelExpectation::RequiredExact(&self.anchor_fingerprint),
            "bridge-rooted execution cannot read the released applied ledger",
        )
        .await?;
        let observed = digest_legacy_applied_records(current.applied())?;
        if observed != self.expected_applied_set {
            return Err(legacy_import_failure(
                "migration_legacy_applied_set_drift",
                "released applied ledger differs from the permanent bridge binding",
            ));
        }
        Ok(())
    }

    async fn validate_exact_anchor(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
    ) -> Result<(), Diagnostic> {
        match load_legacy_cutover_anchor(transaction, &self.managed_scope).await? {
            Some(observed) if observed == self.anchor_fingerprint => Ok(()),
            Some(_) => Err(legacy_import_failure(
                "migration_legacy_import_anchor_mismatch",
                "managed database carries a different legacy cutover anchor",
            )),
            None => Err(legacy_import_failure(
                "migration_legacy_import_anchor_missing",
                "active legacy bridge has no managed-side cutover anchor",
            )),
        }
    }

    async fn validate_exact_contents(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
    ) -> Result<(), Diagnostic> {
        self.validate_legacy_ledger(transaction).await?;
        self.validate_exact_anchor(transaction).await
    }

    async fn validate_pending_import_contents(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
    ) -> Result<VerifiedLegacyAppliedPartition, Diagnostic> {
        let current = load_verified_legacy_partition(
            transaction,
            LegacyCutoverSentinelExpectation::OptionalExact(&self.anchor_fingerprint),
            "pending legacy import cannot read the released applied ledger",
        )
        .await?;
        let observed = digest_legacy_applied_records(current.applied())?;
        if observed != self.expected_applied_set {
            return Err(legacy_import_failure(
                "migration_legacy_applied_set_drift",
                "released applied ledger differs from the pending bridge binding",
            ));
        }
        let anchor = load_legacy_cutover_anchor(transaction, &self.managed_scope).await?;
        let sentinel = current.sentinel_fingerprint().map(str::to_owned);
        match (sentinel.as_deref(), anchor.as_deref()) {
            (None, None) => Ok(current),
            (Some(sentinel), Some(anchor))
                if sentinel == self.anchor_fingerprint && anchor == self.anchor_fingerprint =>
            {
                Ok(current)
            }
            (Some(_), None) | (None, Some(_)) => Err(legacy_import_failure(
                "migration_legacy_import_cutover_pair_incomplete",
                "managed cutover anchor and legacy-writer sentinel must appear atomically",
            )),
            (Some(_), Some(_)) => Err(legacy_import_failure(
                "migration_legacy_import_anchor_mismatch",
                "managed database carries a different legacy cutover anchor pair",
            )),
        }
    }

    async fn inspect_read_only(
        &self,
        transaction: &mut type_bridge_orm::Transaction,
    ) -> Result<LegacyBindingReadInspection, Diagnostic> {
        let current = match load_verified_legacy_partition(
            transaction,
            LegacyCutoverSentinelExpectation::RequiredExact(&self.anchor_fingerprint),
            "read-only verification cannot read the released applied ledger",
        )
        .await
        {
            Ok(current) => current,
            Err(error) if error.code().as_str() == "migration_legacy_cutover_sentinel_invalid" => {
                return Ok(LegacyBindingReadInspection {
                    drift: Some(error),
                    snapshot: None,
                });
            }
            Err(error) => return Err(error),
        };
        let observed = match digest_legacy_applied_records(current.applied()) {
            Ok(observed) => observed,
            Err(drift) => {
                return Ok(LegacyBindingReadInspection {
                    drift: Some(drift),
                    snapshot: None,
                });
            }
        };
        let anchor_fingerprint =
            match observe_legacy_cutover_anchor(transaction, &self.managed_scope).await {
                Ok(anchor_fingerprint) => anchor_fingerprint,
                Err(LegacyAnchorObservationError::Drift(diagnostic)) => {
                    return Ok(LegacyBindingReadInspection {
                        drift: Some(diagnostic),
                        snapshot: None,
                    });
                }
                Err(LegacyAnchorObservationError::Infrastructure(diagnostic)) => {
                    return Err(diagnostic);
                }
            };
        let drift = if observed != self.expected_applied_set {
            Some(legacy_import_failure(
                "migration_legacy_applied_set_drift",
                "released applied ledger differs from the permanent bridge binding",
            ))
        } else {
            match anchor_fingerprint.as_deref() {
                Some(anchor) if anchor == self.anchor_fingerprint => None,
                Some(_) => Some(legacy_import_failure(
                    "migration_legacy_import_anchor_mismatch",
                    "managed database carries a different legacy cutover anchor",
                )),
                None => Some(legacy_import_failure(
                    "migration_legacy_import_anchor_missing",
                    "active legacy bridge has no managed-side cutover anchor",
                )),
            }
        };
        Ok(LegacyBindingReadInspection {
            drift,
            snapshot: Some(LegacyBindingSnapshot {
                applied_set: Some(observed),
                anchor_fingerprint,
                sentinel_fingerprint: current.sentinel_fingerprint().map(str::to_owned),
            }),
        })
    }
}

/// Journal adapter that makes the zero-operation legacy bridge checkpoint
/// anchor-first crash-recoverable and exclusive with released V1 writers on
/// the managed database.
///
/// TypeDB schema transactions exclude both schema and write transactions. The
/// adapter is installed only for one verified bridge plan and intercepts its
/// terminal `record_applied`: after the generic coordinator has acquired the
/// V2 lease and published its managed fence, it holds a schema transaction
/// while re-reading V1 data, re-exporting committed schema, and durably
/// appending the companion-journal checkpoint.
struct LegacyCheckpointStore<'a> {
    inner: &'a TypeDbMigrationStore<'a>,
    managed_database: &'a Database,
    canonical_directory: &'a MigrationDirectory,
    canonical_evidence: &'a CanonicalMigrationHistoryEvidence,
    legacy_history: &'a LegacyAdoptionHistory,
    reconstructed: &'a VerifiedLegacyHead,
    reconstructed_authority: &'a AdoptedGenesisAuthority,
    expected_internal: DeclaredSchema,
    bridge_id: MigrationId,
    binding: LegacyBridgeBinding,
}

struct LegacyCheckpointAuthorities<'a> {
    canonical_directory: &'a MigrationDirectory,
    canonical_evidence: &'a CanonicalMigrationHistoryEvidence,
    legacy_history: &'a LegacyAdoptionHistory,
    reconstructed: &'a VerifiedLegacyHead,
    reconstructed_authority: &'a AdoptedGenesisAuthority,
}

impl<'a> LegacyCheckpointStore<'a> {
    fn new(
        inner: &'a TypeDbMigrationStore<'a>,
        managed_database: &'a Database,
        journal_database: &'a Database,
        authorities: LegacyCheckpointAuthorities<'a>,
        plan: &'a VerifiedMigrationApplyPlan,
    ) -> Result<Self, Diagnostic> {
        let [migration] = plan.migrations() else {
            return Err(legacy_import_failure(
                "migration_legacy_import_guard_plan_mismatch",
                "legacy checkpoint guard requires exactly one bridge manifest",
            ));
        };
        if !migration.manifest().is_legacy_bridge()
            || !migration.steps().is_empty()
            || !migration.transaction_groups().is_empty()
            || plan.source_state() != plan.target_state()
            || plan.source_schema() != plan.target_schema()
        {
            return Err(legacy_import_failure(
                "migration_legacy_import_guard_plan_mismatch",
                "legacy checkpoint guard accepts only one verified zero-operation bridge",
            ));
        }
        let managed_scope = plan
            .source_state()
            .expect("validated bridge plan has a source state")
            .scope()
            .id()
            .as_str()
            .to_owned();
        let binding = LegacyBridgeBinding::from_bridge(
            migration.manifest(),
            authorities.reconstructed_authority,
            managed_database,
            journal_database,
            &managed_scope,
        )?;
        Ok(Self {
            inner,
            managed_database,
            canonical_directory: authorities.canonical_directory,
            canonical_evidence: authorities.canonical_evidence,
            legacy_history: authorities.legacy_history,
            reconstructed: authorities.reconstructed,
            reconstructed_authority: authorities.reconstructed_authority,
            expected_internal: expected_managed_internal_schema()?,
            bridge_id: migration.manifest().id().clone(),
            binding,
        })
    }

    fn authority_changed(&self, error: impl fmt::Display) -> Diagnostic {
        legacy_import_failure(
            "migration_legacy_import_authority_changed",
            "legacy migration authority changed at the guarded checkpoint boundary",
        )
        .with_detail("legacy", error.to_string())
    }

    fn canonical_authority_changed(
        &self,
        phase: &'static str,
        error: impl fmt::Display,
    ) -> Diagnostic {
        legacy_import_failure(
            "migration_legacy_import_canonical_authority_changed",
            "canonical migration authority changed at the guarded checkpoint boundary",
        )
        .with_detail("phase", phase)
        .with_detail("canonical", error.to_string())
    }

    fn require_canonical_authority_unchanged(&self, phase: &'static str) -> Result<(), Diagnostic> {
        self.canonical_evidence
            .require_unchanged(self.canonical_directory)
            .map_err(|error| self.canonical_authority_changed(phase, error))
    }

    fn require_postpublication_authorities_unchanged(&self) -> Result<(), Diagnostic> {
        // Canonical authority is checked first so this remains the immediate
        // operation after companion-journal publication.
        let canonical = self.require_canonical_authority_unchanged("after_journal_publication");
        let legacy = self
            .legacy_history
            .require_unchanged_head(self.reconstructed)
            .map_err(|error| self.authority_changed(error));
        match (canonical, legacy) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(canonical), Err(legacy)) => Err(combine_diagnostics(
                "migration_legacy_import_canonical_and_legacy_authority_changed",
                "canonical and legacy filesystem authorities both changed after checkpoint publication",
                canonical,
                legacy,
            )),
        }
    }

    async fn validate_guarded_authority(
        &self,
        guard: &mut type_bridge_orm::Transaction,
        lease: &MigrationLease,
        sentinel_expectation: LegacyCutoverSentinelExpectation<'_>,
    ) -> Result<(), Diagnostic> {
        require_active_managed_fence(guard, lease).await?;
        let applied = load_verified_legacy_partition(
            guard,
            sentinel_expectation,
            "legacy applied ledger cannot be read through the checkpoint guard",
        )
        .await?;
        verify_legacy_continuity(self.legacy_history.graph(), applied.applied())?;
        let observed = digest_legacy_applied_records(applied.applied())?;
        if observed != self.binding.expected_applied_set {
            return Err(legacy_import_failure(
                "migration_legacy_applied_set_drift",
                "guarded legacy ledger differs from the bridge applied-set binding",
            ));
        }

        let export = self.managed_database.schema_text().await.map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_live_export_failed",
                "managed schema cannot be re-exported through the checkpoint guard",
            )
            .with_detail("provider", error.to_string())
        })?;
        require_active_managed_fence(guard, lease).await?;
        let live_authority = parse_adopted_genesis_authority_with_internal(
            DocumentId::new("typebridge-legacy-guarded-live-head.typeql")?,
            &export,
            Some(&self.expected_internal),
        )?;
        if !same_adopted_authority(self.reconstructed_authority, &live_authority) {
            return Err(legacy_import_failure(
                "migration_legacy_import_live_drift",
                "live managed schema changed before the guarded legacy checkpoint",
            ));
        }
        self.legacy_history
            .require_unchanged_head(self.reconstructed)
            .map_err(|error| self.authority_changed(error))?;

        // `schema_text()` and the retained filesystem observation above are
        // asynchronous authority boundaries. Re-read the complete released
        // ledger only after both have finished, through the same exclusive
        // schema transaction, so the final checkpoint cannot rely solely on
        // the earlier snapshot. This second read also revalidates the exact
        // anchor-bound sentinel before any companion journal record is written.
        require_active_managed_fence(guard, lease).await?;
        let final_applied = load_verified_legacy_partition(
            guard,
            sentinel_expectation,
            "legacy applied ledger cannot be re-read at the final checkpoint boundary",
        )
        .await?;
        verify_legacy_continuity(self.legacy_history.graph(), final_applied.applied())?;
        let final_observed = digest_legacy_applied_records(final_applied.applied())?;
        if final_observed != self.binding.expected_applied_set {
            return Err(legacy_import_failure(
                "migration_legacy_applied_set_drift",
                "final guarded legacy ledger differs from the bridge applied-set binding",
            ));
        }
        require_active_managed_fence(guard, lease).await?;
        self.require_canonical_authority_unchanged("guarded_authority_validation")
    }

    async fn guarded_record_applied(
        &self,
        lease: &MigrationLease,
        record: AppliedRecord,
    ) -> Result<JournalEntry<AppliedRecord>, Diagnostic> {
        if record.migration_id() != &self.bridge_id {
            return Err(legacy_import_failure(
                "migration_legacy_import_guard_record_mismatch",
                "legacy checkpoint guard rejected a non-bridge applied record",
            ));
        }
        self.ensure_managed_anchor(lease).await?;

        // A V1 writer may commit after the marker transaction and before this
        // second guard is acquired. That window is safe because the full
        // ledger/live/filesystem authority is repeated here before any
        // companion Applied record can be appended.
        let mut guard = self
            .managed_database
            .schema_transaction()
            .await
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_import_guard_unavailable",
                    "managed schema transaction cannot guard the companion checkpoint",
                )
                .with_detail("provider", error.to_string())
            })?;
        let checked = async {
            self.validate_guarded_authority(
                &mut guard,
                lease,
                LegacyCutoverSentinelExpectation::RequiredExact(&self.binding.anchor_fingerprint),
            )
            .await?;
            self.binding.validate_exact_anchor(&mut guard).await?;
            require_active_managed_fence(&mut guard, lease).await?;
            self.require_canonical_authority_unchanged("before_journal_publication")?;
            let journal_result = self.inner.record_applied(lease, record).await;
            let postcheck = self.require_postpublication_authorities_unchanged();
            reconcile_legacy_checkpoint_publication(journal_result, postcheck)
        }
        .await;
        finish_schema_guard(&mut guard, checked, "legacy companion checkpoint boundary").await
    }

    async fn ensure_managed_anchor(&self, lease: &MigrationLease) -> Result<(), Diagnostic> {
        // First establish or verify the managed-side recovery anchor in the
        // same exclusive transaction that observes the V1 ledger and live
        // schema. The companion journal is then a retryable projection of this
        // durable cutover decision rather than the sole cross-database truth.
        let mut anchor_guard =
            self.managed_database
                .schema_transaction()
                .await
                .map_err(|error| {
                    legacy_import_failure(
                        "migration_legacy_import_guard_unavailable",
                        "managed schema transaction cannot guard the legacy checkpoint",
                    )
                    .with_detail("provider", error.to_string())
                })?;
        let validation = self
            .validate_guarded_authority(
                &mut anchor_guard,
                lease,
                LegacyCutoverSentinelExpectation::OptionalExact(&self.binding.anchor_fingerprint),
            )
            .await;
        if let Err(error) = validation {
            return finish_schema_guard(
                &mut anchor_guard,
                Err(error),
                "legacy anchor authority validation",
            )
            .await;
        }
        let partition = load_verified_legacy_partition(
            &mut anchor_guard,
            LegacyCutoverSentinelExpectation::OptionalExact(&self.binding.anchor_fingerprint),
            "legacy applied ledger cannot be read while staging the cutover pair",
        )
        .await;
        let sentinel = match partition {
            Ok(partition) => partition.sentinel_fingerprint().map(str::to_owned),
            Err(error) => {
                return finish_schema_guard(
                    &mut anchor_guard,
                    Err(error),
                    "legacy cutover sentinel inspection",
                )
                .await;
            }
        };
        match (
            sentinel.as_deref(),
            load_legacy_cutover_anchor(&mut anchor_guard, &self.binding.managed_scope).await,
        ) {
            (Some(sentinel), Ok(Some(observed)))
                if sentinel == self.binding.anchor_fingerprint
                    && observed == self.binding.anchor_fingerprint =>
            {
                finish_schema_guard(
                    &mut anchor_guard,
                    Ok(()),
                    "existing legacy cutover-pair inspection",
                )
                .await
            }
            (None, Ok(None)) => {
                let staged = async {
                    insert_legacy_cutover_anchor(
                        &mut anchor_guard,
                        &self.binding.managed_scope,
                        &self.binding.anchor_fingerprint,
                    )
                    .await?;
                    TypeDbStateStore::insert_legacy_cutover_sentinel_in_transaction(
                        &mut anchor_guard,
                        &self.binding.anchor_fingerprint,
                    )
                    .await
                    .map_err(|error| {
                        legacy_import_failure(
                            "migration_legacy_import_sentinel_insert_failed",
                            "managed legacy-writer sentinel cannot be staged exactly",
                        )
                        .with_detail("legacy", error.to_string())
                    })?;
                    self.binding
                        .validate_exact_contents(&mut anchor_guard)
                        .await?;
                    require_active_managed_fence(&mut anchor_guard, lease).await
                }
                .await;
                if let Err(error) = staged {
                    return finish_schema_guard(
                        &mut anchor_guard,
                        Err(error),
                        "legacy cutover-pair staging",
                    )
                    .await;
                }
                match anchor_guard.commit_classified().await {
                    Ok(()) => Ok(()),
                    Err(error)
                        if matches!(
                            error.commit_failure_certainty(),
                            Some(CommitFailureCertainty::Unknown) | None
                        ) =>
                    {
                        self.inspect_unknown_anchor_commit(lease, error.to_string())
                            .await
                    }
                    Err(error) => Err(legacy_import_failure(
                        "migration_legacy_import_anchor_commit_aborted",
                        "managed legacy cutover pair commit was definitely aborted",
                    )
                    .with_detail("provider", error.to_string())),
                }
            }
            (Some(_), Ok(None)) | (None, Ok(Some(_))) => {
                let error = legacy_import_failure(
                    "migration_legacy_import_cutover_pair_incomplete",
                    "managed cutover anchor and legacy-writer sentinel must appear atomically",
                );
                finish_schema_guard(
                    &mut anchor_guard,
                    Err(error),
                    "incomplete legacy cutover-pair inspection",
                )
                .await
            }
            (_, Ok(Some(_))) => {
                let error = legacy_import_failure(
                    "migration_legacy_import_anchor_mismatch",
                    "managed database carries a different legacy cutover pair",
                );
                finish_schema_guard(
                    &mut anchor_guard,
                    Err(error),
                    "foreign legacy cutover-pair inspection",
                )
                .await
            }
            (_, Err(error)) => {
                finish_schema_guard(&mut anchor_guard, Err(error), "legacy anchor inspection").await
            }
        }
    }

    async fn inspect_unknown_anchor_commit(
        &self,
        lease: &MigrationLease,
        commit_error: String,
    ) -> Result<(), Diagnostic> {
        let mut inspection = self
            .managed_database
            .schema_transaction()
            .await
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_import_anchor_commit_uncertain",
                    "unknown anchor commit cannot be inspected through a fresh schema guard",
                )
                .with_detail("commit", commit_error.clone())
                .with_detail("provider", error.to_string())
            })?;
        let checked = async {
            require_active_managed_fence(&mut inspection, lease).await?;
            self.binding
                .validate_exact_contents(&mut inspection)
                .await?;
            require_active_managed_fence(&mut inspection, lease).await
        }
        .await
        .map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_anchor_commit_uncertain",
                "unknown anchor commit did not yield the exact recoverable marker",
            )
            .with_detail("commit", commit_error)
            .with_detail("inspection", error.to_string())
        });
        finish_schema_guard(
            &mut inspection,
            checked,
            "unknown legacy anchor commit inspection",
        )
        .await
    }
}

impl MigrationLeaseStore for LegacyCheckpointStore<'_> {
    fn acquire<'a>(
        &'a self,
        scope: &'a ExecutionScope,
        holder: &'a LeaseHolderId,
    ) -> ExecutionFuture<'a, MigrationLease> {
        self.inner.acquire(scope, holder)
    }

    fn release<'a>(&'a self, lease: &'a MigrationLease) -> ExecutionFuture<'a, ()> {
        self.inner.release(lease)
    }
}

impl MigrationExecutionJournal for LegacyCheckpointStore<'_> {
    fn begin_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: PlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<PlanRecord>> {
        self.inner.begin_plan(lease, record)
    }

    fn record_group_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: GroupEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<GroupEventRecord>> {
        self.inner.record_group_event(lease, record)
    }

    fn record_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: AppliedRecord,
    ) -> ExecutionFuture<'a, JournalEntry<AppliedRecord>> {
        Box::pin(async move { self.guarded_record_applied(lease, record).await })
    }

    fn load_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<AppliedRecord>>> {
        self.inner.load_applied(lease)
    }

    fn load_open_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenPlanRecord>> {
        self.inner.load_open_plan(lease)
    }

    fn begin_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackPlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackPlanRecord>> {
        self.inner.begin_rollback_plan(lease, record)
    }

    fn record_rollback_step_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackStepEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackStepEventRecord>> {
        self.inner.record_rollback_step_event(lease, record)
    }

    fn record_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RolledBackRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RolledBackRecord>> {
        self.inner.record_rolled_back(lease, record)
    }

    fn load_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<RolledBackRecord>>> {
        self.inner.load_rolled_back(lease)
    }

    fn load_open_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenRollbackPlanRecord>> {
        self.inner.load_open_rollback_plan(lease)
    }
}

/// Terminal journal adapter for every ordinary bridge descendant.
///
/// Provider transaction checks protect each managed schema commit; this
/// adapter closes the separate window at companion `Applied`/`RolledBack`
/// publication by retaining an exclusive managed SCHEMA guard across it.
struct LegacyBoundStore<'a> {
    inner: &'a TypeDbMigrationStore<'a>,
    managed_database: &'a Database,
    binding: &'a LegacyExecutionBinding,
    managed_scope: &'a str,
    observation_authority: &'a ManagedObservationAuthority,
    capabilities: CapabilitySet,
    applied_checkpoints: BTreeMap<MigrationId, ManagedSchemaState>,
    rolled_back_checkpoints: BTreeMap<MigrationId, ManagedSchemaState>,
}

impl<'a> LegacyBoundStore<'a> {
    fn for_apply(
        inner: &'a TypeDbMigrationStore<'a>,
        managed_database: &'a Database,
        binding: &'a LegacyExecutionBinding,
        managed_scope: &'a str,
        observation_authority: &'a ManagedObservationAuthority,
        plan: &VerifiedMigrationApplyPlan,
    ) -> Result<Self, Diagnostic> {
        let applied_checkpoints = plan
            .migrations()
            .iter()
            .map(|migration| {
                (
                    migration.manifest().id().clone(),
                    migration.manifest().target_state().clone(),
                )
            })
            .collect();
        Self::new(
            inner,
            managed_database,
            binding,
            managed_scope,
            observation_authority,
            applied_checkpoints,
            BTreeMap::new(),
        )
    }

    fn for_rollback(
        inner: &'a TypeDbMigrationStore<'a>,
        managed_database: &'a Database,
        binding: &'a LegacyExecutionBinding,
        managed_scope: &'a str,
        observation_authority: &'a ManagedObservationAuthority,
        plan: &type_bridge_schema_migration::VerifiedMigrationRollbackPlan,
    ) -> Result<Self, Diagnostic> {
        let rolled_back_checkpoints = plan
            .rollbacks()
            .iter()
            .map(|rollback| {
                (
                    rollback.manifest().id().clone(),
                    rollback.manifest().source_state().clone(),
                )
            })
            .collect();
        Self::new(
            inner,
            managed_database,
            binding,
            managed_scope,
            observation_authority,
            BTreeMap::new(),
            rolled_back_checkpoints,
        )
    }

    fn new(
        inner: &'a TypeDbMigrationStore<'a>,
        managed_database: &'a Database,
        binding: &'a LegacyExecutionBinding,
        managed_scope: &'a str,
        observation_authority: &'a ManagedObservationAuthority,
        applied_checkpoints: BTreeMap<MigrationId, ManagedSchemaState>,
        rolled_back_checkpoints: BTreeMap<MigrationId, ManagedSchemaState>,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            inner,
            managed_database,
            binding,
            managed_scope,
            observation_authority,
            capabilities: execution_capability_vocabulary()?,
            applied_checkpoints,
            rolled_back_checkpoints,
        })
    }

    async fn validate_guard(
        &self,
        guard: &mut type_bridge_orm::Transaction,
        lease: &MigrationLease,
        expected: &ManagedSchemaState,
    ) -> Result<(), Diagnostic> {
        require_active_managed_fence(guard, lease).await?;
        self.binding
            .validate_contents(guard, self.managed_scope)
            .await?;
        let export = self.managed_database.schema_text().await.map_err(|error| {
            legacy_import_failure(
                "migration_legacy_checkpoint_live_export_failed",
                "managed schema cannot be exported inside the terminal checkpoint guard",
            )
            .with_detail("provider", error.to_string())
        })?;
        require_active_managed_fence(guard, lease).await?;
        self.binding
            .validate_contents(guard, self.managed_scope)
            .await?;
        let observed = observe_managed_state_from_export_with_authority(
            DocumentId::new("typebridge-terminal-checkpoint-live.typeql")?,
            &export,
            &self.capabilities,
            expected,
            expected,
            self.observation_authority,
        )?;
        if observed != *expected {
            return Err(legacy_import_failure(
                "migration_legacy_checkpoint_live_state_mismatch",
                "terminal checkpoint guard did not observe the exact committed managed state",
            ));
        }
        require_active_managed_fence(guard, lease).await
    }

    async fn guarded_record_applied(
        &self,
        lease: &MigrationLease,
        record: AppliedRecord,
    ) -> Result<JournalEntry<AppliedRecord>, Diagnostic> {
        let expected = self
            .applied_checkpoints
            .get(record.migration_id())
            .ok_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_checkpoint_record_mismatch",
                    "applied checkpoint is absent from the bound verified apply plan",
                )
            })?;
        let mut guard = self
            .managed_database
            .schema_transaction()
            .await
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_binding_guard_unavailable",
                    "managed schema transaction cannot guard applied checkpoint publication",
                )
                .with_detail("provider", error.to_string())
            })?;
        let checked = async {
            self.validate_guard(&mut guard, lease, expected).await?;
            let entry = self.inner.record_applied(lease, record).await?;
            self.validate_guard(&mut guard, lease, expected).await?;
            Ok(entry)
        }
        .await;
        finish_schema_guard(
            &mut guard,
            checked,
            "bridge-descendant applied checkpoint boundary",
        )
        .await
    }

    async fn guarded_record_rolled_back(
        &self,
        lease: &MigrationLease,
        record: RolledBackRecord,
    ) -> Result<JournalEntry<RolledBackRecord>, Diagnostic> {
        let expected = self
            .rolled_back_checkpoints
            .get(record.migration_id())
            .ok_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_checkpoint_record_mismatch",
                    "rollback checkpoint is absent from the bound verified rollback plan",
                )
            })?;
        let mut guard = self
            .managed_database
            .schema_transaction()
            .await
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_binding_guard_unavailable",
                    "managed schema transaction cannot guard rollback checkpoint publication",
                )
                .with_detail("provider", error.to_string())
            })?;
        let checked = async {
            self.validate_guard(&mut guard, lease, expected).await?;
            let entry = self.inner.record_rolled_back(lease, record).await?;
            self.validate_guard(&mut guard, lease, expected).await?;
            Ok(entry)
        }
        .await;
        finish_schema_guard(
            &mut guard,
            checked,
            "bridge-descendant rollback checkpoint boundary",
        )
        .await
    }
}

impl MigrationLeaseStore for LegacyBoundStore<'_> {
    fn acquire<'a>(
        &'a self,
        scope: &'a ExecutionScope,
        holder: &'a LeaseHolderId,
    ) -> ExecutionFuture<'a, MigrationLease> {
        self.inner.acquire(scope, holder)
    }

    fn release<'a>(&'a self, lease: &'a MigrationLease) -> ExecutionFuture<'a, ()> {
        self.inner.release(lease)
    }
}

impl MigrationExecutionJournal for LegacyBoundStore<'_> {
    fn begin_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: PlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<PlanRecord>> {
        self.inner.begin_plan(lease, record)
    }

    fn record_group_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: GroupEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<GroupEventRecord>> {
        self.inner.record_group_event(lease, record)
    }

    fn record_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: AppliedRecord,
    ) -> ExecutionFuture<'a, JournalEntry<AppliedRecord>> {
        Box::pin(async move { self.guarded_record_applied(lease, record).await })
    }

    fn load_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<AppliedRecord>>> {
        self.inner.load_applied(lease)
    }

    fn load_open_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenPlanRecord>> {
        self.inner.load_open_plan(lease)
    }

    fn begin_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackPlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackPlanRecord>> {
        self.inner.begin_rollback_plan(lease, record)
    }

    fn record_rollback_step_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackStepEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackStepEventRecord>> {
        self.inner.record_rollback_step_event(lease, record)
    }

    fn record_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RolledBackRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RolledBackRecord>> {
        Box::pin(async move { self.guarded_record_rolled_back(lease, record).await })
    }

    fn load_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<RolledBackRecord>>> {
        self.inner.load_rolled_back(lease)
    }

    fn load_open_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenRollbackPlanRecord>> {
        self.inner.load_open_rollback_plan(lease)
    }
}

impl TypeDbMigrationRunner {
    /// Bind the runner to one database pair and one explicit execution policy.
    ///
    /// `genesis_source` is the schema every parentless manifest verifies
    /// against — the empty declared schema unless the scope was adopted with
    /// pre-managed content. Pair-name derivation, capability coherence, and
    /// server-version gates are enforced by the store, planner, and provider
    /// this runner composes.
    pub fn new(
        managed_database: Arc<Database>,
        journal_database: Arc<Database>,
        genesis_source: DeclaredSchema,
        context: ManagedDeltaContext,
        lowering_binding: SchemaLoweringBinding,
        policy: MigrationSafetyPolicy,
    ) -> Self {
        Self {
            managed_database,
            journal_database,
            genesis_source,
            context,
            lowering_binding,
            policy,
        }
    }

    /// Discover and replay-verify the canonical chain in one directory.
    pub fn discover(&self, directory: &Path) -> Result<MigrationHistoryGraph, Diagnostic> {
        let directory = open_canonical_directory(directory)?;
        self.discover_in(&directory)
    }

    /// Discover through one retained directory capability.
    pub fn discover_in(
        &self,
        directory: &MigrationDirectory,
    ) -> Result<MigrationHistoryGraph, Diagnostic> {
        let adopted_genesis_present = directory
            .entry_exists(ADOPTED_GENESIS_FILE_NAME.as_ref())
            .map_err(|_| {
                Diagnostic::new(
                    type_bridge_contract::diagnostic::DiagnosticCategory::Integrity,
                    type_bridge_contract::diagnostic::DiagnosticCode::new(
                        "migration_adoption_authority_presence_unreadable",
                    )
                    .expect("static adoption diagnostic code"),
                    "canonical adoption-authority presence cannot be inspected",
                )
            })?;
        let bridge_count = canonical_history_declared_legacy_bridge_count_in(directory)?;
        require_adoption_authority_pair_state(adopted_genesis_present, bridge_count)?;
        let graph =
            discover_verified_migration_chain_in(directory, &self.genesis_source, &self.context)?;
        require_adoption_authority_pair(&graph, adopted_genesis_present)?;
        Ok(graph)
    }

    /// Apply the discovered chain up to `target` through the live pair.
    ///
    /// The applied basis is read under a short-lived inspection lease that is
    /// released before execution; the coordinator re-acquires the lease and
    /// its stale-plan gates reject the derived plan if the ledger moved in
    /// between, so the read is rendezvous only, never authority.
    pub async fn apply(
        &self,
        directory: &Path,
        target: &MigrationApplyTarget,
        holder: &LeaseHolderId,
        approvals: &[MigrationApplyApproval],
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let directory = open_canonical_directory(directory)?;
        self.apply_in(&directory, target, holder, approvals).await
    }

    /// Apply through one retained directory capability.
    pub async fn apply_in(
        &self,
        directory: &MigrationDirectory,
        target: &MigrationApplyTarget,
        holder: &LeaseHolderId,
        approvals: &[MigrationApplyApproval],
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let graph = self.discover_in(directory)?;
        let adopted_authority = self.verify_adopted_extension_state(directory).await?;
        require_adoption_authority_pair(&graph, adopted_authority.is_some())?;
        let observation_authority =
            ManagedObservationAuthority::from_adopted(adopted_authority.as_ref());
        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            self.context.scope_id().clone(),
            catalog,
        )?;
        store.ensure_control_schema().await?;

        let basis = self.load_applied_basis(&store, holder).await?;
        let pending = match target {
            MigrationApplyTarget::DefaultHead => graph.plan_apply_to_default_head(&basis)?,
            MigrationApplyTarget::Explicit(targets) => graph.plan_apply(&basis, targets)?,
        };
        if pending.iter().any(|id| {
            graph
                .manifest(id)
                .is_some_and(|manifest| manifest.is_legacy_bridge())
        }) {
            return Err(legacy_import_failure(
                "migration_legacy_import_required",
                "a legacy bridge can be established only by the guarded legacy import command",
            )
            .into());
        }
        let legacy_binding = LegacyExecutionBinding::from_applied_graph(
            &graph,
            &basis,
            adopted_authority.as_deref(),
            &self.managed_database,
            &self.journal_database,
            self.context.scope_id().as_str(),
        )?;
        let expected_current = self.expected_applied_state(&graph, &basis)?;
        self.require_legacy_binding_guarded(
            &store,
            holder,
            &legacy_binding,
            &basis,
            &expected_current,
            &observation_authority,
        )
        .await?;
        if pending.is_empty() {
            return Ok(MigrationDirectoryApplyOutcome::UpToDate);
        }

        let plan = build_verified_migration_apply_plan(
            &graph,
            &basis,
            target,
            &self.context,
            &self.lowering_binding,
            &self.policy,
            approvals,
        )?;
        if let (Some(authority), Some(target_schema)) =
            (adopted_authority.as_ref(), plan.target_schema())
        {
            authority.ensure_released_extension_subjects_survive(target_schema)?;
        }
        let store = match &legacy_binding {
            LegacyExecutionBinding::Active(_) => store.bind_guarded_bridge_plan(&plan)?,
            LegacyExecutionBinding::Absent => store.bind_plan(&plan)?,
        };
        let provider = TypeDbMigrationProvider::new_with_legacy_binding(
            Arc::clone(&self.managed_database),
            legacy_binding.clone(),
            self.context.scope_id().as_str().to_owned(),
            observation_authority.clone(),
        )?;
        let guarded_store = LegacyBoundStore::for_apply(
            &store,
            &self.managed_database,
            &legacy_binding,
            self.context.scope_id().as_str(),
            &observation_authority,
            &plan,
        )?;
        let outcome =
            execute_verified_migration_apply_plan(&guarded_store, &provider, holder, &plan).await?;
        if adopted_authority.is_some() {
            self.verify_adopted_extension_state(directory).await?;
        }
        Ok(MigrationDirectoryApplyOutcome::Executed(outcome))
    }

    /// Verify the migration state triad and report every drift finding.
    ///
    /// The check is strictly read-only: the applied basis is loaded through
    /// the store's lease-free snapshot read — no lease is acquired, no
    /// control schema is installed, and no database is created or touched
    /// beyond schema exports and read transactions. The live schema is
    /// rebuilt for reporting without candidate matching, and nothing is
    /// repaired, generated, or applied. A schema mismatch is drift, not an
    /// invitation to reconcile production automatically.
    pub async fn verify(
        &self,
        directory: &Path,
        desired: Option<&DeclaredSchema>,
    ) -> Result<MigrationVerifyReport, MigrationDirectoryApplyError> {
        let directory = open_canonical_directory(directory)?;
        self.verify_in(&directory, desired).await
    }

    /// Verify through one retained directory capability.
    pub async fn verify_in(
        &self,
        directory: &MigrationDirectory,
        desired: Option<&DeclaredSchema>,
    ) -> Result<MigrationVerifyReport, MigrationDirectoryApplyError> {
        let graph = self.discover_in(directory)?;
        let adopted_authority = self.verify_adopted_extension_state(directory).await?;
        require_adoption_authority_pair(&graph, adopted_authority.is_some())?;
        let observation_authority =
            ManagedObservationAuthority::from_adopted(adopted_authority.as_ref());
        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            self.context.scope_id().clone(),
            catalog,
        )?;
        let scope = ExecutionScope::new(self.context.scope_id().clone());
        let basis: BTreeSet<MigrationId> = store
            .load_applied_read_only(&scope)
            .await?
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect();
        let legacy_binding = LegacyExecutionBinding::from_applied_graph(
            &graph,
            &basis,
            adopted_authority.as_deref(),
            &self.managed_database,
            &self.journal_database,
            self.context.scope_id().as_str(),
        )?;
        let legacy_before = self
            .inspect_legacy_binding_read_only(&legacy_binding)
            .await?;

        let export = self.managed_database.schema_text().await.map_err(|error| {
            legacy_import_failure(
                "migration_typedb_export_unreadable",
                "managed database schema export failed during verification",
            )
            .with_detail("provider", error.to_string())
        })?;
        let document = DocumentId::new("typebridge-verify-live-export.typeql")?;
        let live = rebuild_live_managed_state_with_authority(
            document,
            &export,
            &self.genesis_source,
            &self.context,
            &observation_authority,
        )?;

        let legacy_after = self
            .inspect_legacy_binding_read_only(&legacy_binding)
            .await?;
        let basis_after: BTreeSet<MigrationId> = store
            .load_applied_read_only(&scope)
            .await?
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect();
        let legacy_drift =
            reconcile_verify_snapshots(&basis, &basis_after, legacy_before, legacy_after);

        let mut report = verify_migration_state(
            &graph,
            &basis,
            &self.genesis_source,
            desired,
            Some(&live),
            &self.context,
        )?;
        if let Some(diagnostic) = legacy_drift {
            report.prepend_applied_ledger_drift(diagnostic);
        }
        Ok(report)
    }

    /// Import a completed legacy (v1) history as the canonical bridge basis.
    ///
    /// The legacy directory is loaded through checksum-bound archival metadata
    /// produced by the frozen trusted reader, including histories with
    /// nonportable operations. The complete graph and applied ledger are
    /// validated, the head snapshot independently reconstructs exact released
    /// schema identity, and the live database must equal it before the bridge
    /// can apply as a zero-operation checkpoint. No legacy operation is replayed
    /// or marked as newly executed.
    pub async fn import_legacy_frontier(
        &self,
        legacy_directory: &Path,
        canonical_directory: &Path,
        holder: &LeaseHolderId,
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let canonical_directory = open_canonical_directory(canonical_directory)?;
        self.import_legacy_frontier_in(legacy_directory, &canonical_directory, holder)
            .await
    }

    /// Import a legacy frontier through retained canonical-directory authority.
    pub async fn import_legacy_frontier_in(
        &self,
        legacy_directory: &Path,
        canonical_directory: &MigrationDirectory,
        holder: &LeaseHolderId,
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let legacy_history = load_adoption_history(legacy_directory).map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_load_failed",
                "legacy migration directory failed the checked adoption loader",
            )
            .with_detail("legacy", error.to_string())
        })?;
        let reconstructed = reconstruct_legacy_head(&legacy_history).map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_reconstruction_failed",
                "legacy graph head could not be reconstructed from its immutable snapshot",
            )
            .with_detail("legacy", error.to_string())
        })?;
        self.import_verified_legacy_frontier_in(
            &legacy_history,
            &reconstructed,
            canonical_directory,
            holder,
        )
        .await
    }

    /// Import a frontier from one retained, already reconstructed legacy
    /// authority.
    ///
    /// This keeps bridge derivation, live comparison, and guarded checkpoint
    /// application bound to the same checked directory capture. The method
    /// revalidates that capture at every external-state boundary.
    pub async fn import_verified_legacy_frontier_in(
        &self,
        legacy_history: &LegacyAdoptionHistory,
        reconstructed: &VerifiedLegacyHead,
        canonical_directory: &MigrationDirectory,
        holder: &LeaseHolderId,
    ) -> Result<MigrationDirectoryApplyOutcome, MigrationDirectoryApplyError> {
        let reconstructed_authority = Arc::new(parse_adopted_genesis_authority(
            DocumentId::new("typebridge-legacy-reconstructed-head.typeql")?,
            reconstructed.schema_typeql(),
        )?);
        let observation_authority =
            ManagedObservationAuthority::Adopted(Arc::clone(&reconstructed_authority));
        let stored_genesis = read_stored_genesis(canonical_directory)?;
        if stored_genesis != reconstructed.schema_typeql().as_bytes() {
            return Err(legacy_import_failure(
                "migration_legacy_import_genesis_bytes_mismatch",
                "stored adopted genesis bytes differ from the verified legacy-head snapshot",
            )
            .into());
        }
        if reconstructed_authority
            .declared()
            .declared_identity_fingerprint()
            != self.genesis_source.declared_identity_fingerprint()
        {
            return Err(legacy_import_failure(
                "migration_legacy_import_genesis_mismatch",
                "stored adopted genesis differs from the verified legacy-head snapshot",
            )
            .into());
        }
        let frontier = extract_legacy_frontier(legacy_history.graph())?;
        let legacy_applied_set = extract_legacy_applied_set_digest(legacy_history.graph())?;
        let (graph, canonical_evidence) = discover_verified_migration_chain_with_evidence_in(
            canonical_directory,
            &self.genesis_source,
            &self.context,
        )?;
        require_adoption_authority_pair(&graph, true)?;
        let bridge = graph
            .manifests()
            .map(|(_, manifest)| manifest)
            .find(|manifest| manifest.is_legacy_bridge())
            .ok_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_import_bridge_missing",
                    "canonical directory carries no legacy-frontier bridge manifest",
                )
            })?;
        if bridge.legacy_parents() != frontier.as_slice() {
            return Err(legacy_import_failure(
                "migration_legacy_import_frontier_mismatch",
                "bridge manifest records a different legacy frontier than the loaded files",
            )
            .into());
        }
        if bridge.legacy_applied_set() != Some(&legacy_applied_set) {
            return Err(legacy_import_failure(
                "migration_legacy_import_applied_set_mismatch",
                "bridge manifest records a different complete legacy applied set",
            )
            .into());
        }
        let pending_binding = LegacyBridgeBinding::from_bridge(
            bridge,
            &reconstructed_authority,
            &self.managed_database,
            &self.journal_database,
            self.context.scope_id().as_str(),
        )?;
        let applied =
            load_pending_legacy_applied_read_only(&self.managed_database, &pending_binding).await?;
        legacy_history
            .require_unchanged_head(reconstructed)
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_import_authority_changed",
                    "legacy migration authority changed while external state was inspected",
                )
                .with_detail("legacy", error.to_string())
            })?;
        verify_legacy_continuity(legacy_history.graph(), &applied)?;
        if digest_legacy_applied_records(&applied)? != legacy_applied_set {
            return Err(legacy_import_failure(
                "migration_legacy_applied_set_drift",
                "legacy applied ledger differs from the complete checked graph binding",
            )
            .into());
        }

        // Exact live comparison is read-only and precedes control-schema
        // installation. On a retry, permit only the frozen managed fence
        // partition that a prior attempt may already have installed.
        let export = self.managed_database.schema_text().await.map_err(|error| {
            legacy_import_failure(
                "migration_legacy_import_live_export_failed",
                "managed database schema cannot be exported for adoption preflight",
            )
            .with_detail("provider", error.to_string())
        })?;
        legacy_history
            .require_unchanged_head(reconstructed)
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_import_authority_changed",
                    "legacy migration authority changed while live schema was exported",
                )
                .with_detail("legacy", error.to_string())
            })?;
        let expected_internal = expected_managed_internal_schema()?;
        let live_authority = parse_adopted_genesis_authority_with_internal(
            DocumentId::new("typebridge-legacy-live-head.typeql")?,
            &export,
            Some(&expected_internal),
        )?;
        if !same_adopted_authority(&reconstructed_authority, &live_authority) {
            return Err(legacy_import_failure(
                "migration_legacy_import_live_drift",
                "live managed schema differs from the independently reconstructed legacy head",
            )
            .into());
        }

        let target = MigrationApplyTarget::Explicit(BTreeSet::from([bridge.id().clone()]));
        // Build the adoption plan against its mandated empty canonical basis
        // before `apply` is allowed to install either control schema. This
        // catches every provider-independent lowering/policy defect first.
        let preflight_plan = build_verified_migration_apply_plan(
            &graph,
            &BTreeSet::new(),
            &target,
            &self.context,
            &self.lowering_binding,
            &self.policy,
            &[],
        )?;
        if let Some(target_schema) = preflight_plan.target_schema() {
            reconstructed_authority.ensure_released_extension_subjects_survive(target_schema)?;
        }
        TypeDbMigrationProvider::new(Arc::clone(&self.managed_database))?;
        legacy_history
            .require_unchanged_head(reconstructed)
            .map_err(|error| {
                legacy_import_failure(
                    "migration_legacy_import_authority_changed",
                    "legacy migration authority changed before checkpoint application",
                )
                .with_detail("legacy", error.to_string())
            })?;

        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            self.context.scope_id().clone(),
            catalog,
        )?;
        store.ensure_control_schema().await?;
        let basis = self.load_applied_basis(&store, holder).await?;
        let active_binding = LegacyExecutionBinding::from_applied_graph(
            &graph,
            &basis,
            Some(reconstructed_authority.as_ref()),
            &self.managed_database,
            &self.journal_database,
            self.context.scope_id().as_str(),
        )?;
        let expected_current = self.expected_applied_state(&graph, &basis)?;
        if matches!(active_binding, LegacyExecutionBinding::Active(_)) {
            self.require_legacy_binding_guarded(
                &store,
                holder,
                &active_binding,
                &basis,
                &expected_current,
                &observation_authority,
            )
            .await?;
        } else {
            self.require_pending_import_binding_guarded(
                &store,
                holder,
                &pending_binding,
                &basis,
                &expected_current,
                &observation_authority,
            )
            .await?;
        }
        let pending = graph.plan_apply(
            &basis,
            match &target {
                MigrationApplyTarget::Explicit(targets) => targets,
                MigrationApplyTarget::DefaultHead => unreachable!("legacy target is explicit"),
            },
        )?;
        if pending.is_empty() {
            return Ok(MigrationDirectoryApplyOutcome::UpToDate);
        }
        let plan = if basis.is_empty() {
            preflight_plan
        } else {
            build_verified_migration_apply_plan(
                &graph,
                &basis,
                &target,
                &self.context,
                &self.lowering_binding,
                &self.policy,
                &[],
            )?
        };
        if let Some(target_schema) = plan.target_schema() {
            reconstructed_authority.ensure_released_extension_subjects_survive(target_schema)?;
        }
        let store = store.bind_legacy_import_plan(&plan)?;
        let provider = TypeDbMigrationProvider::new_with_observation_authority(
            Arc::clone(&self.managed_database),
            observation_authority,
        )?;
        let guarded_store = LegacyCheckpointStore::new(
            &store,
            &self.managed_database,
            &self.journal_database,
            LegacyCheckpointAuthorities {
                canonical_directory,
                canonical_evidence: &canonical_evidence,
                legacy_history,
                reconstructed,
                reconstructed_authority: &reconstructed_authority,
            },
            &plan,
        )?;
        let outcome =
            execute_verified_migration_apply_plan(&guarded_store, &provider, holder, &plan).await?;
        self.verify_adopted_extension_state(canonical_directory)
            .await?;
        Ok(MigrationDirectoryApplyOutcome::Executed(outcome))
    }

    /// Roll the requested applied migrations back through the live pair.
    ///
    /// `removals` must be downward-closed over the applied set: an identity
    /// with a remaining applied descendant is rejected by the planner. The
    /// applied basis is read under the same rendezvous-only inspection lease
    /// as [`Self::apply`]; the coordinator's stale-ledger gate rejects the
    /// derived plan if the ledger moved in between.
    pub async fn rollback(
        &self,
        directory: &Path,
        removals: &BTreeSet<MigrationId>,
        holder: &LeaseHolderId,
        approvals: &[MigrationApplyApproval],
    ) -> Result<MigrationDirectoryRollbackOutcome, MigrationDirectoryApplyError> {
        let directory = open_canonical_directory(directory)?;
        self.rollback_in(&directory, removals, holder, approvals)
            .await
    }

    /// Roll back through one retained directory capability.
    pub async fn rollback_in(
        &self,
        directory: &MigrationDirectory,
        removals: &BTreeSet<MigrationId>,
        holder: &LeaseHolderId,
        approvals: &[MigrationApplyApproval],
    ) -> Result<MigrationDirectoryRollbackOutcome, MigrationDirectoryApplyError> {
        let graph = self.discover_in(directory)?;
        let adopted_authority = self.verify_adopted_extension_state(directory).await?;
        require_adoption_authority_pair(&graph, adopted_authority.is_some())?;
        let observation_authority =
            ManagedObservationAuthority::from_adopted(adopted_authority.as_ref());
        let catalog = VerifiedMigrationCatalog::new(graph.manifests().map(|(_, m)| m))?;
        let store = TypeDbMigrationStore::new(
            Arc::clone(&self.managed_database),
            Arc::clone(&self.journal_database),
            self.context.scope_id().clone(),
            catalog,
        )?;
        store.ensure_control_schema().await?;

        let basis = self.load_applied_basis(&store, holder).await?;
        let legacy_binding = LegacyExecutionBinding::from_applied_graph(
            &graph,
            &basis,
            adopted_authority.as_deref(),
            &self.managed_database,
            &self.journal_database,
            self.context.scope_id().as_str(),
        )?;
        let expected_current = self.expected_applied_state(&graph, &basis)?;
        self.require_legacy_binding_guarded(
            &store,
            holder,
            &legacy_binding,
            &basis,
            &expected_current,
            &observation_authority,
        )
        .await?;
        if removals.iter().any(|id| {
            graph
                .manifest(id)
                .is_some_and(|manifest| manifest.is_legacy_bridge())
        }) {
            return Err(legacy_import_failure(
                "migration_rollback_legacy_bridge_permanent",
                "the legacy cutover bridge is a permanent lineage root and cannot be rolled back",
            )
            .into());
        }
        if removals.iter().any(|id| graph.manifest(id).is_none()) {
            return Err(legacy_import_failure(
                "migration_history_unknown_rollback_target",
                "rollback target is outside verified history",
            )
            .into());
        }
        if removals.iter().all(|id| !basis.contains(id)) {
            return Ok(MigrationDirectoryRollbackOutcome::UpToDate);
        }

        let plan = build_verified_migration_rollback_plan(
            &graph,
            &basis,
            removals,
            &self.context,
            &self.lowering_binding,
            &self.policy,
            approvals,
        )?;
        if let Some(authority) = adopted_authority.as_ref() {
            authority.ensure_released_extension_subjects_survive(plan.target_schema())?;
        }
        let store = match &legacy_binding {
            LegacyExecutionBinding::Active(_) => store.bind_guarded_bridge_rollback_plan(&plan)?,
            LegacyExecutionBinding::Absent => store.bind_rollback_plan(&plan)?,
        };
        let provider = TypeDbMigrationProvider::new_with_legacy_binding(
            Arc::clone(&self.managed_database),
            legacy_binding.clone(),
            self.context.scope_id().as_str().to_owned(),
            observation_authority.clone(),
        )?;
        let guarded_store = LegacyBoundStore::for_rollback(
            &store,
            &self.managed_database,
            &legacy_binding,
            self.context.scope_id().as_str(),
            &observation_authority,
            &plan,
        )?;
        let outcome =
            execute_verified_migration_rollback_plan(&guarded_store, &provider, holder, &plan)
                .await?;
        if adopted_authority.is_some() {
            self.verify_adopted_extension_state(directory).await?;
        }
        Ok(MigrationDirectoryRollbackOutcome::Executed(outcome))
    }

    async fn load_applied_basis(
        &self,
        store: &TypeDbMigrationStore<'_>,
        holder: &LeaseHolderId,
    ) -> Result<BTreeSet<MigrationId>, Diagnostic> {
        let scope = ExecutionScope::new(self.context.scope_id().clone());
        let lease = store.acquire(&scope, holder).await?;
        let loaded = store.load_applied(&lease).await;
        let release = store.release(&lease).await;
        let entries = match (loaded, release) {
            (Ok(entries), Ok(())) => entries,
            (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
            (Err(primary), Err(secondary)) => {
                return Err(combine_diagnostics(
                    "migration_legacy_basis_load_and_release_failed",
                    "applied-basis loading and lease release both failed",
                    primary,
                    secondary,
                ));
            }
        };
        Ok(entries
            .iter()
            .map(|entry| entry.record().migration_id().clone())
            .collect())
    }

    fn expected_applied_state(
        &self,
        graph: &MigrationHistoryGraph,
        basis: &BTreeSet<MigrationId>,
    ) -> Result<ManagedSchemaState, Diagnostic> {
        let frontier = graph.applied_frontier(basis)?;
        let Some(first) = frontier.first() else {
            return managed_schema_state(&self.genesis_source, &self.context)
                .map_err(managed_state_derivation_diagnostic);
        };
        let expected = graph
            .manifest(first)
            .ok_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_basis_manifest_missing",
                    "applied frontier references a missing verified manifest",
                )
            })?
            .target_state()
            .clone();
        for id in &frontier[1..] {
            let state = graph
                .manifest(id)
                .ok_or_else(|| {
                    legacy_import_failure(
                        "migration_legacy_basis_manifest_missing",
                        "applied frontier references a missing verified manifest",
                    )
                })?
                .target_state();
            if state != &expected {
                return Err(legacy_import_failure(
                    "migration_legacy_basis_state_divergent",
                    "applied frontier does not identify one exact managed state",
                ));
            }
        }
        Ok(expected)
    }

    async fn require_exact_v2_basis(
        store: &TypeDbMigrationStore<'_>,
        lease: &MigrationLease,
        expected: &BTreeSet<MigrationId>,
    ) -> Result<(), Diagnostic> {
        let entries = store.load_applied(lease).await?;
        let mut observed = BTreeSet::new();
        for entry in entries {
            if !observed.insert(entry.record().migration_id().clone()) {
                return Err(legacy_import_failure(
                    "migration_legacy_v2_basis_duplicate",
                    "companion journal contains a duplicate active migration identity",
                ));
            }
        }
        if &observed != expected {
            return Err(legacy_import_failure(
                "migration_legacy_v2_basis_changed",
                "companion-journal applied basis changed across the guarded boundary",
            ));
        }
        Ok(())
    }

    async fn require_legacy_binding_guarded(
        &self,
        store: &TypeDbMigrationStore<'_>,
        holder: &LeaseHolderId,
        binding: &LegacyExecutionBinding,
        expected_basis: &BTreeSet<MigrationId>,
        expected_live: &ManagedSchemaState,
        observation_authority: &ManagedObservationAuthority,
    ) -> Result<(), Diagnostic> {
        let scope = ExecutionScope::new(self.context.scope_id().clone());
        let lease = store.acquire(&scope, holder).await?;
        let checked = match self.managed_database.schema_transaction().await {
            Ok(mut guard) => {
                let result = async {
                    require_active_managed_fence(&mut guard, &lease).await?;
                    binding
                        .validate_contents(&mut guard, self.context.scope_id().as_str())
                        .await?;
                    Self::require_exact_v2_basis(store, &lease, expected_basis).await?;
                    let export = self.managed_database.schema_text().await.map_err(|error| {
                        legacy_import_failure(
                            "migration_legacy_boundary_live_export_failed",
                            "managed schema cannot be exported inside the guarded pair boundary",
                        )
                        .with_detail("provider", error.to_string())
                    })?;
                    require_active_managed_fence(&mut guard, &lease).await?;
                    binding
                        .validate_contents(&mut guard, self.context.scope_id().as_str())
                        .await?;
                    Self::require_exact_v2_basis(store, &lease, expected_basis).await?;
                    let observed = observe_managed_state_from_export_with_authority(
                        DocumentId::new("typebridge-legacy-boundary-live.typeql")?,
                        &export,
                        self.context.available_capabilities(),
                        expected_live,
                        expected_live,
                        observation_authority,
                    )?;
                    if observed != *expected_live {
                        return Err(legacy_import_failure(
                            "migration_legacy_boundary_live_state_mismatch",
                            "guarded pair boundary did not observe the exact current managed state",
                        ));
                    }
                    require_active_managed_fence(&mut guard, &lease).await
                }
                .await;
                finish_schema_guard(&mut guard, result, "legacy pair boundary inspection").await
            }
            Err(error) => Err(legacy_import_failure(
                "migration_legacy_binding_guard_unavailable",
                "managed schema transaction cannot guard the legacy pair boundary",
            )
            .with_detail("provider", error.to_string())),
        };
        combine_boundary_release(checked, store.release(&lease).await)
    }

    async fn require_pending_import_binding_guarded(
        &self,
        store: &TypeDbMigrationStore<'_>,
        holder: &LeaseHolderId,
        binding: &LegacyBridgeBinding,
        expected_basis: &BTreeSet<MigrationId>,
        expected_live: &ManagedSchemaState,
        observation_authority: &ManagedObservationAuthority,
    ) -> Result<(), Diagnostic> {
        let scope = ExecutionScope::new(self.context.scope_id().clone());
        let lease = store.acquire(&scope, holder).await?;
        let checked = match self.managed_database.schema_transaction().await {
            Ok(mut guard) => {
                let result = async {
                    require_active_managed_fence(&mut guard, &lease).await?;
                    binding.validate_pending_import_contents(&mut guard).await?;
                    Self::require_exact_v2_basis(store, &lease, expected_basis).await?;
                    let export = self.managed_database.schema_text().await.map_err(|error| {
                        legacy_import_failure(
                            "migration_legacy_boundary_live_export_failed",
                            "managed schema cannot be exported inside the pending-import boundary",
                        )
                        .with_detail("provider", error.to_string())
                    })?;
                    require_active_managed_fence(&mut guard, &lease).await?;
                    binding.validate_pending_import_contents(&mut guard).await?;
                    Self::require_exact_v2_basis(store, &lease, expected_basis).await?;
                    let observed = observe_managed_state_from_export_with_authority(
                        DocumentId::new("typebridge-pending-import-live.typeql")?,
                        &export,
                        self.context.available_capabilities(),
                        expected_live,
                        expected_live,
                        observation_authority,
                    )?;
                    if observed != *expected_live {
                        return Err(legacy_import_failure(
                            "migration_legacy_boundary_live_state_mismatch",
                            "pending-import boundary did not observe the exact bridge source state",
                        ));
                    }
                    require_active_managed_fence(&mut guard, &lease).await
                }
                .await;
                finish_schema_guard(
                    &mut guard,
                    result,
                    "pending legacy import boundary inspection",
                )
                .await
            }
            Err(error) => Err(legacy_import_failure(
                "migration_legacy_binding_guard_unavailable",
                "managed schema transaction cannot guard the pending legacy import",
            )
            .with_detail("provider", error.to_string())),
        };
        combine_boundary_release(checked, store.release(&lease).await)
    }

    async fn inspect_legacy_binding_read_only(
        &self,
        binding: &LegacyExecutionBinding,
    ) -> Result<LegacyBindingReadInspection, Diagnostic> {
        inspect_legacy_binding_read_only(
            &self.managed_database,
            binding,
            self.context.scope_id().as_str(),
        )
        .await
    }

    async fn verify_adopted_extension_state(
        &self,
        directory: &MigrationDirectory,
    ) -> Result<Option<Arc<AdoptedGenesisAuthority>>, Diagnostic> {
        let Some(bytes) = read_optional_stored_genesis(directory)? else {
            return Ok(None);
        };
        let source = std::str::from_utf8(&bytes).map_err(|_| {
            legacy_import_failure(
                "migration_adopted_genesis_not_utf8",
                "stored adopted genesis is not valid UTF-8",
            )
        })?;
        let stored = parse_adopted_genesis_authority(
            DocumentId::new("typebridge-stored-adopted-genesis.typeql")?,
            source,
        )?;
        if stored.declared().declared_identity_fingerprint()
            != self.genesis_source.declared_identity_fingerprint()
        {
            return Err(legacy_import_failure(
                "migration_adopted_genesis_runner_mismatch",
                "runner genesis does not match the stored adopted-genesis authority",
            ));
        }

        let export = self.managed_database.schema_text().await.map_err(|error| {
            legacy_import_failure(
                "migration_adopted_extension_export_failed",
                "managed database schema cannot be exported for released-extension verification",
            )
            .with_detail("provider", error.to_string())
        })?;
        let expected_internal = expected_managed_internal_schema()?;
        let live = parse_adopted_genesis_authority_with_internal(
            DocumentId::new("typebridge-live-adopted-schema.typeql")?,
            &export,
            Some(&expected_internal),
        )?;
        stored.ensure_released_extension_identity_matches(&live)?;
        Ok(Some(Arc::new(stored)))
    }
}

fn legacy_import_failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        type_bridge_contract::diagnostic::DiagnosticCategory::InvalidContract,
        type_bridge_contract::diagnostic::DiagnosticCode::new(code)
            .expect("static legacy import diagnostic code"),
        message,
    )
}

fn same_adopted_authority(
    expected: &AdoptedGenesisAuthority,
    live: &AdoptedGenesisAuthority,
) -> bool {
    live.legacy_identity() == expected.legacy_identity()
        && live.declared().declared_identity_fingerprint()
            == expected.declared().declared_identity_fingerprint()
}

async fn finish_schema_guard<T>(
    guard: &mut type_bridge_orm::Transaction,
    primary: Result<T, Diagnostic>,
    operation: &'static str,
) -> Result<T, Diagnostic> {
    match (primary, guard.rollback().await) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(schema_guard_cleanup_failure(cleanup, None, operation)),
        (Err(primary), Err(cleanup)) => Err(schema_guard_cleanup_failure(
            cleanup,
            Some(&primary),
            operation,
        )),
    }
}

fn schema_guard_cleanup_failure(
    cleanup: OrmError,
    primary: Option<&Diagnostic>,
    operation: &'static str,
) -> Diagnostic {
    let mut diagnostic = legacy_import_failure(
        "migration_legacy_schema_guard_cleanup_uncertain",
        "managed schema guard termination was not acknowledged; a fresh runner must recover",
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

fn combine_diagnostics(
    code: &'static str,
    message: &'static str,
    primary: Diagnostic,
    secondary: Diagnostic,
) -> Diagnostic {
    legacy_import_failure(code, message)
        .with_detail("primary_code", primary.code().as_str().to_owned())
        .with_detail("primary", primary.to_string())
        .with_detail("secondary_code", secondary.code().as_str().to_owned())
        .with_detail("secondary", secondary.to_string())
}

fn reconcile_legacy_checkpoint_publication<T>(
    journal_result: Result<T, Diagnostic>,
    authority_postcheck: Result<(), Diagnostic>,
) -> Result<T, Diagnostic> {
    match (journal_result, authority_postcheck) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(authority)) => Err(legacy_import_failure(
            "migration_legacy_import_checkpoint_published_authority_changed",
            "companion checkpoint was published but filesystem authority changed; restore the exact authority and retry",
        )
        .with_detail("checkpoint_state", "published")
        .with_detail("recovery", "restore_exact_authority_and_retry")
        .with_detail("authority_code", authority.code().as_str().to_owned())
        .with_detail("authority", authority.to_string())),
        (Err(checkpoint), Err(authority)) => Err(combine_diagnostics(
            "migration_legacy_import_checkpoint_publication_uncertain",
            "companion checkpoint acknowledgment and filesystem authority validation both failed; a fresh runner must recover",
            checkpoint,
            authority,
        )
        .with_detail("checkpoint_state", "unacknowledged")
        .with_detail("recovery", "fresh_runner_required")),
    }
}

fn combine_boundary_release(
    boundary: Result<(), Diagnostic>,
    release: Result<(), Diagnostic>,
) -> Result<(), Diagnostic> {
    match (boundary, release) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(primary), Err(secondary)) => Err(combine_diagnostics(
            "migration_legacy_boundary_and_release_failed",
            "legacy pair validation and lease release both failed",
            primary,
            secondary,
        )),
    }
}

fn managed_state_derivation_diagnostic(error: DeltaError) -> Diagnostic {
    match error {
        DeltaError::Contract(diagnostic) => diagnostic,
        DeltaError::Schema(diagnostics) => diagnostics
            .iter()
            .next()
            .map(|entry| entry.diagnostic().clone())
            .unwrap_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_basis_state_unavailable",
                    "applied basis cannot be bound to an exact managed state",
                )
            }),
    }
}

fn reconcile_verify_snapshots(
    basis_before: &BTreeSet<MigrationId>,
    basis_after: &BTreeSet<MigrationId>,
    before: LegacyBindingReadInspection,
    after: LegacyBindingReadInspection,
) -> Option<Diagnostic> {
    if basis_before != basis_after || before.snapshot != after.snapshot {
        return Some(legacy_import_failure(
            "migration_legacy_verify_mixed_time",
            "migration authority changed while the live schema was exported",
        ));
    }
    before.drift.or(after.drift)
}

fn read_guard_cleanup_failure(cleanup: OrmError, primary: Option<&Diagnostic>) -> Diagnostic {
    let mut diagnostic = legacy_import_failure(
        "migration_legacy_read_guard_cleanup_uncertain",
        "read-only legacy binding snapshot termination was not acknowledged",
    )
    .with_detail("cleanup", cleanup.to_string());
    if let Some(primary) = primary {
        diagnostic = diagnostic
            .with_detail("primary_code", primary.code().as_str().to_owned())
            .with_detail("primary", primary.to_string());
    }
    diagnostic
}

async fn load_verified_legacy_partition(
    transaction: &mut type_bridge_orm::Transaction,
    expectation: LegacyCutoverSentinelExpectation<'_>,
    unreadable_message: &'static str,
) -> Result<VerifiedLegacyAppliedPartition, Diagnostic> {
    TypeDbStateStore::load_verified_legacy_partition_in_transaction(transaction, expectation)
        .await
        .map_err(|error: LegacyCutoverSentinelError| {
            if error.is_contract_violation() {
                legacy_import_failure(
                    "migration_legacy_cutover_sentinel_invalid",
                    "managed legacy ledger violates the exact V2 cutover-sentinel contract",
                )
                .with_detail("sentinel", error.to_string())
            } else {
                legacy_import_failure(
                    "migration_legacy_applied_set_unreadable",
                    unreadable_message,
                )
                .with_detail("legacy", error.to_string())
            }
        })
}

#[cfg(test)]
async fn load_legacy_applied_read_only(
    managed_database: &Database,
    expectation: LegacyCutoverSentinelExpectation<'_>,
) -> Result<Vec<AppliedMigrationRecord>, Diagnostic> {
    let mut transaction = managed_database.read_transaction().await.map_err(|error| {
        legacy_import_failure(
            "migration_legacy_import_ledger_unreadable",
            "legacy applied ledger cannot be read from a managed read-only snapshot",
        )
        .with_detail("provider", error.to_string())
    })?;
    let loaded = load_verified_legacy_partition(
        &mut transaction,
        expectation,
        "legacy applied ledger cannot be read from the managed database",
    )
    .await
    .map(VerifiedLegacyAppliedPartition::into_applied);
    let close = transaction.close().await;
    match (loaded, close) {
        (Ok(applied), Ok(())) => Ok(applied),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(read_guard_cleanup_failure(cleanup, None)),
        (Err(primary), Err(cleanup)) => Err(read_guard_cleanup_failure(cleanup, Some(&primary))),
    }
}

async fn load_pending_legacy_applied_read_only(
    managed_database: &Database,
    binding: &LegacyBridgeBinding,
) -> Result<Vec<AppliedMigrationRecord>, Diagnostic> {
    let mut transaction = managed_database.read_transaction().await.map_err(|error| {
        legacy_import_failure(
            "migration_legacy_import_ledger_unreadable",
            "legacy applied ledger cannot be read from a managed read-only snapshot",
        )
        .with_detail("provider", error.to_string())
    })?;
    let loaded = binding
        .validate_pending_import_contents(&mut transaction)
        .await
        .map(VerifiedLegacyAppliedPartition::into_applied);
    let close = transaction.close().await;
    match (loaded, close) {
        (Ok(applied), Ok(())) => Ok(applied),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(read_guard_cleanup_failure(cleanup, None)),
        (Err(primary), Err(cleanup)) => Err(read_guard_cleanup_failure(cleanup, Some(&primary))),
    }
}

async fn inspect_legacy_binding_read_only(
    managed_database: &Database,
    binding: &LegacyExecutionBinding,
    managed_scope: &str,
) -> Result<LegacyBindingReadInspection, Diagnostic> {
    let mut transaction = managed_database.read_transaction().await.map_err(|error| {
        legacy_import_failure(
            "migration_legacy_binding_read_unavailable",
            "read-only verification cannot open a managed snapshot",
        )
        .with_detail("provider", error.to_string())
    })?;
    let observed = binding
        .inspect_read_only(&mut transaction, managed_scope)
        .await;
    let close = transaction.close().await;
    match (observed, close) {
        (Ok(drift), Ok(())) => Ok(drift),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup)) => Err(read_guard_cleanup_failure(cleanup, None)),
        (Err(primary), Err(cleanup)) => Err(read_guard_cleanup_failure(cleanup, Some(&primary))),
    }
}

#[derive(Serialize)]
struct LegacyCutoverAppliedSetWire<'a> {
    algorithm: &'a str,
    canonicalization: &'a str,
    digest: &'a str,
}

#[derive(Serialize)]
struct LegacyCutoverPreimage<'a> {
    applied_set: LegacyCutoverAppliedSetWire<'a>,
    bridge_manifest_sha256: &'a str,
    journal_database: &'a str,
    legacy_identity: &'a Fingerprint,
    managed_database: &'a str,
    managed_scope: &'a str,
}

fn legacy_cutover_fingerprint(
    applied_set: &type_bridge_schema_migration::LegacyAppliedSetDigest,
    bridge_manifest_digest: &str,
    managed_database: &str,
    journal_database: &str,
    managed_scope: &str,
    legacy_identity: &Fingerprint,
) -> Result<String, Diagnostic> {
    let preimage = LegacyCutoverPreimage {
        applied_set: LegacyCutoverAppliedSetWire {
            algorithm: applied_set.algorithm(),
            canonicalization: applied_set.canonicalization(),
            digest: applied_set.as_str(),
        },
        bridge_manifest_sha256: bridge_manifest_digest,
        managed_database,
        journal_database,
        managed_scope,
        legacy_identity,
    };
    let bytes = to_canonical_json(&preimage)?;
    Ok(Fingerprint::compute(
        FingerprintDomain::new("typebridge.migration.legacy-cutover-anchor")?,
        CanonicalizationVersion::new("typebridge.legacy-cutover-anchor/v1")?,
        None,
        &bytes,
    )
    .digest()
    .to_hex())
}

async fn load_legacy_cutover_anchor(
    transaction: &mut type_bridge_orm::Transaction,
    managed_scope: &str,
) -> Result<Option<String>, Diagnostic> {
    observe_legacy_cutover_anchor(transaction, managed_scope)
        .await
        .map_err(LegacyAnchorObservationError::into_diagnostic)
}

enum LegacyAnchorObservationError {
    Drift(Diagnostic),
    Infrastructure(Diagnostic),
}

impl LegacyAnchorObservationError {
    fn into_diagnostic(self) -> Diagnostic {
        match self {
            Self::Drift(diagnostic) | Self::Infrastructure(diagnostic) => diagnostic,
        }
    }
}

async fn observe_legacy_cutover_anchor(
    transaction: &mut type_bridge_orm::Transaction,
    managed_scope: &str,
) -> Result<Option<String>, LegacyAnchorObservationError> {
    let schema_query = "match entity $t; fetch { \"label\": label($t) };";
    let schema_values = legacy_anchor_query_values(transaction, schema_query)
        .await
        .map_err(LegacyAnchorObservationError::Infrastructure)?;
    let mut anchor_type_present = false;
    for document in &schema_values {
        let label = document
            .get("label")
            .and_then(|value| value.get("value").unwrap_or(value).as_str())
            .ok_or_else(|| {
                LegacyAnchorObservationError::Infrastructure(legacy_import_failure(
                    "migration_legacy_import_anchor_provider_contract",
                    "legacy cutover anchor schema fetch returned a malformed label",
                ))
            })?;
        anchor_type_present |= label == LEGACY_CUTOVER_ENTITY;
    }
    if !anchor_type_present {
        return Ok(None);
    }

    let existence_query =
        format!("match $anchor isa {LEGACY_CUTOVER_ENTITY}; fetch {{ \"exists\": true }};");
    let existence = legacy_anchor_query_values(transaction, &existence_query)
        .await
        .map_err(LegacyAnchorObservationError::Infrastructure)?;
    let detail_query = format!(
        "match $anchor isa {LEGACY_CUTOVER_ENTITY}, has {LEGACY_CUTOVER_KEY} $key, has {LEGACY_CUTOVER_SCOPE} $scope, has {LEGACY_CUTOVER_FINGERPRINT} $fingerprint; fetch {{ \"key\": $key, \"scope\": $scope, \"fingerprint\": $fingerprint }};"
    );
    let values = legacy_anchor_query_values(transaction, &detail_query)
        .await
        .map_err(LegacyAnchorObservationError::Infrastructure)?;
    parse_legacy_cutover_anchor(existence, values, managed_scope)
        .map_err(LegacyAnchorObservationError::Drift)
}

fn parse_legacy_cutover_anchor(
    existence: Vec<serde_json::Value>,
    values: Vec<serde_json::Value>,
    managed_scope: &str,
) -> Result<Option<String>, Diagnostic> {
    if existence.len() > 1 {
        return Err(legacy_import_failure(
            "migration_legacy_import_anchor_duplicate",
            "managed database carries multiple legacy cutover anchors",
        ));
    }
    if existence.is_empty() && values.is_empty() {
        return Ok(None);
    }
    if existence.len() != 1 || values.len() != 1 {
        return Err(legacy_import_failure(
            "migration_legacy_import_anchor_malformed",
            "managed legacy cutover singleton is missing required exact fields",
        ));
    }
    let document = &values[0];
    let scalar = |field: &'static str| {
        document
            .get(field)
            .and_then(|value| value.get("value").unwrap_or(value).as_str())
            .ok_or_else(|| {
                legacy_import_failure(
                    "migration_legacy_import_anchor_malformed",
                    "managed legacy cutover anchor has a malformed scalar field",
                )
                .with_detail("field", field)
            })
    };
    if scalar("key")? != LEGACY_CUTOVER_SINGLETON_KEY {
        return Err(legacy_import_failure(
            "migration_legacy_import_anchor_foreign",
            "managed database carries a foreign legacy cutover singleton key",
        ));
    }
    if scalar("scope")? != managed_scope {
        return Err(legacy_import_failure(
            "migration_legacy_import_anchor_foreign",
            "managed database carries a legacy cutover anchor for another scope",
        ));
    }
    let value = scalar("fingerprint")?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(legacy_import_failure(
            "migration_legacy_import_anchor_malformed",
            "managed legacy cutover anchor fingerprint is malformed",
        ));
    }
    Ok(Some(value.to_owned()))
}

async fn legacy_anchor_query_values(
    transaction: &mut type_bridge_orm::Transaction,
    query: &str,
) -> Result<Vec<serde_json::Value>, Diagnostic> {
    match transaction.query(query).await.map_err(|error| {
        legacy_import_failure(
            "migration_legacy_import_anchor_unreadable",
            "managed legacy cutover anchor cannot be read",
        )
        .with_detail("provider", error.to_string())
    })? {
        QueryResult::Documents(values) | QueryResult::Rows(values) => Ok(values),
        QueryResult::Ok => Err(legacy_import_failure(
            "migration_legacy_import_anchor_provider_contract",
            "legacy cutover anchor fetch returned no document result",
        )),
    }
}

async fn insert_legacy_cutover_anchor(
    transaction: &mut type_bridge_orm::Transaction,
    managed_scope: &str,
    fingerprint: &str,
) -> Result<(), Diagnostic> {
    let query = format!(
        "insert $anchor isa {LEGACY_CUTOVER_ENTITY}, has {LEGACY_CUTOVER_KEY} {}, has {LEGACY_CUTOVER_SCOPE} {}, has {LEGACY_CUTOVER_FINGERPRINT} {};",
        typeql_string_literal(LEGACY_CUTOVER_SINGLETON_KEY),
        typeql_string_literal(managed_scope),
        typeql_string_literal(fingerprint),
    );
    transaction.query(&query).await.map_err(|error| {
        legacy_import_failure(
            "migration_legacy_import_anchor_insert_failed",
            "managed legacy cutover anchor cannot be staged exactly",
        )
        .with_detail("provider", error.to_string())
    })?;
    Ok(())
}

fn typeql_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("UTF-8 strings are JSON representable")
}

fn expected_managed_internal_schema() -> Result<DeclaredSchema, Diagnostic> {
    released_typeql_to_declared_projection(
        DocumentId::new("typebridge-managed-fence-schema.typeql")?,
        MANAGED_FENCE_SCHEMA_TYPEQL,
    )
    .map_err(|_| {
        legacy_import_failure(
            "migration_legacy_import_internal_contract_invalid",
            "frozen canonical control schema cannot be normalized",
        )
    })
}

fn read_stored_genesis(directory: &MigrationDirectory) -> Result<Vec<u8>, Diagnostic> {
    read_optional_stored_genesis(directory)?.ok_or_else(|| {
        legacy_import_failure(
            "migration_legacy_import_genesis_unreadable",
            "stored adopted genesis cannot be inspected",
        )
    })
}

fn read_optional_stored_genesis(
    directory: &MigrationDirectory,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    let file = match directory.open_regular_readonly(ADOPTED_GENESIS_FILE_NAME.as_ref()) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(legacy_import_failure(
                "migration_legacy_import_genesis_unreadable",
                "stored adopted genesis cannot be read as a regular file",
            ));
        }
    };
    let mut bytes = Vec::new();
    file.take(
        u64::try_from(MAX_TYPEQL_SCHEMA_BYTES)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut bytes)
    .map_err(|_| {
        legacy_import_failure(
            "migration_legacy_import_genesis_unreadable",
            "stored adopted genesis cannot be read",
        )
    })?;
    if bytes.len() > MAX_TYPEQL_SCHEMA_BYTES {
        return Err(legacy_import_failure(
            "migration_legacy_import_genesis_oversized",
            "stored adopted genesis exceeds the schema byte ceiling",
        ));
    }
    Ok(Some(bytes))
}

fn open_canonical_directory(path: &Path) -> Result<MigrationDirectory, Diagnostic> {
    MigrationDirectory::open_ambient(path).map_err(|_| {
        legacy_import_failure(
            "migration_typedb_directory_unreadable",
            "canonical migration directory cannot be opened",
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, QueryResult, TransactionOps, TxType,
    };

    use super::*;

    fn adopted_authority(source: &str) -> AdoptedGenesisAuthority {
        parse_adopted_genesis_authority(
            DocumentId::new("runner-adopted-authority.typeql").expect("document id"),
            source,
        )
        .expect("released authority fixture")
    }

    #[test]
    fn initial_adoption_comparison_rejects_opaque_definition_drift() {
        let expected = adopted_authority(
            "define\nentity person;\n\
             fun inspect() -> integer:\n\
               opaque-token\n\
               return { 1 };\n\
             struct payload, value note string;\n",
        );
        let equivalent = adopted_authority(
            "define\nentity person;\n\
             fun inspect () -> integer:\n\
               opaque-token /* formatting-only */\n\
               return { 1 };\n\
             struct payload, value note string;\n",
        );
        let body_drift = adopted_authority(
            "define\nentity person;\n\
             fun inspect() -> integer:\n\
               opaque-token\n\
               return { 2 };\n\
             struct payload, value note string;\n",
        );
        let struct_drift = adopted_authority(
            "define\nentity person;\n\
             fun inspect() -> integer:\n\
               opaque-token\n\
               return { 1 };\n\
             struct payload, value count integer;\n",
        );

        assert!(same_adopted_authority(&expected, &equivalent));
        assert!(!same_adopted_authority(&expected, &body_drift));
        assert!(!same_adopted_authority(&expected, &struct_drift));
    }

    struct ReadOnlyInspectBackend {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        transaction_types: Arc<Mutex<Vec<TxType>>>,
        terminals: Arc<Mutex<Vec<&'static str>>>,
    }

    impl DriverBackend for ReadOnlyInspectBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.transaction_types
                .lock()
                .expect("transaction types")
                .push(tx_type);
            let responses = Arc::clone(&self.responses);
            let terminals = Arc::clone(&self.terminals);
            Box::pin(async move {
                Ok(Box::new(ReadOnlyInspectTransaction {
                    responses,
                    terminals,
                }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct ReadOnlyInspectTransaction {
        responses: Arc<Mutex<VecDeque<QueryResult>>>,
        terminals: Arc<Mutex<Vec<&'static str>>>,
    }

    type InspectionDatabase = (
        Database,
        Arc<Mutex<Vec<TxType>>>,
        Arc<Mutex<Vec<&'static str>>>,
    );

    impl TransactionOps for ReadOnlyInspectTransaction {
        fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            if typeql.starts_with("# typebridge-internal-legacy-state-schema-probe/v1\n") {
                return Box::pin(async { Ok(QueryResult::Documents(Vec::new())) });
            }
            let result = self
                .responses
                .lock()
                .expect("responses")
                .pop_front()
                .expect("configured response");
            Box::pin(async move { Ok(result) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.terminals.lock().expect("terminals").push("commit");
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.terminals.lock().expect("terminals").push("rollback");
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.terminals.lock().expect("terminals").push("close");
            Box::pin(async { Ok(()) })
        }
    }

    fn inspection_database(responses: Vec<QueryResult>) -> InspectionDatabase {
        let transaction_types = Arc::new(Mutex::new(Vec::new()));
        let terminals = Arc::new(Mutex::new(Vec::new()));
        let backend = ReadOnlyInspectBackend {
            responses: Arc::new(Mutex::new(responses.into())),
            transaction_types: Arc::clone(&transaction_types),
            terminals: Arc::clone(&terminals),
        };
        (
            Database::with_backend(Box::new(backend), "managed".to_owned()),
            transaction_types,
            terminals,
        )
    }

    #[tokio::test]
    async fn verify_binding_brackets_export_with_acknowledged_read_snapshots() {
        let (database, transaction_types, terminals) = inspection_database(vec![
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
        ]);
        let before = inspect_legacy_binding_read_only(
            &database,
            &LegacyExecutionBinding::Absent,
            "scope\0\u{0008}\u{000c}",
        )
        .await
        .expect("read-only inspection");
        let after = inspect_legacy_binding_read_only(
            &database,
            &LegacyExecutionBinding::Absent,
            "scope\0\u{0008}\u{000c}",
        )
        .await
        .expect("read-only inspection");
        assert!(before.drift.is_none());
        assert!(after.drift.is_none());
        assert_eq!(before.snapshot, after.snapshot);
        assert_eq!(
            transaction_types
                .lock()
                .expect("transaction types")
                .as_slice(),
            &[TxType::Read, TxType::Read]
        );
        assert_eq!(
            terminals.lock().expect("terminals").as_slice(),
            &["close", "close"]
        );
    }

    #[tokio::test]
    async fn legacy_import_ledger_preflight_is_read_only_and_acknowledges_close() {
        let (database, transaction_types, terminals) =
            inspection_database(vec![QueryResult::Documents(Vec::new())]);
        let applied =
            load_legacy_applied_read_only(&database, LegacyCutoverSentinelExpectation::Absent)
                .await
                .expect("read-only legacy ledger load");
        assert!(applied.is_empty());
        assert_eq!(
            transaction_types
                .lock()
                .expect("transaction types")
                .as_slice(),
            &[TxType::Read]
        );
        assert_eq!(terminals.lock().expect("terminals").as_slice(), &["close"]);
    }

    #[tokio::test]
    async fn verify_binding_distinguishes_semantic_drift_from_provider_failure() {
        let exact_anchor = serde_json::json!({
            "key": LEGACY_CUTOVER_SINGLETON_KEY,
            "scope": "scope",
            "fingerprint": "a".repeat(64),
        });
        let (drift_database, _, _) = inspection_database(vec![
            QueryResult::Documents(vec![serde_json::json!({
                "label": LEGACY_CUTOVER_ENTITY,
            })]),
            QueryResult::Documents(vec![serde_json::json!({"exists": true})]),
            QueryResult::Documents(vec![exact_anchor]),
        ]);
        let drift = inspect_legacy_binding_read_only(
            &drift_database,
            &LegacyExecutionBinding::Absent,
            "scope",
        )
        .await
        .expect("semantic pair drift is reportable")
        .drift
        .expect("drift finding");
        assert_eq!(
            drift.code().as_str(),
            "migration_legacy_import_anchor_without_bridge"
        );

        let (failed_database, _, _) = inspection_database(vec![QueryResult::Ok]);
        let error = inspect_legacy_binding_read_only(
            &failed_database,
            &LegacyExecutionBinding::Absent,
            "scope",
        )
        .await
        .expect_err("provider contract failures are command errors, not drift");
        assert_eq!(
            error.code().as_str(),
            "migration_legacy_import_anchor_provider_contract"
        );
    }

    #[test]
    fn verify_reports_mixed_time_authority_instead_of_accepting_either_snapshot() {
        let basis = BTreeSet::from([MigrationId::new("app", "0001_bridge").expect("migration id")]);
        let before = LegacyBindingReadInspection {
            drift: None,
            snapshot: Some(LegacyBindingSnapshot {
                applied_set: None,
                anchor_fingerprint: None,
                sentinel_fingerprint: None,
            }),
        };
        let after = LegacyBindingReadInspection {
            drift: None,
            snapshot: Some(LegacyBindingSnapshot {
                applied_set: None,
                anchor_fingerprint: Some("a".repeat(64)),
                sentinel_fingerprint: Some("a".repeat(64)),
            }),
        };
        let diagnostic = reconcile_verify_snapshots(&basis, &basis, before, after)
            .expect("changed authority is drift");
        assert_eq!(
            diagnostic.code().as_str(),
            "migration_legacy_verify_mixed_time"
        );
    }

    #[test]
    fn anchor_parser_rejects_foreign_multiple_and_malformed_singletons() {
        let existence = vec![serde_json::json!({"exists": true})];
        let exact = serde_json::json!({
            "key": LEGACY_CUTOVER_SINGLETON_KEY,
            "scope": "scope",
            "fingerprint": "a".repeat(64),
        });
        assert_eq!(
            parse_legacy_cutover_anchor(existence.clone(), vec![exact], "scope")
                .expect("exact singleton"),
            Some("a".repeat(64))
        );
        let foreign = serde_json::json!({
            "key": LEGACY_CUTOVER_SINGLETON_KEY,
            "scope": "other",
            "fingerprint": "a".repeat(64),
        });
        assert_eq!(
            parse_legacy_cutover_anchor(existence.clone(), vec![foreign], "scope")
                .expect_err("foreign scope")
                .code()
                .as_str(),
            "migration_legacy_import_anchor_foreign"
        );
        assert_eq!(
            parse_legacy_cutover_anchor(
                vec![serde_json::json!({"exists": true}); 2],
                Vec::new(),
                "scope",
            )
            .expect_err("multiple anchors")
            .code()
            .as_str(),
            "migration_legacy_import_anchor_duplicate"
        );
        assert_eq!(
            parse_legacy_cutover_anchor(
                existence,
                vec![serde_json::json!({"scope": "scope"})],
                "scope",
            )
            .expect_err("malformed anchor")
            .code()
            .as_str(),
            "migration_legacy_import_anchor_malformed"
        );
    }

    #[test]
    fn anchor_literals_escape_every_control_character() {
        let literal = typeql_string_literal("scope\0\u{0008}\u{000c}\n\r\t\"");
        assert_eq!(literal, "\"scope\\u0000\\b\\f\\n\\r\\t\\\"\"");
        assert!(!literal.chars().any(|character| character.is_control()));
    }

    #[test]
    fn anchor_fingerprint_contract_matches_golden() {
        let applied_set =
            type_bridge_schema_migration::LegacyAppliedSetDigest::new("11".repeat(32))
                .expect("applied set digest");
        let legacy_identity = Fingerprint::compute(
            FingerprintDomain::new("typebridge.test.legacy-identity").expect("domain"),
            CanonicalizationVersion::new("typebridge.test-legacy-identity/v1")
                .expect("canonicalization"),
            None,
            b"legacy identity bytes",
        );
        let fingerprint = legacy_cutover_fingerprint(
            &applied_set,
            &"22".repeat(32),
            "managed-db",
            "managed-db__typebridge_journal",
            "scope",
            &legacy_identity,
        )
        .expect("anchor fingerprint");
        assert_eq!(
            fingerprint,
            "bc3944d73f853fc73d65351c36f5e9ca0789c9eb56d6346a760058905dd7790b"
        );
    }

    #[test]
    fn checkpoint_publication_reports_recoverable_and_uncertain_authority_failures() {
        assert_eq!(
            reconcile_legacy_checkpoint_publication::<u8>(Ok(7), Ok(()))
                .expect("exact publication"),
            7
        );

        let checkpoint = legacy_import_failure(
            "migration_test_checkpoint_failed",
            "checkpoint fixture failed",
        );
        assert_eq!(
            reconcile_legacy_checkpoint_publication::<()>(Err(checkpoint), Ok(()))
                .expect_err("checkpoint failure is preserved")
                .code()
                .as_str(),
            "migration_test_checkpoint_failed"
        );

        let authority = legacy_import_failure(
            "migration_test_authority_changed",
            "authority fixture changed",
        );
        assert_eq!(
            reconcile_legacy_checkpoint_publication(Ok(()), Err(authority))
                .expect_err("published checkpoint with drift is recoverable")
                .code()
                .as_str(),
            "migration_legacy_import_checkpoint_published_authority_changed"
        );

        let checkpoint = legacy_import_failure(
            "migration_test_checkpoint_failed",
            "checkpoint fixture failed",
        );
        let authority = legacy_import_failure(
            "migration_test_authority_changed",
            "authority fixture changed",
        );
        assert_eq!(
            reconcile_legacy_checkpoint_publication::<()>(Err(checkpoint), Err(authority))
                .expect_err("unacknowledged publication plus drift is uncertain")
                .code()
                .as_str(),
            "migration_legacy_import_checkpoint_publication_uncertain"
        );
    }
}
