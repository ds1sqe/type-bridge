use std::future::Future;
use std::pin::Pin;

use type_bridge_core_lib::ast::Clause;

use super::crud_info::CrudInfo;
use super::traits::{InterceptError, Interceptor, RequestContext};

/// Convenience trait for interceptors that only care about CRUD operations.
///
/// Non-CRUD requests (raw queries, validation) are automatically passed through
/// without invoking any callbacks.
pub trait CrudInterceptor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Called before a CRUD query is compiled and sent to TypeDB.
    fn on_crud_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        crud_info: CrudInfo,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>;

    /// Called after CRUD query execution. Default is a no-op.
    fn on_crud_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _crud_info: CrudInfo,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }

    /// Whether this interceptor should run for the given CRUD operation.
    /// Default: run for all CRUD operations.
    fn should_intercept(&self, _crud_info: &CrudInfo) -> bool {
        true
    }
}

/// Wraps a [`CrudInterceptor`] as a generic [`Interceptor`].
///
/// Non-CRUD requests pass through unchanged. CRUD requests are
/// delegated if [`CrudInterceptor::should_intercept`] returns true.
pub struct CrudInterceptorAdapter<T: CrudInterceptor>(pub T);

impl<T: CrudInterceptor + 'static> Interceptor for CrudInterceptorAdapter<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            let crud_info = ctx.crud_info.clone();
            if crud_info.is_crud() && self.0.should_intercept(&crud_info) {
                self.0.on_crud_request(clauses, crud_info, ctx).await
            } else {
                Ok(clauses)
            }
        })
    }

    fn on_response<'a>(
        &'a self,
        result: &'a serde_json::Value,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            if ctx.crud_info.is_crud() && self.0.should_intercept(&ctx.crud_info) {
                self.0.on_crud_response(result, ctx.crud_info.clone(), ctx).await
            } else {
                Ok(())
            }
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    fn make_ctx() -> RequestContext {
        RequestContext {
            request_id: "test-req".into(),
            client_id: "test-client".into(),
            database: "test-db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
            crud_info: CrudInfo::default(),
        }
    }

    fn make_crud_ctx() -> RequestContext {
        RequestContext {
            request_id: "test-req".into(),
            client_id: "test-client".into(),
            database: "test-db".into(),
            transaction_type: "write".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
            crud_info: CrudInfo {
                operation: Some("insert".to_string()),
                type_name: Some("person".to_string()),
                type_kind: Some("entity".to_string()),
                attribute_names: vec!["name".to_string()],
                iid: None,
            },
        }
    }

    /// A counting CRUD interceptor for testing.
    struct CountingCrudInterceptor {
        request_count: Arc<AtomicUsize>,
        response_count: Arc<AtomicUsize>,
    }

    impl CountingCrudInterceptor {
        fn new() -> Self {
            Self {
                request_count: Arc::new(AtomicUsize::new(0)),
                response_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl CrudInterceptor for CountingCrudInterceptor {
        fn name(&self) -> &str {
            "counting-crud"
        }

        fn on_crud_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _crud_info: CrudInfo,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.request_count.fetch_add(1, Ordering::SeqCst);
                Ok(clauses)
            })
        }

        fn on_crud_response<'a>(
            &'a self,
            _result: &'a serde_json::Value,
            _crud_info: CrudInfo,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.response_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    /// A CRUD interceptor that rejects requests.
    struct RejectingCrudInterceptor;

    impl CrudInterceptor for RejectingCrudInterceptor {
        fn name(&self) -> &str {
            "rejecting-crud"
        }

        fn on_crud_request<'a>(
            &'a self,
            _clauses: Vec<Clause>,
            _crud_info: CrudInfo,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async { Err(InterceptError::AccessDenied { reason: "crud rejected".into() }) })
        }
    }

    /// A CRUD interceptor that only intercepts "delete" operations.
    struct DeleteOnlyInterceptor {
        count: Arc<AtomicUsize>,
    }

    impl CrudInterceptor for DeleteOnlyInterceptor {
        fn name(&self) -> &str {
            "delete-only"
        }

        fn on_crud_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _crud_info: CrudInfo,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(clauses)
            })
        }

        fn should_intercept(&self, crud_info: &CrudInfo) -> bool {
            crud_info.operation.as_deref() == Some("delete")
        }
    }

    // --- Adapter tests ---

    #[tokio::test]
    async fn adapter_passes_through_non_crud_requests() {
        let inner = CountingCrudInterceptor::new();
        let req_count = inner.request_count.clone();
        let adapter = CrudInterceptorAdapter(inner);

        let mut ctx = make_ctx(); // no CRUD info
        let result = adapter.on_request(vec![], &mut ctx).await.unwrap();
        assert!(result.is_empty());
        assert_eq!(req_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn adapter_delegates_crud_requests() {
        let inner = CountingCrudInterceptor::new();
        let req_count = inner.request_count.clone();
        let adapter = CrudInterceptorAdapter(inner);

        let mut ctx = make_crud_ctx();
        let result = adapter.on_request(vec![], &mut ctx).await.unwrap();
        assert!(result.is_empty());
        assert_eq!(req_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_delegates_crud_responses() {
        let inner = CountingCrudInterceptor::new();
        let resp_count = inner.response_count.clone();
        let adapter = CrudInterceptorAdapter(inner);

        let ctx = make_crud_ctx();
        adapter.on_response(&serde_json::json!({}), &ctx).await.unwrap();
        assert_eq!(resp_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_passes_through_non_crud_responses() {
        let inner = CountingCrudInterceptor::new();
        let resp_count = inner.response_count.clone();
        let adapter = CrudInterceptorAdapter(inner);

        let ctx = make_ctx();
        adapter.on_response(&serde_json::json!({}), &ctx).await.unwrap();
        assert_eq!(resp_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn adapter_should_intercept_false_skips_crud() {
        let count = Arc::new(AtomicUsize::new(0));
        let adapter = CrudInterceptorAdapter(DeleteOnlyInterceptor { count: count.clone() });

        // "insert" operation — should_intercept returns false
        let mut ctx = make_crud_ctx();
        adapter.on_request(vec![], &mut ctx).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn adapter_should_intercept_true_runs_crud() {
        let count = Arc::new(AtomicUsize::new(0));
        let adapter = CrudInterceptorAdapter(DeleteOnlyInterceptor { count: count.clone() });

        let mut ctx = make_crud_ctx();
        ctx.crud_info.operation = Some("delete".to_string());
        adapter.on_request(vec![], &mut ctx).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_name_delegates() {
        let adapter = CrudInterceptorAdapter(CountingCrudInterceptor::new());
        assert_eq!(adapter.name(), "counting-crud");
    }

    #[tokio::test]
    async fn adapter_rejects_when_on_crud_request_returns_error() {
        let adapter = CrudInterceptorAdapter(RejectingCrudInterceptor);
        assert_eq!(adapter.name(), "rejecting-crud");

        let mut ctx = make_crud_ctx();
        let result = adapter.on_request(vec![], &mut ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn default_on_crud_response_is_noop() {
        // Use a minimal CrudInterceptor that doesn't override on_crud_response
        struct MinimalCrud;
        impl CrudInterceptor for MinimalCrud {
            fn name(&self) -> &str { "minimal" }
            fn on_crud_request<'a>(
                &'a self,
                clauses: Vec<Clause>,
                _crud_info: CrudInfo,
                _ctx: &'a mut RequestContext,
            ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
                Box::pin(async move { Ok(clauses) })
            }
        }

        let adapter = CrudInterceptorAdapter(MinimalCrud);
        let ctx = make_crud_ctx();
        let result = adapter.on_response(&serde_json::json!({}), &ctx).await;
        assert!(result.is_ok());
    }
}
