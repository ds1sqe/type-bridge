//! Unified error types for the ORM crate.

use thiserror::Error;

/// How confidently a failed commit can be classified without observing server state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitFailureCertainty {
    /// The provider proves that the transaction did not commit.
    DefinitelyAborted,
    /// The provider cannot determine whether the transaction committed.
    Unknown,
}

/// A classified commit result for recovery-aware callers.
///
/// [`OrmError`] retains its released, exhaustively matchable variant set.
/// Ordinary commit callers receive provider failures as
/// [`OrmError::Transaction`]; callers that must distinguish a proven abort
/// can opt into [`crate::Transaction::commit_classified`].
#[derive(Debug, Error)]
pub enum ClassifiedCommitError {
    /// An ORM transaction-lifecycle error without stronger commit evidence.
    #[error(transparent)]
    Orm(#[from] OrmError),

    /// A provider commit failure with explicit durability certainty.
    #[error("Transaction error: Commit failed: {message}")]
    Driver {
        /// Whether the failed response proves that the commit was aborted.
        certainty: CommitFailureCertainty,
        /// The original provider error text.
        message: String,
    },
}

impl ClassifiedCommitError {
    /// Returns the durability certainty carried by a provider commit failure.
    #[must_use]
    pub const fn commit_failure_certainty(&self) -> Option<CommitFailureCertainty> {
        match self {
            Self::Driver { certainty, .. } => Some(*certainty),
            Self::Orm(_) => None,
        }
    }

    /// Convert to the released ORM error surface.
    #[must_use]
    pub fn into_orm_error(self) -> OrmError {
        match self {
            Self::Orm(error) => error,
            Self::Driver { message, .. } => {
                OrmError::Transaction(format!("Commit failed: {message}"))
            }
        }
    }
}

/// Unified error type for the ORM crate.
#[derive(Debug, Error)]
pub enum OrmError {
    /// Canonical typed match request or result validation failed.
    #[error(transparent)]
    Match(#[from] crate::match_request::MatchError),

    /// The detected TypeDB driver or server version lies outside the supported
    /// window, or the driver and server speak different protocol bands.
    ///
    /// The inner [`type_bridge_core_lib::version::VersionError`] message is
    /// preserved verbatim — including version numbers and remediation text —
    /// so no information is lost at the ORM boundary.
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(#[from] type_bridge_core_lib::version::VersionError),

    /// Connection to TypeDB failed.
    #[error("Connection error: {0}")]
    Connection(String),

    /// Query execution failed.
    #[error("Query execution error: {0}")]
    QueryExecution(String),

    /// Transaction already committed or rolled back.
    #[error("Transaction error: {0}")]
    Transaction(String),

    /// Failed to hydrate query results into Rust structs.
    #[error("Hydration error for type '{type_name}': {message}")]
    Hydration {
        /// The entity type name that failed hydration.
        type_name: String,
        /// A description of what went wrong.
        message: String,
    },

    /// Entity not found.
    #[error("Entity not found: {0}")]
    NotFound(String),

    /// Invalid filter specification.
    #[error("Invalid filter: {0}")]
    InvalidFilter(String),

    /// Runtime descriptor validation failed.
    #[error("Descriptor validation error for type '{type_name}': {message}")]
    DescriptorValidation {
        /// The descriptor type name, or `<registry>` for registry state errors.
        type_name: String,
        /// A description of what went wrong.
        message: String,
    },

    /// Runtime descriptor conflicts with an existing registration or expected kind.
    #[error("Descriptor conflict for type '{type_name}': {message}")]
    DescriptorConflict {
        /// The descriptor type name.
        type_name: String,
        /// A description of the conflict.
        message: String,
    },

    /// Runtime descriptor was not registered.
    #[error("Descriptor not found for type '{0}'")]
    DescriptorNotFound(String),

    /// AST compilation failed.
    #[error("Compilation error: {0}")]
    Compilation(String),

    /// Schema management error.
    #[error("Schema error: {0}")]
    Schema(#[from] crate::_schema::SchemaError),

    /// Serde JSON error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A lifecycle hook rejected or failed the operation.
    #[error("Hook error: {0}")]
    Hook(#[from] crate::hooks::HookError),
}

/// Convenience Result alias for ORM operations.
pub type Result<T> = std::result::Result<T, OrmError>;
