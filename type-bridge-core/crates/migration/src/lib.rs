//! Migration specification IR for type-bridge.
//!
//! This crate defines the serde contract that Python migration authoring lowers
//! into, plus pure validation, checksum gates, the transaction-aware planner,
//! and the async executor that runs each step over a `Database`.

#![deny(missing_docs)]

#[cfg(test)]
#[macro_use]
mod typeql_test;

pub mod backfill;
pub mod checksum;
pub mod error;
pub mod executor;
pub mod graph;
pub mod native_step;
pub mod plan;
pub mod recovery;
pub mod spec;
pub mod state;

#[cfg(test)]
pub(crate) mod testing;

pub use backfill::{BackfillResult, execute_backfill};
pub use checksum::{
    ChecksumDrift, check_checksum_drift, checksum_drift_errors, migration_file_checksum,
};
pub use error::{MigrationError, Result};
pub use executor::{MigrationResult, execute_migration, execute_plan, execute_plan_with_run_log};
pub use graph::{AppliedMigrationRecord, MigrationValidationError, ValidationCode, validate_graph};
pub use native_step::NativeDataStep;
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

mod copy_plan;
