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
    /// Graph validation failed; one or more structural errors were found.
    #[error("migration graph validation failed with {} error(s)", errors.len())]
    Planning {
        /// All validation errors discovered.
        errors: Vec<MigrationValidationError>,
    },
    /// An `OperationSpec` variant is intentionally unsupported by the Rust
    /// planner.
    #[error(
        "operation {kind} is not supported for Rust planning; use RunTypeql or supported typed ops"
    )]
    UnloweredOperation {
        /// Variant name of the unsupported operation.
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
    /// A sidecar file IO or JSON decode error in the native loader.
    #[error("migration loader error: {message}")]
    Loader {
        /// Human-readable error message describing the loader failure.
        message: String,
    },
    /// A backfill count query or write failed.
    ///
    /// Raised by [`backfill::execute_backfill`](crate::backfill::execute_backfill)
    /// when a count query, the backfill insert, or a transaction open fails.
    #[error("backfill execution error: {message}")]
    BackfillQuery {
        /// Human-readable error message describing the failure.
        message: String,
    },
    /// An executable migration has no checked artifact checksum, so stable
    /// recovery step identities cannot be constructed.
    #[error("migration {app_label}.{name} has no checked checksum for recovery execution")]
    MissingRecoveryChecksum {
        /// Application or migration package label.
        app_label: String,
        /// Migration file stem.
        name: String,
    },
    /// An external per-step recovery controller failed.
    #[error("migration recovery controller error: {message}")]
    Recovery {
        /// Human-readable controller failure.
        message: String,
    },
    /// A detected schema change has no canonical authoring lowering.
    ///
    /// The canonical mapper must never silently discard a `SchemaDiff` field
    /// (#166); a change it cannot express as a typed operation or canonical
    /// `RunTypeql` is surfaced as this error instead.
    #[error("unsupported schema change on {type_name}: {change}")]
    UnsupportedChange {
        /// The schema type the change applies to.
        type_name: String,
        /// Description of the unrepresentable change.
        change: String,
    },
    /// Authoring inputs are inconsistent (e.g. the diff references a type
    /// that is absent from the schema it was computed from).
    #[error("inconsistent authoring input: {message}")]
    AuthoringInput {
        /// Human-readable error message.
        message: String,
    },
}
