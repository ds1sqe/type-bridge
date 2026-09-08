//! End-to-end pipeline integration tests using MockExecutor.
//!
//! Tests the full validate → intercept → compile → execute → intercept flow
//! from an external test crate perspective.

mod support;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use type_bridge_core_lib::ast::{Clause, Constraint, LiteralValue, Pattern, Value};
use type_bridge_server::error::PipelineError;
use type_bridge_server::interceptor::{InterceptError, Interceptor, RequestContext};
use type_bridge_server::pipeline::{PipelineBuilder, QueryInput, ValidateInput};
use type_bridge_server::schema_source::InMemorySchemaSource;

use support::{MockExecutor, SIMPLE_SCHEMA, make_pipeline, make_simple_clauses};

// ── Helper interceptors ──────────────────────────────────────────────

struct PassthroughInterceptor {
    name: String,
}

impl Interceptor for PassthroughInterceptor {
    fn name(&self) -> &str {
        &self.name
    }
    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move { Ok(clauses) })
    }
}

struct CountingInterceptor {
    name: String,
    request_count: Arc<AtomicUsize>,
}

impl Interceptor for CountingInterceptor {
    fn name(&self) -> &str {
        &self.name
    }
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
}

struct RejectingRequestInterceptor;

impl Interceptor for RejectingRequestInterceptor {
    fn name(&self) -> &str {
        "rejector"
    }
    fn on_request<'a>(
        &'a self,
        _clauses: Vec<Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async {
            Err(InterceptError::AccessDenied {
                reason: "test rejection".into(),
            })
        })
    }
}

struct RejectingResponseInterceptor;

impl Interceptor for RejectingResponseInterceptor {
    fn name(&self) -> &str {
        "resp-rejector"
    }
    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move { Ok(clauses) })
    }
    fn on_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async { Err(InterceptError::Internal("response rejected".into())) })
    }
}

struct MetadataInterceptor;

impl Interceptor for MetadataInterceptor {
    fn name(&self) -> &str {
        "metadata"
    }
    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            ctx.metadata.insert(
                "custom_marker".into(),
                serde_json::json!("set-by-interceptor"),
            );
            Ok(clauses)
        })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn make_query_input(clauses: Vec<Clause>) -> QueryInput {
    QueryInput {
        database: None,
        transaction_type: "read".to_string(),
        clauses,
        metadata: HashMap::new(),
    }
}

fn make_query_input_with_db(clauses: Vec<Clause>, db: &str) -> QueryInput {
    QueryInput {
        database: Some(db.to_string()),
        transaction_type: "read".to_string(),
        clauses,
        metadata: HashMap::new(),
    }
}

fn make_invalid_clauses() -> Vec<Clause> {
    vec![Clause::Match(vec![Pattern::Entity {
        variable: "p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![Constraint::Has {
            attr_name: "nonexistent_attr".to_string(),
            value: Value::Literal(LiteralValue {
                value: serde_json::json!("val"),
                value_type: "string".to_string(),
            }),
        }],
        is_strict: false,
    }])]
}

// ── Execute query tests ──────────────────────────────────────────────

#[tokio::test]
async fn execute_valid_query_returns_results() {
    let executor = MockExecutor::with_result(serde_json::json!({"data": [1, 2, 3]}));
    let pipeline = make_pipeline(executor, false);

    let output = pipeline
        .execute_query(make_query_input(vec![]))
        .await
        .unwrap();
    assert_eq!(output.results, serde_json::json!({"data": [1, 2, 3]}));
    assert!(!output.request_id.is_empty());
}

#[tokio::test]
async fn execute_with_schema_validation_passes() {
    let pipeline = make_pipeline(MockExecutor::new(), true);
    let input = make_query_input(make_simple_clauses());
    let result = pipeline.execute_query(input).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn execute_with_schema_validation_rejects_invalid() {
    let pipeline = make_pipeline(MockExecutor::new(), true);
    let input = make_query_input(make_invalid_clauses());
    let result = pipeline.execute_query(input).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(&err, PipelineError::Validation(_)));
}

#[tokio::test]
async fn execute_without_schema_skips_validation() {
    let pipeline = make_pipeline(MockExecutor::new(), false);
    // Invalid clauses should pass because there's no schema to validate against
    let input = make_query_input(make_invalid_clauses());
    let result = pipeline.execute_query(input).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn skip_validation_allows_invalid_queries() {
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_schema_source(InMemorySchemaSource::new(SIMPLE_SCHEMA))
        .with_default_database("test_db")
        .with_skip_validation()
        .build()
        .unwrap();

    // Schema is loaded but validation is skipped
    assert!(pipeline.schema().is_some());
    let input = make_query_input(make_invalid_clauses());
    let result = pipeline.execute_query(input).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn skip_validation_schema_still_accessible() {
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_schema_source(InMemorySchemaSource::new(SIMPLE_SCHEMA))
        .with_skip_validation()
        .build()
        .unwrap();

    let schema = pipeline.schema().unwrap();
    assert!(schema.entities.contains_key("person"));
}

#[tokio::test]
async fn uses_input_database_when_provided() {
    let executor = MockExecutor::new();
    let calls = executor.calls.clone();
    let pipeline = make_pipeline(executor, false);

    let input = make_query_input_with_db(vec![], "custom_db");
    pipeline.execute_query(input).await.unwrap();

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded[0].0, "custom_db");
}

#[tokio::test]
async fn uses_default_database_when_none() {
    let executor = MockExecutor::new();
    let calls = executor.calls.clone();
    let pipeline = make_pipeline(executor, false);

    let input = make_query_input(vec![]);
    pipeline.execute_query(input).await.unwrap();

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded[0].0, "test_db");
}

// ── Interceptor tests ────────────────────────────────────────────────

#[tokio::test]
async fn single_interceptor_modifies_context() {
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(MetadataInterceptor)
        .build()
        .unwrap();

    let output = pipeline
        .execute_query(make_query_input(vec![]))
        .await
        .unwrap();
    assert_eq!(output.interceptors_applied, vec!["metadata"]);
}

#[tokio::test]
async fn multiple_interceptors_execute_in_order() {
    let count1 = Arc::new(AtomicUsize::new(0));
    let count2 = Arc::new(AtomicUsize::new(0));

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(CountingInterceptor {
            name: "first".into(),
            request_count: count1.clone(),
        })
        .with_interceptor(CountingInterceptor {
            name: "second".into(),
            request_count: count2.clone(),
        })
        .build()
        .unwrap();

    let output = pipeline
        .execute_query(make_query_input(vec![]))
        .await
        .unwrap();
    assert_eq!(output.interceptors_applied, vec!["first", "second"]);
    assert_eq!(count1.load(Ordering::SeqCst), 1);
    assert_eq!(count2.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn request_interceptor_rejection_prevents_execution() {
    let executor = MockExecutor::new();
    let calls = executor.calls.clone();

    let pipeline = PipelineBuilder::new(executor)
        .with_interceptor(RejectingRequestInterceptor)
        .build()
        .unwrap();

    let result = pipeline.execute_query(make_query_input(vec![])).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), PipelineError::Interceptor(_)));

    // Executor should not have been called
    let recorded = calls.lock().unwrap();
    assert!(recorded.is_empty());
}

#[tokio::test]
async fn response_interceptor_failure_returns_error() {
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(RejectingResponseInterceptor)
        .build()
        .unwrap();

    let result = pipeline.execute_query(make_query_input(vec![])).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(&err, PipelineError::Interceptor(msg) if msg.contains("response rejected")));
}

#[tokio::test]
async fn interceptor_names_included_in_output() {
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(PassthroughInterceptor {
            name: "alpha".into(),
        })
        .with_interceptor(PassthroughInterceptor {
            name: "beta".into(),
        })
        .with_interceptor(PassthroughInterceptor {
            name: "gamma".into(),
        })
        .build()
        .unwrap();

    let output = pipeline
        .execute_query(make_query_input(vec![]))
        .await
        .unwrap();
    assert_eq!(output.interceptors_applied, vec!["alpha", "beta", "gamma"]);
}

// ── Executor error tests ─────────────────────────────────────────────

#[tokio::test]
async fn executor_error_propagates() {
    let pipeline = make_pipeline(MockExecutor::failing("database crashed"), false);
    let result = pipeline.execute_query(make_query_input(vec![])).await;
    let err = result.unwrap_err();
    assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("database crashed")));
}

// ── Validate-only tests ──────────────────────────────────────────────

#[tokio::test]
async fn validate_only_with_schema_valid() {
    let pipeline = make_pipeline(MockExecutor::new(), true);
    let input = ValidateInput {
        clauses: make_simple_clauses(),
    };
    let result = pipeline.validate(&input).unwrap();
    assert!(result.is_valid);
    assert!(result.errors.is_empty());
}

#[tokio::test]
async fn validate_only_with_schema_invalid() {
    let pipeline = make_pipeline(MockExecutor::new(), true);
    let input = ValidateInput {
        clauses: make_invalid_clauses(),
    };
    let result = pipeline.validate(&input).unwrap();
    assert!(!result.is_valid);
    assert!(!result.errors.is_empty());
}

#[tokio::test]
async fn validate_only_without_schema_errors() {
    let pipeline = make_pipeline(MockExecutor::new(), false);
    let input = ValidateInput { clauses: vec![] };
    let result = pipeline.validate(&input);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), PipelineError::Schema(_)));
}
