//! Error types for migration IR and validation boundaries.

use crate::checksum::ChecksumDrift;
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
}
