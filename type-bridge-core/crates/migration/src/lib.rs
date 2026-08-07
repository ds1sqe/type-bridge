//! Migration specification IR for type-bridge.
//!
//! This crate defines the serde contract that Python migration authoring lowers
//! into, plus pure validation, checksum gates, the transaction-aware planner,
//! and the async executor that runs each step over a `Database`.

#![deny(missing_docs)]

// The legacy CLI source is shared by the standalone binary and the Python
// extension. Give the library its package name so the shared module can keep
// using the same absolute imports in both compilation contexts.
extern crate self as type_bridge_migration;

pub mod adoption;
pub mod author;
pub mod backfill;
pub mod checksum;
pub mod error;
pub mod executor;
pub mod graph;
pub mod loader;
pub mod plan;
pub mod recovery;
pub mod spec;
pub mod state;

/// Released V1 command-line parser and command runner.
///
/// The standalone `type-bridge-migration` binary and the Python wheel entry
/// point both call this module, keeping one parser, one execution path, and the
/// same stdout/stderr and exit-code contract.
#[path = "bin/type_bridge_migration.rs"]
pub mod legacy_cli;

#[cfg(test)]
pub(crate) mod testing;

pub use adoption::{
    LEGACY_ADOPTION_METADATA_V2, LEGACY_IGNORED_SOURCE_METADATA_V1,
    LEGACY_SIDECAR_ADOPTION_METADATA_V1, LegacyAdoptionHistory, LegacyAdoptionMetadata,
    LegacyDirectoryAuthority, LegacyDirectoryEntry, LegacyIgnoredSourceMetadata,
    LegacyMetadataRevision, LegacySchemaEffect, LegacySidecarAdoptionMetadata,
    MAX_LEGACY_ARTIFACT_BYTES, MAX_LEGACY_DIRECTORY_ENTRIES, MAX_LEGACY_HISTORY_BYTES,
    VerifiedLegacyHead, load_adoption_history, reconstruct_legacy_head,
};
pub use author::map_schema_diff;
pub use backfill::{BackfillResult, execute_backfill};
pub use checksum::{
    ChecksumDrift, check_checksum_drift, checksum_drift_errors, migration_file_checksum,
};
pub use error::{MigrationError, Result};
pub use executor::{MigrationResult, execute_migration, execute_plan, execute_plan_with_run_log};
pub use graph::{AppliedMigrationRecord, MigrationValidationError, ValidationCode, validate_graph};
pub use loader::{load_dir, load_dir_checked, load_sidecar};
pub use plan::{
    ExecutionPlan, ExecutionStep, MigrationAction, MigrationExecution, OperationKind, StepKind,
    plan,
};
pub use recovery::{
    CheckedExecutionPlan, CheckedExecutionStep, CheckedMigrationExecution,
    CheckedMigrationIdentity, ExecutionStepId, PendingProof, RecoveryExecutionResult,
    RecoveryFuture, RecoveryMigrationResult, RecoveryMigrationStatus, RecoveryPlanStatus,
    StepExecutionOutcome, StepExecutionResult, StepRecoveryController, StepRecoveryDecision,
    StepRecoveryEvent, StepRecoveryEventKind, execute_recovery_plan, plan_recovery,
    prepare_recovery_plan,
};
pub use spec::{MigrationDependencySpec, MigrationGraph, MigrationSpec, OperationSpec};
pub use state::{
    InMemoryStateStore, LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_CUTOVER_SENTINEL_MIGRATION_ID, LEGACY_CUTOVER_SENTINEL_NAME,
    LEGACY_WRITER_CUTOVER_MESSAGE, LegacyCutoverSentinelError, LegacyCutoverSentinelExpectation,
    MigrationExecutorInfo, MigrationRunRecord, MigrationStateSchemaKind, MigrationStateStore,
    TypeDbStateStore, VerifiedLegacyAppliedPartition, applied_migration_entity_label,
    collect_executor_info, finished_run_record, is_migration_state_type, migration_state_schema,
    migration_timestamp_now, require_legacy_writer_open, require_legacy_writer_open_in_transaction,
    started_run_record,
};
