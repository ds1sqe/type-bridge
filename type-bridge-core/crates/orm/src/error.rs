//! Unified error types for the ORM crate.

use thiserror::Error;

/// Unified error type for the ORM crate.
#[derive(Debug, Error)]
pub enum OrmError {
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

    /// AST compilation failed.
    #[error("Compilation error: {0}")]
    Compilation(String),

    /// Schema management error.
    #[error("Schema error: {0}")]
    Schema(#[from] crate::schema::SchemaError),

    /// Serde JSON error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Convenience Result alias for ORM operations.
pub type Result<T> = std::result::Result<T, OrmError>;
