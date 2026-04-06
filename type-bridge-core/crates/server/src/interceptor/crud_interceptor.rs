use std::future::Future;
use std::pin::Pin;

use type_bridge_core_lib::ast::Clause;

use super::crud_info::{CrudInfo, extract_crud_info};
use super::traits::{InterceptError, Interceptor, RequestContext};

/// A CRUD-aware interceptor that receives semantic operation context.
///
/// Unlike [`Interceptor`], which receives raw clauses, `CrudInterceptor`
/// receives a [`CrudInfo`] describing the operation, type, and attributes
/// involved. Use [`CrudInterceptorAdapter`] to bridge into the generic
/// interceptor chain, or register via
/// [`PipelineBuilder::with_crud_interceptor`](crate::pipeline::PipelineBuilder::with_crud_interceptor).
pub trait CrudInterceptor: Send + Sync {
    /// Human-readable name for logging.
    fn name(&self) -> &str;

    /// Filter which CRUD operations this interceptor cares about.
    /// Return `false` to skip this interceptor for the given operation.
    /// Default: intercept all operations.
    fn should_intercept(&self, _info: &CrudInfo) -> bool {
        true
    }

    /// Called before the query is compiled, with CRUD context.
    fn on_crud_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        info: &'a CrudInfo,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>;

    /// Called after query execution, with CRUD context.
    /// Default: no-op pass-through.
    fn on_crud_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _info: &'a CrudInfo,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

/// Adapter that wraps a [`CrudInterceptor`] and implements the generic
/// [`Interceptor`] trait, bridging CRUD-aware interceptors into the
/// standard interceptor chain.
///
/// On the request path, the adapter:
/// 1. Extracts [`CrudInfo`] from the incoming clauses
/// 2. Stores the `CrudInfo` on `ctx.crud_info`
/// 3. Checks [`should_intercept()`](CrudInterceptor::should_intercept) — if false, passes through unchanged
/// 4. Delegates to [`on_crud_request()`](CrudInterceptor::on_crud_request)
///
/// On the response path, the adapter:
/// 1. Reads `CrudInfo` from `ctx.crud_info`
/// 2. Checks `should_intercept()` — if false, passes through
/// 3. Delegates to [`on_crud_response()`](CrudInterceptor::on_crud_response)
pub struct CrudInterceptorAdapter {
    inner: Box<dyn CrudInterceptor>,
}

impl CrudInterceptorAdapter {
    pub fn new(interceptor: impl CrudInterceptor + 'static) -> Self {
        Self {
            inner: Box::new(interceptor),
        }
    }
}

impl Interceptor for CrudInterceptorAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            let info = extract_crud_info(&clauses);
            let should = self.inner.should_intercept(&info);
            ctx.crud_info = Some(info);

            if !should {
                return Ok(clauses);
            }

            // Take info out temporarily to avoid simultaneous &CrudInfo + &mut RequestContext borrow.
            let info = ctx.crud_info.take().unwrap();
            let result = self.inner.on_crud_request(clauses, &info, ctx).await;
            ctx.crud_info = Some(info);
            result
        })
    }

    fn on_response<'a>(
        &'a self,
        result: &'a serde_json::Value,
        ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(ref info) = ctx.crud_info
                && self.inner.should_intercept(info)
            {
                return self.inner.on_crud_response(result, info, ctx).await;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use type_bridge_core_lib::ast::{Constraint, LiteralValue, Pattern, RolePlayer, Statement, Value};

    use super::*;
    use crate::interceptor::crud_info::{CrudInfo, CrudOperation, TypeKind};

    fn make_ctx() -> RequestContext {
        RequestContext {
            request_id: "test-req".into(),
            client_id: "test-client".into(),
            database: "test-db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
            crud_info: None,
        }
    }

    fn lit_str(s: &str) -> Value {
        Value::Literal(LiteralValue {
            value: serde_json::json!(s),
            value_type: "string".to_string(),
        })
    }

    fn relation_put_clauses() -> Vec<Clause> {
        vec![
            Clause::Match(vec![
                Pattern::Entity {
                    variable: "$issue".into(),
                    type_name: "JiraIssue".into(),
                    constraints: vec![Constraint::Has {
                        attr_name: "key".into(),
                        value: lit_str("DM-1"),
                    }],
                    is_strict: false,
                },
                Pattern::Entity {
                    variable: "$requirement".into(),
                    type_name: "Requirement".into(),
                    constraints: vec![Constraint::Has {
                        attr_name: "req_id".into(),
                        value: lit_str("REQ-DM-1"),
                    }],
                    is_strict: false,
                },
            ]),
            Clause::Put(vec![Statement::Relation {
                variable: "$rel".into(),
                type_name: "JiraIssueIsRequirement".into(),
                role_players: vec![
                    RolePlayer {
                        role: "issue".into(),
                        player_var: "$issue".into(),
                    },
                    RolePlayer {
                        role: "requirement".into(),
                        player_var: "$requirement".into(),
                    },
                ],
                include_variable: true,
                attributes: vec![],
            }]),
        ]
    }

    /// A CrudInterceptor that tracks calls.
    struct TrackingCrudInterceptor {
        request_count: Arc<AtomicUsize>,
        response_count: Arc<AtomicUsize>,
    }

    impl TrackingCrudInterceptor {
        fn new() -> Self {
            Self {
                request_count: Arc::new(AtomicUsize::new(0)),
                response_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl CrudInterceptor for TrackingCrudInterceptor {
        fn name(&self) -> &str {
            "tracking"
        }

        fn on_crud_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _info: &'a CrudInfo,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async move {
                self.request_count.fetch_add(1, Ordering::SeqCst);
                Ok(clauses)
            })
        }

        fn on_crud_response<'a>(
            &'a self,
            _result: &'a serde_json::Value,
            _info: &'a CrudInfo,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async move {
                self.response_count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    /// A CrudInterceptor that only intercepts Insert operations.
    struct InsertOnlyCrudInterceptor {
        request_count: Arc<AtomicUsize>,
    }

    impl CrudInterceptor for InsertOnlyCrudInterceptor {
        fn name(&self) -> &str {
            "insert-only"
        }

        fn should_intercept(&self, info: &CrudInfo) -> bool {
            info.operation == CrudOperation::Insert
        }

        fn on_crud_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _info: &'a CrudInfo,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async move {
                self.request_count.fetch_add(1, Ordering::SeqCst);
                Ok(clauses)
            })
        }
    }

    /// A CrudInterceptor that uses the default on_crud_response and should_intercept.
    struct MinimalCrudInterceptor;

    impl CrudInterceptor for MinimalCrudInterceptor {
        fn name(&self) -> &str {
            "minimal"
        }

        fn on_crud_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _info: &'a CrudInfo,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(clauses) })
        }
    }

    #[test]
    fn adapter_name_delegates() {
        let adapter = CrudInterceptorAdapter::new(MinimalCrudInterceptor);
        assert_eq!(adapter.name(), "minimal");
    }

    #[tokio::test]
    async fn adapter_stores_crud_info_on_context() {
        let adapter = CrudInterceptorAdapter::new(MinimalCrudInterceptor);
        let mut ctx = make_ctx();
        assert!(ctx.crud_info.is_none());

        let clauses = vec![Clause::Insert(vec![Statement::Isa {
            variable: "$p".into(),
            type_name: "person".into(),
        }])];
        adapter.on_request(clauses, &mut ctx).await.unwrap();

        let info = ctx.crud_info.as_ref().unwrap();
        assert_eq!(info.operation, CrudOperation::Insert);
        assert_eq!(info.type_name.as_deref(), Some("person"));
        assert_eq!(info.type_kind, Some(TypeKind::Entity));
    }

    #[tokio::test]
    async fn adapter_stores_relation_write_crud_info_on_context() {
        let adapter = CrudInterceptorAdapter::new(MinimalCrudInterceptor);
        let mut ctx = make_ctx();

        adapter.on_request(relation_put_clauses(), &mut ctx).await.unwrap();

        let info = ctx.crud_info.as_ref().unwrap();
        assert_eq!(info.operation, CrudOperation::Put);
        assert_eq!(info.type_name.as_deref(), Some("JiraIssueIsRequirement"));
        assert_eq!(info.type_kind, Some(TypeKind::Relation));
    }

    #[tokio::test]
    async fn adapter_delegates_on_crud_request() {
        let interceptor = TrackingCrudInterceptor::new();
        let req_count = interceptor.request_count.clone();
        let adapter = CrudInterceptorAdapter::new(interceptor);
        let mut ctx = make_ctx();

        adapter.on_request(vec![], &mut ctx).await.unwrap();
        assert_eq!(req_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_delegates_on_crud_response() {
        let interceptor = TrackingCrudInterceptor::new();
        let resp_count = interceptor.response_count.clone();
        let adapter = CrudInterceptorAdapter::new(interceptor);

        // Set up crud_info on context (as on_request would have done)
        let mut ctx = make_ctx();
        ctx.crud_info = Some(CrudInfo {
            operation: CrudOperation::Read,
            type_name: None,
            type_kind: None,
            attribute_names: vec![],
            iid: None,
        });

        adapter
            .on_response(&serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert_eq!(resp_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn adapter_passthrough_when_should_intercept_false() {
        let interceptor = InsertOnlyCrudInterceptor {
            request_count: Arc::new(AtomicUsize::new(0)),
        };
        let req_count = interceptor.request_count.clone();
        let adapter = CrudInterceptorAdapter::new(interceptor);
        let mut ctx = make_ctx();

        // Send a Read query — should_intercept returns false
        let clauses = vec![Clause::Match(vec![])];
        adapter.on_request(clauses, &mut ctx).await.unwrap();

        // on_crud_request was NOT called
        assert_eq!(req_count.load(Ordering::SeqCst), 0);
        // But crud_info is still stored
        assert!(ctx.crud_info.is_some());
        assert_eq!(
            ctx.crud_info.as_ref().unwrap().operation,
            CrudOperation::Read
        );
    }

    #[tokio::test]
    async fn adapter_skips_response_when_no_crud_info() {
        let interceptor = TrackingCrudInterceptor::new();
        let resp_count = interceptor.response_count.clone();
        let adapter = CrudInterceptorAdapter::new(interceptor);

        let ctx = make_ctx(); // crud_info is None
        adapter
            .on_response(&serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert_eq!(resp_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn default_should_intercept_returns_true() {
        let interceptor = MinimalCrudInterceptor;
        let info = CrudInfo {
            operation: CrudOperation::Delete,
            type_name: None,
            type_kind: None,
            attribute_names: vec![],
            iid: None,
        };
        assert!(interceptor.should_intercept(&info));
    }

    #[tokio::test]
    async fn default_on_crud_response_is_noop() {
        let interceptor = MinimalCrudInterceptor;
        let ctx = make_ctx();
        let info = CrudInfo {
            operation: CrudOperation::Read,
            type_name: None,
            type_kind: None,
            attribute_names: vec![],
            iid: None,
        };
        let result = interceptor
            .on_crud_response(&serde_json::json!({}), &info, &ctx)
            .await;
        assert!(result.is_ok());
    }
}
