use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

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
pub trait Interceptor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Called before the query is compiled and sent to TypeDB.
    /// Returns the (possibly transformed) clauses.
    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>;

    /// Called after query execution, before response is sent to client.
    /// Default implementation is a no-op pass-through.
    fn on_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn intercept_error_access_denied_display() {
        let e = InterceptError::AccessDenied { reason: "no permission".into() };
        assert_eq!(e.to_string(), "Access denied: no permission");
    }

    #[test]
    fn intercept_error_rate_limited_display() {
        let e = InterceptError::RateLimited { reason: "too many requests".into() };
        assert_eq!(e.to_string(), "Rate limited: too many requests");
    }

    #[test]
    fn intercept_error_validation_failed_display() {
        let e = InterceptError::ValidationFailed { reason: "bad input".into() };
        assert_eq!(e.to_string(), "Validation failed: bad input");
    }

    #[test]
    fn intercept_error_internal_display() {
        let e = InterceptError::Internal("something broke".into());
        assert_eq!(e.to_string(), "Internal error: something broke");
    }

    #[test]
    fn request_context_clone() {
        let ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.request_id, "req-1");
        assert_eq!(cloned.database, "db");
    }

    #[test]
    fn request_context_debug() {
        let ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
        };
        let debug = format!("{:?}", ctx);
        assert!(debug.contains("req-1"));
    }

    #[tokio::test]
    async fn default_on_response_returns_ok() {
        struct MinimalInterceptor;

        impl Interceptor for MinimalInterceptor {
            fn name(&self) -> &str { "minimal" }
            fn on_request<'a>(
                &'a self,
                clauses: Vec<Clause>,
                _ctx: &'a mut RequestContext,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
                Box::pin(async move { Ok(clauses) })
            }
            // on_response uses default impl
        }

        let interceptor = MinimalInterceptor;

        // Exercise name() and on_request() to cover all methods
        assert_eq!(interceptor.name(), "minimal");
        let mut ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
        };
        let req_result = interceptor.on_request(vec![], &mut ctx).await;
        assert!(req_result.is_ok());

        let result = interceptor.on_response(&serde_json::json!({}), &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn interceptor_trait_object_safety() {
        struct DummyInterceptor;

        impl Interceptor for DummyInterceptor {
            fn name(&self) -> &str { "dummy" }
            fn on_request<'a>(
                &'a self,
                clauses: Vec<Clause>,
                _ctx: &'a mut RequestContext,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
                Box::pin(async move { Ok(clauses) })
            }
        }

        let boxed: Box<dyn Interceptor> = Box::new(DummyInterceptor);
        // Exercise methods through trait object
        assert_eq!(boxed.name(), "dummy");
        let mut ctx = RequestContext {
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
        };
        let result = boxed.on_request(vec![], &mut ctx).await;
        assert!(result.unwrap().is_empty());
    }
}
