//! Verified offline schema migration contracts and provider lowering policy.

#![deny(missing_docs)]

mod apply_plan;
mod catalog;
mod coordinator;
mod directory;
mod execution;
mod generate;
mod history;
mod history_bundle;
mod legacy;
pub mod lowering;
mod manifest;
mod policy;
pub mod profile;
mod rollback_plan;
mod verify;

pub use apply_plan::{
    MigrationApplyPlanError, MigrationApplyTarget, VerifiedMigrationApplyManifest,
    VerifiedMigrationApplyPlan, VerifiedMigrationApplyStep, VerifiedMigrationTransactionGroup,
    build_verified_migration_apply_plan, build_verified_migration_apply_preview,
    partition_transaction_groups,
};
pub use catalog::MigrationCatalog;
pub use coordinator::{
    BackfillExecutionFuture, GroupCommitFailure, GroupCommitFuture, MigrationBackfillObservation,
    MigrationExecutionDirection, MigrationExecutionOutcome, MigrationExecutionPosition,
    MigrationExecutionProvider, MigrationExecutionReport, MigrationExecutionReportPosition,
    MigrationExecutionStatus, MigrationRollbackOutcome, PreparedMigrationGroup,
    await_interruptible_operation, execute_verified_migration_apply_plan,
    execute_verified_migration_apply_plan_controlled, execute_verified_migration_rollback_plan,
    execute_verified_migration_rollback_plan_controlled, require_authorized_apply_plan,
    require_authorized_rollback_plan,
};
pub use directory::{
    MigrationAuthoringLock, MigrationDirectory, MigrationDirectoryEntry,
    validate_portable_direct_child,
};
pub use execution::{
    AppliedRecord, BackfillCompletionEvidence, BackfillEventRecord, BackfillExecutionCounts,
    BackfillExecutionDirection, BackfillRecoveryObservation, ExecutionBindingToken, ExecutionFence,
    ExecutionFuture, ExecutionScope, GroupCommitCertainty, GroupEventRecord, GroupJournalEventKind,
    GroupRecoveryDecision, GroupRecoveryObservation, JournalEntry, JournalSequence, LeaseHolderId,
    MAX_MIGRATION_BACKFILL_OBSERVATIONS, MAX_MIGRATION_EXECUTION_GROUPS, MigrationCancellation,
    MigrationExecutionControl, MigrationExecutionJournal, MigrationExecutionResourceLimits,
    MigrationLease, MigrationLeaseStore, OpenPlanRecord, OpenRollbackPlanRecord, PlanRecord,
    RollbackPlanRecord, RollbackStepEventRecord, RolledBackRecord, active_applied_entries,
    decide_backfill_recovery, decide_group_recovery,
};
pub use generate::{
    BackfillMigrationGenerationRequest, GeneratedMigration, MigrationGenerationOutcome,
    MigrationGenerationRequest, MigrationPreviewError, generate_backfill_migration,
    generate_next_migration, render_migration_preview, try_acquire_migration_authoring_lock,
    write_generated_migration_under_lock,
};
pub use history::{
    CanonicalMigrationHistoryEvidence, MigrationHistoryGraph,
    canonical_history_declared_legacy_bridge_count_in, canonical_history_declares_legacy_bridge_in,
    discover_verified_migration_chain, discover_verified_migration_chain_in,
    discover_verified_migration_chain_with_evidence_in, discover_verified_migrations,
    discover_verified_migrations_in, require_adoption_authority_pair,
    require_adoption_authority_pair_state,
};
pub use history_bundle::{
    MAX_MIGRATION_HISTORY_BUNDLE_BYTES, MIGRATION_HISTORY_BUNDLE_FINGERPRINT_CANONICALIZATION,
    MIGRATION_HISTORY_BUNDLE_FINGERPRINT_DOMAIN, MIGRATION_HISTORY_BUNDLE_V1,
    VerifiedMigrationHistoryBundle, VerifiedMigrationHistoryBundleEntry,
    decode_verified_migration_history_bundle, encode_verified_migration_history_bundle,
};
pub use legacy::{
    LEGACY_APPLIED_SET_ALGORITHM, LEGACY_APPLIED_SET_CANONICALIZATION, LEGACY_CHECKSUM_ALGORITHM,
    LegacyAppliedSetDigest, LegacyMigrationAppLabel, LegacyMigrationChecksum, LegacyMigrationId,
    LegacyMigrationName, LegacyMigrationReference, build_legacy_frontier_bridge,
};
pub use lowering::{
    SchemaFactCatalog, SchemaLoweringBinding, SchemaLoweringDiagnostic, SchemaLoweringPlan,
    StatementOperationKind, StatementUnit, TypeQlStatement, TypeQlVerb, lower_schema_delta,
};
pub use manifest::{
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    decode_verified_manifest, encode_verified_manifest, verified_manifest_digest,
};
pub use policy::{MigrationApplyApproval, MigrationSafetyPolicy, SafetyPolicyDecision};
pub use rollback_plan::{
    VerifiedMigrationRollbackBackfillStep, VerifiedMigrationRollbackManifest,
    VerifiedMigrationRollbackOperation, VerifiedMigrationRollbackPlan,
    VerifiedMigrationRollbackStep, build_verified_migration_rollback_plan,
    build_verified_migration_rollback_preview,
};
pub use verify::{MigrationDriftFinding, MigrationVerifyReport, verify_migration_state};

pub use profile::{
    AnnotationKind, AnnotationSubjectKind, AnnotationTransition, EvidenceFlag, EvidenceRequirement,
    FactKind, FactTransition, InterfaceDefault, InterfaceKind, LoweringMechanism, SafetyScenario,
    SafetyScenarioRule, SchemaLoweringProfile, TransitionRule, annotation_transition_rule,
    canonical_profile_bytes, fact_transition_rule, migration_runtime_capability_vocabulary,
    profile_fingerprint, schema_lowering_profile_binding, typedb_3_12_1_profile,
};
pub use type_bridge_contract::schema_lowering::{
    SCHEMA_LOWERING_PROFILE_CANONICALIZATION, SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN,
    SchemaLoweringProfileFingerprint, SchemaLoweringProfileId,
    TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID,
};
pub use type_bridge_schema::{SafetyClass, SafetyClassificationError, classify_operation_safety};
