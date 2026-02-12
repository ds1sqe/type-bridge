use std::future::Future;
use std::pin::Pin;

use typedb_driver::TransactionType;

use crate::error::PipelineError;

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

/// Abstraction over a TypeDB transaction.
///
/// Real implementation wraps a TypeDB `Transaction`; mock implementation
/// returns configurable results for testing.
pub(crate) trait TransactionOps: Send {
    /// Execute a TypeQL query within this transaction.
    fn query(
        &mut self,
        typeql: &str,
    ) -> Pin<Box<dyn Future<Output = Result<QueryResultKind, PipelineError>> + Send + '_>>;

    /// Commit this transaction.
    fn commit(
        &mut self,
    ) -> Pin<Box<dyn Future<Output = Result<(), PipelineError>> + Send + '_>>;
}

/// Abstraction over a TypeDB driver connection.
///
/// Real implementation wraps a `TypeDBDriver`; mock implementation
/// returns configurable transactions for testing.
pub(crate) trait DriverBackend: Send + Sync {
    /// Open a new transaction against the given database.
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TransactionType,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn TransactionOps>, PipelineError>> + Send + '_>>;

    /// Check if the driver connection is open.
    fn is_open(&self) -> bool;
}
