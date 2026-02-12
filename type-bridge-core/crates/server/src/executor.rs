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
///
/// struct MockExecutor;
///
/// #[async_trait::async_trait]
/// impl QueryExecutor for MockExecutor {
///     async fn execute(
///         &self, _database: &str, typeql: &str, _transaction_type: &str,
///     ) -> Result<serde_json::Value, PipelineError> {
///         Ok(serde_json::json!([{"query": typeql}]))
///     }
///     fn is_connected(&self) -> bool { true }
/// }
/// ```
#[async_trait::async_trait]
pub trait QueryExecutor: Send + Sync {
    /// Execute a TypeQL string against the given database.
    async fn execute(
        &self,
        database: &str,
        typeql: &str,
        transaction_type: &str,
    ) -> Result<serde_json::Value, PipelineError>;

    /// Check if the backend connection is alive.
    fn is_connected(&self) -> bool;
}
