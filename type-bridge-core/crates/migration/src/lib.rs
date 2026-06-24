//! Migration specification IR for type-bridge.
//!
//! This crate defines the serde contract that Python migration authoring lowers
//! into, plus pure validation, checksum gates, the transaction-aware planner,
//! and the async executor that runs each step over a `Database`.

#![warn(missing_docs)]

pub mod backfill;
pub mod checksum;
pub mod error;
pub mod executor;
pub mod graph;
pub mod loader;
pub mod plan;
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
pub use loader::{load_dir, load_dir_checked, load_sidecar};
pub use plan::{ExecutionPlan, ExecutionStep, MigrationAction, MigrationExecution, StepKind, plan};
pub use spec::{MigrationDependencySpec, MigrationGraph, MigrationSpec, OperationSpec};
pub use state::{
    InMemoryStateStore, MigrationExecutorInfo, MigrationRunRecord, MigrationStateStore,
    TypeDbStateStore, collect_executor_info, finished_run_record, migration_timestamp_now,
    started_run_record,
};
