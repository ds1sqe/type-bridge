//! Error types for lifecycle hooks.

use thiserror::Error;

use super::CrudOperation;

/// Errors that can occur in lifecycle hooks.
#[derive(Debug, Error)]
pub enum HookError {
    /// A pre-hook rejected the operation.
    #[error("Hook '{hook_name}' rejected {operation:?}: {reason}")]
    Rejected {
        /// Stable human-readable name of the rejecting hook.
        hook_name: String,
        /// CRUD operation rejected by the hook.
        operation: CrudOperation,
        /// Caller-safe reason supplied by the hook.
        reason: String,
    },

    /// A hook encountered an internal error.
    #[error("Hook '{hook_name}' failed: {source}")]
    Internal {
        /// Stable human-readable name of the failing hook.
        hook_name: String,
        /// Error returned by the hook implementation.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
