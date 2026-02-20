//! Schema-specific error types.

use thiserror::Error;

/// Errors specific to schema management operations.
#[derive(Debug, Error)]
pub enum SchemaError {
    /// Schema validation failed (e.g. duplicate attribute with different value types).
    #[error("Schema validation error: {message}")]
    Validation {
        /// Description of the validation failure.
        message: String,
    },

    /// Schema conflict detected during diff.
    #[error("Schema conflict: {message}")]
    Conflict {
        /// Description of the conflict.
        message: String,
    },

    /// Schema sync to database failed.
    #[error("Schema sync error: {0}")]
    Sync(String),
}
