use type_bridge_core_lib::ast::Clause;

use super::traits::{InterceptError, Interceptor, RequestContext};

pub struct InterceptorChain {
    interceptors: Vec<Box<dyn Interceptor>>,
}

impl InterceptorChain {
    pub fn new(interceptors: Vec<Box<dyn Interceptor>>) -> Self {
        Self { interceptors }
    }

    /// Run all request interceptors in order. Each can transform or reject the query.
    pub async fn execute_request(
        &self,
        mut clauses: Vec<Clause>,
        ctx: &mut RequestContext,
    ) -> Result<Vec<Clause>, InterceptError> {
        for interceptor in &self.interceptors {
            clauses = interceptor.on_request(clauses, ctx).await?;
        }
        Ok(clauses)
    }

    /// Run all response interceptors in reverse order (middleware pattern).
    pub async fn execute_response(
        &self,
        result: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        for interceptor in self.interceptors.iter().rev() {
            interceptor.on_response(result, ctx).await?;
        }
        Ok(())
    }

    pub fn interceptor_names(&self) -> Vec<&str> {
        self.interceptors.iter().map(|i| i.name()).collect()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
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
        }
    }

    /// Passes through clauses unchanged, increments a counter.
    struct CountingInterceptor {
        name: String,
        request_count: Arc<AtomicUsize>,
        response_count: Arc<AtomicUsize>,
    }

    impl CountingInterceptor {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                request_count: Arc::new(AtomicUsize::new(0)),
                response_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl Interceptor for CountingInterceptor {
        fn name(&self) -> &str { &self.name }

        fn on_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.request_count.fetch_add(1, Ordering::SeqCst);
                Ok(clauses)
            })
        }

        fn on_response<'a>(
            &'a self,
            _result: &'a serde_json::Value,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.response_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    /// Rejects on_request with the given error.
    struct RejectingInterceptor {
        name: String,
    }

    impl Interceptor for RejectingInterceptor {
        fn name(&self) -> &str { &self.name }

        fn on_request<'a>(
            &'a self,
            _clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async { Err(InterceptError::AccessDenied { reason: "rejected".into() }) })
        }

        fn on_response<'a>(
            &'a self,
            _result: &'a serde_json::Value,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async { Err(InterceptError::Internal("response rejected".into())) })
        }
    }

    /// Adds a metadata entry on request.
    struct MetadataInterceptor;

    impl Interceptor for MetadataInterceptor {
        fn name(&self) -> &str { "metadata" }

        fn on_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                ctx.metadata.insert("marker".into(), serde_json::json!("set"));
                Ok(clauses)
            })
        }
    }

    /// Records the order it was called in a shared vec.
    struct OrderTrackingInterceptor {
        name: String,
        order: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl Interceptor for OrderTrackingInterceptor {
        fn name(&self) -> &str { &self.name }

        fn on_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.order.lock().unwrap().push(format!("req:{}", self.name));
                Ok(clauses)
            })
        }

        fn on_response<'a>(
            &'a self,
            _result: &'a serde_json::Value,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.order.lock().unwrap().push(format!("resp:{}", self.name));
                Ok(())
            })
        }
    }

    // --- execute_request tests ---

    #[tokio::test]
    async fn empty_chain_execute_request_passes_through() {
        let chain = InterceptorChain::new(vec![]);
        let mut ctx = make_ctx();
        let clauses: Vec<Clause> = vec![];
        let result = chain.execute_request(clauses, &mut ctx).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn single_interceptor_request_passes() {
        let interceptor = CountingInterceptor::new("counter");
        let req_count = interceptor.request_count.clone();
        let chain = InterceptorChain::new(vec![Box::new(interceptor)]);

        let mut ctx = make_ctx();
        chain.execute_request(vec![], &mut ctx).await.unwrap();
        assert_eq!(req_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn single_interceptor_request_rejects() {
        let chain = InterceptorChain::new(vec![
            Box::new(RejectingInterceptor { name: "rejector".into() }),
        ]);
        assert_eq!(chain.interceptor_names(), vec!["rejector"]);
        let mut ctx = make_ctx();
        let result = chain.execute_request(vec![], &mut ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn multiple_interceptors_request_chain_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let chain = InterceptorChain::new(vec![
            Box::new(OrderTrackingInterceptor { name: "first".into(), order: order.clone() }),
            Box::new(OrderTrackingInterceptor { name: "second".into(), order: order.clone() }),
            Box::new(OrderTrackingInterceptor { name: "third".into(), order: order.clone() }),
        ]);
        assert_eq!(chain.interceptor_names(), vec!["first", "second", "third"]);
        let mut ctx = make_ctx();
        chain.execute_request(vec![], &mut ctx).await.unwrap();

        let calls = order.lock().unwrap();
        assert_eq!(*calls, vec!["req:first", "req:second", "req:third"]);
    }

    #[tokio::test]
    async fn first_interceptor_rejects_second_never_runs() {
        let second = CountingInterceptor::new("second");
        let second_count = second.request_count.clone();

        let chain = InterceptorChain::new(vec![
            Box::new(RejectingInterceptor { name: "rejector".into() }),
            Box::new(second),
        ]);
        let mut ctx = make_ctx();
        let result = chain.execute_request(vec![], &mut ctx).await;
        assert!(result.is_err());
        assert_eq!(second_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn second_interceptor_rejects() {
        let first = CountingInterceptor::new("first");
        let first_count = first.request_count.clone();

        let chain = InterceptorChain::new(vec![
            Box::new(first),
            Box::new(RejectingInterceptor { name: "rejector".into() }),
        ]);
        let mut ctx = make_ctx();
        let result = chain.execute_request(vec![], &mut ctx).await;
        assert!(result.is_err());
        assert_eq!(first_count.load(Ordering::SeqCst), 1); // first did run
    }

    #[tokio::test]
    async fn request_interceptor_can_modify_context() {
        let chain = InterceptorChain::new(vec![Box::new(MetadataInterceptor)]);
        assert_eq!(chain.interceptor_names(), vec!["metadata"]);
        let mut ctx = make_ctx();
        chain.execute_request(vec![], &mut ctx).await.unwrap();
        assert_eq!(ctx.metadata.get("marker").unwrap(), &serde_json::json!("set"));
    }

    // --- execute_response tests ---

    #[tokio::test]
    async fn empty_chain_execute_response_passes_through() {
        let chain = InterceptorChain::new(vec![]);
        let ctx = make_ctx();
        let result = chain.execute_response(&serde_json::json!({}), &ctx).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn single_interceptor_response_success() {
        let interceptor = CountingInterceptor::new("counter");
        let resp_count = interceptor.response_count.clone();
        let chain = InterceptorChain::new(vec![Box::new(interceptor)]);

        let ctx = make_ctx();
        chain.execute_response(&serde_json::json!({}), &ctx).await.unwrap();
        assert_eq!(resp_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn single_interceptor_response_failure() {
        let chain = InterceptorChain::new(vec![
            Box::new(RejectingInterceptor { name: "rejector".into() }),
        ]);
        let ctx = make_ctx();
        let result = chain.execute_response(&serde_json::json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn multiple_interceptors_response_reverse_order() {
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let chain = InterceptorChain::new(vec![
            Box::new(OrderTrackingInterceptor { name: "first".into(), order: order.clone() }),
            Box::new(OrderTrackingInterceptor { name: "second".into(), order: order.clone() }),
            Box::new(OrderTrackingInterceptor { name: "third".into(), order: order.clone() }),
        ]);
        let ctx = make_ctx();
        chain.execute_response(&serde_json::json!({}), &ctx).await.unwrap();

        let calls = order.lock().unwrap();
        assert_eq!(*calls, vec!["resp:third", "resp:second", "resp:first"]);
    }

    #[tokio::test]
    async fn response_last_in_reverse_fails_others_skipped() {
        // "third" is first in reverse order — if it fails, "second" and "first" don't run
        let first = CountingInterceptor::new("first");
        let first_resp = first.response_count.clone();
        let second = CountingInterceptor::new("second");
        let second_resp = second.response_count.clone();

        let chain = InterceptorChain::new(vec![
            Box::new(first),
            Box::new(second),
            Box::new(RejectingInterceptor { name: "third".into() }),
        ]);
        let ctx = make_ctx();
        let result = chain.execute_response(&serde_json::json!({}), &ctx).await;
        assert!(result.is_err());
        // "third" is last added but first in reverse — it fails, so "second" and "first" never run
        assert_eq!(first_resp.load(Ordering::SeqCst), 0);
        assert_eq!(second_resp.load(Ordering::SeqCst), 0);
    }

    // --- interceptor_names tests ---

    #[test]
    fn interceptor_names_empty() {
        let chain = InterceptorChain::new(vec![]);
        assert!(chain.interceptor_names().is_empty());
    }

    #[test]
    fn interceptor_names_single() {
        let chain = InterceptorChain::new(vec![
            Box::new(CountingInterceptor::new("my-interceptor")),
        ]);
        assert_eq!(chain.interceptor_names(), vec!["my-interceptor"]);
    }

    #[test]
    fn interceptor_names_multiple() {
        let chain = InterceptorChain::new(vec![
            Box::new(CountingInterceptor::new("alpha")),
            Box::new(CountingInterceptor::new("beta")),
            Box::new(CountingInterceptor::new("gamma")),
        ]);
        assert_eq!(chain.interceptor_names(), vec!["alpha", "beta", "gamma"]);
    }
}
