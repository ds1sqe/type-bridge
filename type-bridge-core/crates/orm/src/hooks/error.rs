//! Error types for lifecycle hooks.

use thiserror::Error;

use super::CrudOperation;

/// Errors that can occur in lifecycle hooks.
#[derive(Debug, Error)]
pub enum HookError {
    /// A pre-hook rejected the operation.
    #[error("Hook '{hook_name}' rejected {operation:?}: {reason}")]
    Rejected {
        hook_name: String,
        operation: CrudOperation,
        reason: String,
    },

    /// A hook encountered an internal error.
    #[error("Hook '{hook_name}' failed: {source}")]
    Internal {
        hook_name: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
