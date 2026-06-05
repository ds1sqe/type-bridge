//! Migration specification IR for type-bridge.
//!
//! This crate defines the serde contract that Python migration authoring lowers
//! into, plus pure validation and checksum gates used before execution.

#![warn(missing_docs)]

pub mod checksum;
pub mod error;
pub mod graph;
pub mod spec;

pub use checksum::{
    ChecksumDrift, check_checksum_drift, checksum_drift_errors, migration_file_checksum,
};
pub use error::{MigrationError, Result};
pub use graph::{AppliedMigrationRecord, MigrationValidationError, ValidationCode, validate_graph};
pub use spec::{MigrationDependencySpec, MigrationGraph, MigrationSpec, OperationSpec};
