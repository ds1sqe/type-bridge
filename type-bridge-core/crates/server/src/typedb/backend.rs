use std::future::Future;
use std::pin::Pin;

use super::ExecuteError;
use crate::error::PipelineError;

/// Boxed future returned by async trait methods.
pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Transaction type for TypeDB operations.
///
/// This is a crate-owned mirror of the driver's transaction-type concept.
/// The mapping to the underlying driver type lives exclusively in
/// `real_driver.rs` at the band-8 call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransactionType {
    /// Read-only transaction (no commit needed).
    Read,
    /// Read-write transaction.
    Write,
    /// Schema-modification transaction.
    Schema,
}

/// Result of a query, already processed from TypeDB stream types to JSON.
#[derive(Debug, Clone)]
pub(crate) enum QueryResultKind {
    /// Write/schema confirmation (no data returned).
    Ok,
    /// Rows of concept data, each converted to JSON.
    Rows(Vec<serde_json::Value>),
    /// Documents, each converted to JSON.
    Documents(Vec<serde_json::Value>),
}

/// Abstraction over a TypeDB driver connection.
///
/// Real implementation wraps a `TypeDBDriver`; mock implementation
/// returns configurable transactions for testing.
pub(crate) trait DriverBackend: Send + Sync {
    /// Execute one query as an atomic core transaction with explicit options.
    fn execute(
        &self,
        database: &str,
        typeql: &str,
        options: type_bridge_orm::session::staged::TransactionOptions,
    ) -> BoxFuture<'_, Result<QueryResultKind, ExecuteError>>;

    /// Check whether a database exists.
    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, PipelineError>>;

    /// Create a database.
    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), PipelineError>>;

    /// Delete a database.
    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), PipelineError>>;

    /// Check if the driver connection is open.
    fn is_open(&self) -> bool;
}
