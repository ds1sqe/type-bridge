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

/// A single value bound to a `given` variable.
///
/// Temporal variants carry ISO-8601 text; the real backend parses them when
/// lowering onto the driver, mirroring how [`QueryResult`] renders temporal
/// values back to strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GivenValue {
    /// TypeQL `boolean`.
    Boolean(bool),
    /// TypeQL `integer`.
    Integer(i64),
    /// TypeQL `double`.
    Double(f64),
    /// TypeQL `string`.
    String(String),
    /// TypeQL `date`, ISO-8601 (`YYYY-MM-DD`).
    Date(String),
    /// TypeQL `datetime`, ISO-8601 without offset.
    Datetime(String),
    /// TypeQL `datetime-tz`, RFC 3339 with offset.
    DatetimeTz(String),
}

/// Input rows for a `given`-stage query: a variable header plus value rows.
///
/// Every row must have exactly one entry per header variable, in header
/// order. Rows travel through the driver API, never the query string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GivenRowsSpec {
    /// Variable names, without the `$` sigil, in column order.
    pub variables: Vec<String>,
    /// Value rows; each inner vec is one input row in header order.
    pub rows: Vec<Vec<GivenValue>>,
}

/// Abstraction over a TypeDB transaction.
///
/// Implementations handle query execution and transaction lifecycle.
/// The `RealBackend` (behind the `typedb` feature) provides the real
/// TypeDB implementation; tests use mock implementations.
pub trait TransactionOps: Send {
    /// Execute a TypeQL query string.
    fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>>;

    /// Execute a TypeQL query with `given`-stage input rows.
    ///
    /// Rows travel through the driver API, not the query string. Defaults to
    /// an error for backends without given-stage support (mocks, pre-band-9
    /// drivers); the real backend overrides this on band-9 connections.
    fn query_with_rows(
        &mut self,
        typeql: &str,
        rows: GivenRowsSpec,
    ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
        let _ = (typeql, rows);
        Box::pin(async move {
            Err(OrmError::QueryExecution(
                "given-stage parameterized queries are not supported by this backend".into(),
            ))
        })
    }

    /// Commit this transaction. Only meaningful for write/schema transactions.
    fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;

    /// Roll back this transaction.
    fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;

    /// Close this transaction without committing.
    fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>>;
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

    /// The server version detected at connect time, when the backend knows it.
    ///
    /// Defaults to `None` for backends without a version gate (mocks, embedded
    /// test backends). The real TypeDB backend reports the version the connect
    /// gate detected; it is `None` only on the band-7 gRPC fallback.
    fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
        None
    }

    /// Check whether a database exists, when the backend supports database
    /// lifecycle operations.
    fn database_exists(&self, database: &str) -> BoxFuture<'_, Result<bool, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Database existence checks are not supported by this backend for database '{database}'"
            )))
        })
    }

    /// Create a database, when the backend supports database lifecycle
    /// operations.
    fn create_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Database creation is not supported by this backend for database '{database}'"
            )))
        })
    }

    /// Delete a database, when the backend supports database lifecycle
    /// operations.
    fn delete_database(&self, database: &str) -> BoxFuture<'_, Result<(), OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Database deletion is not supported by this backend for database '{database}'"
            )))
        })
    }

    /// Export the database schema as TypeQL text, when the backend supports it.
    fn schema_text(&self, database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
        let database = database.to_string();
        Box::pin(async move {
            Err(OrmError::Connection(format!(
                "Schema export is not supported by this backend for database '{database}'"
            )))
        })
    }
}
