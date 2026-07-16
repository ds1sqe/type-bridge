//! Verified offline schema migration contracts and provider lowering policy.

mod apply_plan;
mod coordinator;
mod execution;
mod generate;
pub mod lowering;
mod history;
mod manifest;
mod policy;
pub mod profile;

pub use apply_plan::{
    MigrationApplyPlanError, MigrationApplyTarget, VerifiedMigrationApplyManifest,
    VerifiedMigrationApplyPlan, VerifiedMigrationApplyStep,
    VerifiedMigrationTransactionGroup, build_verified_migration_apply_plan,
    partition_transaction_groups,
};
pub use coordinator::{
    GroupCommitFailure, GroupCommitFuture, MigrationExecutionOutcome,
    MigrationExecutionProvider, PreparedMigrationGroup,
    execute_verified_migration_apply_plan,
};
pub use execution::{
    AppliedRecord, ExecutionFence, ExecutionFuture, ExecutionScope,
    GroupCommitCertainty, GroupEventRecord, GroupJournalEventKind,
    GroupRecoveryDecision, GroupRecoveryObservation, JournalEntry,
    JournalSequence, LeaseHolderId, MigrationExecutionJournal, MigrationLease,
    MigrationLeaseStore, OpenPlanRecord, PlanRecord, decide_group_recovery,
};
pub use lowering::{
    SchemaFactCatalog, SchemaLoweringBinding, SchemaLoweringDiagnostic, SchemaLoweringPlan,
    StatementOperationKind, StatementUnit, TypeQlStatement, TypeQlVerb, lower_schema_delta,
};
pub use generate::{
    GeneratedMigration, MigrationGenerationOutcome, MigrationGenerationRequest,
    MigrationPreviewError, generate_next_migration, render_migration_preview,
    write_generated_migration,
};
pub use history::{
    MigrationHistoryGraph, discover_verified_migration_chain,
    discover_verified_migrations,
};
pub use manifest::{
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    decode_verified_manifest, encode_verified_manifest, verified_manifest_digest,
};
pub use policy::{
    MigrationApplyApproval, MigrationSafetyPolicy, SafetyPolicyDecision,
};

pub use profile::{
    AnnotationKind, AnnotationSubjectKind, AnnotationTransition, EvidenceFlag,
    EvidenceRequirement, FactKind, FactTransition, InterfaceDefault, InterfaceKind,
    LoweringMechanism, SafetyScenario, SafetyScenarioRule, SchemaLoweringProfile,
    TransitionRule, annotation_transition_rule, canonical_profile_bytes,
    fact_transition_rule, profile_fingerprint, schema_lowering_profile_binding,
    typedb_3_12_1_profile,
};
pub use type_bridge_schema::{
    SafetyClass, SafetyClassificationError, classify_operation_safety,
};
pub use type_bridge_contract::schema_lowering::{
    SCHEMA_LOWERING_PROFILE_CANONICALIZATION, SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN,
    SchemaLoweringProfileFingerprint, SchemaLoweringProfileId,
    TYPEDB_3_12_1_SCHEMA_LOWERING_PROFILE_ID,
};
