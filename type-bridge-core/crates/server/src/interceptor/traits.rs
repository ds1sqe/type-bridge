use std::collections::HashMap;
use type_bridge_core_lib::ast::Clause;

/// Metadata attached to each request flowing through the interceptor chain.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: String,
    pub client_id: String,
    pub database: String,
    pub transaction_type: String,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Errors that interceptors can produce.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // variants for future interceptors
pub enum InterceptError {
    #[error("Access denied: {reason}")]
    AccessDenied { reason: String },
    #[error("Rate limited: {reason}")]
    RateLimited { reason: String },
    #[error("Validation failed: {reason}")]
    ValidationFailed { reason: String },
    #[error("Internal error: {0}")]
    Internal(String),
}

/// The core interceptor trait.
///
/// Interceptors receive the query AST and context, and can:
/// - Pass through unchanged
/// - Transform the query (e.g., add tenant filters)
/// - Reject the query (return Err)
/// - Add metadata to context for downstream interceptors
#[async_trait::async_trait]
pub trait Interceptor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Called before the query is compiled and sent to TypeDB.
    /// Returns the (possibly transformed) clauses.
    async fn on_request(
        &self,
        clauses: Vec<Clause>,
        ctx: &mut RequestContext,
    ) -> Result<Vec<Clause>, InterceptError>;

    /// Called after query execution, before response is sent to client.
    /// Default implementation is a no-op pass-through.
    async fn on_response(
        &self,
        _result: &serde_json::Value,
        _ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        Ok(())
    }
}
