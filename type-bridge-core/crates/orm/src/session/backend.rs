//! Backend abstraction traits for TypeDB connectivity.
//!
//! These traits enable testing with mock backends while using the real
//! TypeDB driver in production (behind the `typedb` feature flag).

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::error::OrmError;

/// Result of a query execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryResult {
    /// No data returned (write/schema confirmation).
    Ok,
    /// Document results from a fetch clause.
    Documents(Vec<serde_json::Value>),
    /// Row results from a match + reduce/concept query.
    Rows(Vec<serde_json::Value>),
}

/// Boxed future type alias for async trait methods.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Transaction type for TypeDB operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxType {
    /// Read-only transaction (no commit needed).
    Read,
    /// Read-write transaction (auto-committed after query).
    Write,
    /// Schema transaction for defining types.
    Schema,
}

/// Abstraction over a TypeDB transaction.
///
/// Implementations handle query execution and transaction lifecycle.
/// The `RealBackend` (behind the `typedb` feature) provides the real
/// TypeDB implementation; tests use mock implementations.
pub trait TransactionOps: Send {
    /// Execute a TypeQL query string.
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>>;

    /// Commit this transaction. Only meaningful for write/schema transactions.
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;
}

/// Abstraction over a TypeDB driver connection.
///
/// Opens transactions against a named database. Implementations must be
/// thread-safe (`Send + Sync`) for use across async tasks.
pub trait DriverBackend: Send + Sync {
    /// Open a new transaction against the given database.
    fn open_transaction(
        &self,
        database: &str,
        tx_type: TxType,
    ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>>;

    /// Check if the underlying connection is still alive.
    fn is_open(&self) -> bool;
}
