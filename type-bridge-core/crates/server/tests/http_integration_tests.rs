//! HTTP integration tests using axum's tower::ServiceExt oneshot.
//!
//! Tests the full HTTP request/response flow through the router with
//! various pipeline configurations.

mod support;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{self, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use type_bridge_core_lib::ast::{Clause, Constraint, LiteralValue, Pattern, Value};
use type_bridge_server::interceptor::{InterceptError, Interceptor, RequestContext};
use type_bridge_server::transport::http::create_router;

use support::{MockExecutor, make_pipeline, make_simple_clauses};

// ── Helpers ──────────────────────────────────────────────────────────

fn app(executor: MockExecutor, with_schema: bool) -> Router {
    let pipeline = Arc::new(make_pipeline(executor, with_schema));
    create_router(pipeline)
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn json_request(method: &str, uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn invalid_clause_json() -> serde_json::Value {
    let clauses = vec![Clause::Match(vec![Pattern::Entity {
        variable: "p".to_string(),
        type_name: "person".to_string(),
        constraints: vec![Constraint::Has {
            attr_name: "nonexistent".to_string(),
            value: Value::Literal(LiteralValue {
                value: serde_json::json!("val"),
                value_type: "string".to_string(),
            }),
        }],
        is_strict: false,
    }])];
    serde_json::to_value(&clauses).unwrap()
}

// ── Test interceptors ────────────────────────────────────────────────

struct CountingInterceptor {
    name: String,
    count: Arc<AtomicUsize>,
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
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(clauses)
        })
    }
}

struct RejectingInterceptor;

impl Interceptor for RejectingInterceptor {
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
                reason: "forbidden".into(),
            })
        })
    }
}

// ── Health endpoint tests ────────────────────────────────────────────

#[tokio::test]
async fn health_200() {
    let router = app(MockExecutor::new(), false);
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["status"], "ok");
    assert!(!json["version"].as_str().unwrap().is_empty());
    assert_eq!(json["typedb_connected"], true);
}

#[tokio::test]
async fn health_reflects_connection_status() {
    let executor = MockExecutor::new();
    *executor.connected.lock().unwrap() = false;
    let router = app(executor, false);
    let req = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let json = body_json(resp).await;
    assert_eq!(json["typedb_connected"], false);
}

// ── Schema endpoint tests ────────────────────────────────────────────

#[tokio::test]
async fn schema_200_with_loaded_schema() {
    let router = app(MockExecutor::new(), true);
    let req = Request::builder()
        .uri("/schema")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert!(json["entities"].is_object());
}

#[tokio::test]
async fn schema_500_without_schema() {
    let router = app(MockExecutor::new(), false);
    let req = Request::builder()
        .uri("/schema")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "SCHEMA_ERROR");
}

// ── Query success tests ──────────────────────────────────────────────

#[tokio::test]
async fn query_success_200() {
    let router = app(MockExecutor::new(), false);
    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn query_with_database_override() {
    let executor = MockExecutor::new();
    let calls = executor.calls.clone();
    let pipeline = Arc::new(make_pipeline(executor, false));
    let router = create_router(pipeline);

    let body = serde_json::json!({
        "database": "override_db",
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    router.oneshot(req).await.unwrap();

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded[0].0, "override_db");
}

#[tokio::test]
async fn query_response_metadata_fields() {
    let router = app(MockExecutor::new(), false);
    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    let json = body_json(resp).await;

    assert!(json["metadata"]["request_id"].is_string());
    assert!(json["metadata"]["execution_time_ms"].is_number());
    assert!(json["metadata"]["interceptors_applied"].is_array());
}

// ── Query error tests ────────────────────────────────────────────────

#[tokio::test]
async fn query_validation_failure_400() {
    let router = app(MockExecutor::new(), true);
    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": invalid_clause_json()
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "VALIDATION_FAILED");
}

#[tokio::test]
async fn query_executor_failure_400() {
    let router = app(MockExecutor::failing("db error"), false);
    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "QUERY_EXECUTION_ERROR");
}

#[tokio::test]
async fn query_bad_json_400() {
    let router = app(MockExecutor::new(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/query")
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from("not json"))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn query_interceptor_rejection_403() {
    let pipeline = Arc::new(
        type_bridge_server::pipeline::PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(RejectingInterceptor)
            .build()
            .unwrap(),
    );
    let router = create_router(pipeline);

    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "INTERCEPTOR_ERROR");
}

// ── Validate endpoint tests ──────────────────────────────────────────

#[tokio::test]
async fn validate_valid_200() {
    let router = app(MockExecutor::new(), true);
    let clauses = serde_json::to_value(make_simple_clauses()).unwrap();
    let body = serde_json::json!({ "clauses": clauses });
    let req = json_request("POST", "/query/validate", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["is_valid"], true);
    assert!(json["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn validate_invalid_with_errors() {
    let router = app(MockExecutor::new(), true);
    let body = serde_json::json!({ "clauses": invalid_clause_json() });
    let req = json_request("POST", "/query/validate", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(json["is_valid"], false);
    assert!(!json["errors"].as_array().unwrap().is_empty());

    // Each error has code, message, path
    let error = &json["errors"][0];
    assert!(error["code"].is_string());
    assert!(error["message"].is_string());
    assert!(error["path"].is_string());
}

#[tokio::test]
async fn validate_no_schema_500() {
    let router = app(MockExecutor::new(), false);
    let body = serde_json::json!({ "clauses": [] });
    let req = json_request("POST", "/query/validate", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let json = body_json(resp).await;
    assert_eq!(json["error"]["code"], "SCHEMA_ERROR");
}

// ── Interceptor via HTTP tests ───────────────────────────────────────

#[tokio::test]
async fn query_with_counting_interceptor() {
    let count = Arc::new(AtomicUsize::new(0));
    let pipeline = Arc::new(
        type_bridge_server::pipeline::PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(CountingInterceptor {
                name: "counter".into(),
                count: count.clone(),
            })
            .build()
            .unwrap(),
    );
    let router = create_router(pipeline);

    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let json = body_json(resp).await;
    assert_eq!(
        json["metadata"]["interceptors_applied"],
        serde_json::json!(["counter"])
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn query_with_audit_interceptor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");

    let config = type_bridge_server::config::AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().to_string(),
    };
    let audit =
        type_bridge_server::interceptor::audit_log::AuditLogInterceptor::new(&config).unwrap();

    let pipeline = Arc::new(
        type_bridge_server::pipeline::PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(audit)
            .build()
            .unwrap(),
    );
    let router = create_router(pipeline);

    let body = serde_json::json!({
        "transaction_type": "read",
        "clauses": []
    });
    let req = json_request("POST", "/query", body);
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify audit entry was written
    let content = std::fs::read_to_string(&path).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(entry["status"], "ok");
    assert!(entry["request_id"].is_string());
}

// ── Routing tests ────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_404() {
    let router = app(MockExecutor::new(), false);
    let req = Request::builder()
        .uri("/nonexistent")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_on_query_405() {
    let router = app(MockExecutor::new(), false);
    let req = Request::builder()
        .method("GET")
        .uri("/query")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
}
