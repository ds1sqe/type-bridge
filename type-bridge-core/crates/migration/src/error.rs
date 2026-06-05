//! Error types for migration IR and validation boundaries.

use crate::checksum::ChecksumDrift;
use crate::graph::MigrationValidationError;
use thiserror::Error;

/// Crate-local result alias.
pub type Result<T> = std::result::Result<T, MigrationError>;

/// Migration specification errors.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// JSON serialization or deserialization failed.
    #[error("invalid migration specification JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Applied migration checksum does not match the loaded migration.
    #[error("{message}")]
    ChecksumDrift {
        /// Structured checksum drift details.
        drift: Box<ChecksumDrift>,
        /// Human-readable error message.
        message: String,
    },
    /// The requested CLI behavior is reserved for a later sub-plan.
    #[error("{feature} is not implemented until sub-plan {sub_plan}")]
    Unsupported {
        /// Feature or command name.
        feature: &'static str,
        /// Sub-plan number that owns the behavior.
        sub_plan: u8,
    },
    /// Graph validation failed; one or more structural errors were found.
    #[error("migration graph validation failed with {} error(s)", errors.len())]
    Planning {
        /// All validation errors discovered.
        errors: Vec<MigrationValidationError>,
    },
    /// An `OperationSpec` variant that has not been lowered to `RunTypeql` or
    /// `DefineSchema` was encountered by the planner.
    ///
    /// Granular typed ops (e.g. `AddAttribute`, `AddOwnership`) must be
    /// converted to `RunTypeql` by the Python executor's lowering pass (Phase 3)
    /// before they reach the Rust planner.
    #[error(
        "operation {kind} is not lowered for execution; lower granular ops to RunTypeql before planning"
    )]
    UnloweredOperation {
        /// Variant name of the unlowered operation.
        kind: String,
    },
    /// The requested target migration was not found in the graph.
    #[error("target migration not found: {target}")]
    TargetNotFound {
        /// The target name that could not be resolved.
        target: String,
    },
    /// The schema generator failed to produce TypeQL from a `DefineSchema` op.
    #[error("schema generation failed: {message}")]
    SchemaGeneration {
        /// Human-readable error message.
        message: String,
    },
    /// An applied-state storage operation failed at the ORM seam.
    ///
    /// Carries the ORM-layer failure (connection, transaction, or query
    /// execution) reworded for the migration error hierarchy. Raised by the
    /// TypeDB-backed [`MigrationStateStore`](crate::state::MigrationStateStore)
    /// when a state read or write cannot complete.
    #[error("migration state storage error: {message}")]
    State {
        /// Human-readable error message describing the storage failure.
        message: String,
    },
}
