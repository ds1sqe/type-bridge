//! Migration specification IR for type-bridge.
//!
//! This crate defines the serde contract that Python migration authoring lowers
//! into, plus pure validation, checksum gates, the transaction-aware planner,
//! and the async executor that runs each step over a `Database`.

#![warn(missing_docs)]

pub mod checksum;
pub mod error;
pub mod executor;
pub mod graph;
pub mod plan;
pub mod spec;

#[cfg(test)]
pub(crate) mod testing;

pub use checksum::{
    ChecksumDrift, check_checksum_drift, checksum_drift_errors, migration_file_checksum,
};
pub use error::{MigrationError, Result};
pub use executor::{MigrationResult, execute_plan};
pub use graph::{AppliedMigrationRecord, MigrationValidationError, ValidationCode, validate_graph};
pub use plan::{ExecutionPlan, ExecutionStep, MigrationAction, MigrationExecution, plan};
pub use spec::{MigrationDependencySpec, MigrationGraph, MigrationSpec, OperationSpec};
