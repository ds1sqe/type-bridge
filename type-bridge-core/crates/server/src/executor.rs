use std::future::Future;
use std::pin::Pin;

use crate::error::PipelineError;

/// Backend-agnostic query execution trait.
///
/// Implement this trait to provide a custom database backend for the query pipeline.
/// The built-in `TypeDBClient` (behind the `typedb` feature) implements this trait.
///
/// # Example
///
/// ```rust,ignore
/// use type_bridge_server::{QueryExecutor, PipelineError};
/// use std::pin::Pin;
/// use std::future::Future;
///
/// struct MockExecutor;
///
/// impl QueryExecutor for MockExecutor {
///     fn execute<'a>(&'a self, _database: &'a str, typeql: &'a str, _transaction_type: &'a str)
///         -> Pin<Box<dyn Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>>
///     {
///         Box::pin(async move {
///             Ok(serde_json::json!([{"query": typeql}]))
///         })
///     }
///     fn is_connected(&self) -> bool { true }
/// }
/// ```
pub trait QueryExecutor: Send + Sync {
    /// Execute a TypeQL string against the given database.
    fn execute<'a>(
        &'a self,
        database: &'a str,
        typeql: &'a str,
        transaction_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>>;

    /// Check if the backend connection is alive.
    fn is_connected(&self) -> bool;
}
