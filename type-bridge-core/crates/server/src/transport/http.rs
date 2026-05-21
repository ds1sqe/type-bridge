use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use type_bridge_core_lib::query_parser::parse_typeql_query;

use crate::error::PipelineError;
use crate::pipeline::{QueryInput, QueryPipeline, ValidateInput};
use crate::transport::types::*;

// --- Axum-specific error response types ---

#[derive(Debug, Serialize)]
struct ErrorResponse {
    pub status: String,
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl IntoResponse for PipelineError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            PipelineError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "CONFIG_ERROR"),
            PipelineError::Connection(_) => (StatusCode::SERVICE_UNAVAILABLE, "CONNECTION_ERROR"),
            PipelineError::QueryExecution(_) => (StatusCode::BAD_REQUEST, "QUERY_EXECUTION_ERROR"),
            PipelineError::Validation(_) => (StatusCode::BAD_REQUEST, "VALIDATION_FAILED"),
            PipelineError::Parse(_) => (StatusCode::BAD_REQUEST, "PARSE_ERROR"),
            PipelineError::Schema(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SCHEMA_ERROR"),
            PipelineError::Interceptor(_) => (StatusCode::FORBIDDEN, "INTERCEPTOR_ERROR"),
            PipelineError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
        };

        let body = ErrorResponse {
            status: "error".to_string(),
            error: ErrorDetail {
                code: code.to_string(),
                message: self.to_string(),
                details: None,
            },
        };

        (status, axum::Json(body)).into_response()
    }
}

// --- Router ---

pub fn create_router(pipeline: Arc<QueryPipeline>) -> Router {
    Router::new()
        .route("/query", post(handle_query))
        .route("/query/raw", post(handle_raw_query))
        .route("/query/validate", post(handle_validate))
        .route("/health", get(handle_health))
        .route("/schema", get(handle_schema))
        .with_state(pipeline)
}

// --- Handlers ---

async fn handle_query(
    State(pipeline): State<Arc<QueryPipeline>>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, PipelineError> {
    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: req.transaction_type,
            clauses: req.clauses,
            metadata: req.metadata,
        })
        .await?;

    Ok(Json(QueryResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: ResponseMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            interceptors_applied: output.interceptors_applied,
        },
    }))
}

async fn handle_raw_query(
    State(pipeline): State<Arc<QueryPipeline>>,
    Json(req): Json<RawQueryRequest>,
) -> Result<Json<QueryResponse>, PipelineError> {
    let clauses = parse_typeql_query(&req.query).map_err(|e| PipelineError::Parse(e.to_string()))?;

    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: req.transaction_type,
            clauses,
            metadata: req.metadata,
        })
        .await?;

    Ok(Json(QueryResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: ResponseMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            interceptors_applied: output.interceptors_applied,
        },
    }))
}

async fn handle_validate(
    State(pipeline): State<Arc<QueryPipeline>>,
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, PipelineError> {
    let output = pipeline.validate(&ValidateInput {
        clauses: req.clauses,
    })?;

    let errors: Vec<serde_json::Value> = output
        .errors
        .iter()
        .map(|e| {
            serde_json::json!({
                "code": e.code,
                "message": e.message,
                "path": e.path,
            })
        })
        .collect();

    Ok(Json(ValidateResponse {
        status: "ok".to_string(),
        is_valid: output.is_valid,
        errors,
    }))
}

async fn handle_health(State(pipeline): State<Arc<QueryPipeline>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        typedb_connected: pipeline.is_connected(),
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn to_schema_error(e: impl std::fmt::Display) -> PipelineError {
    PipelineError::Schema(e.to_string())
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn to_internal_error(e: impl std::fmt::Display) -> PipelineError {
    PipelineError::Internal(e.to_string())
}

async fn handle_schema(
    State(pipeline): State<Arc<QueryPipeline>>,
) -> Result<Json<serde_json::Value>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    schema_to_json(schema)
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn schema_to_json(
    schema: &type_bridge_core_lib::schema::TypeSchema,
) -> Result<Json<serde_json::Value>, PipelineError> {
    let json = schema.to_json().map_err(to_schema_error)?;

    let value: serde_json::Value = serde_json::from_str(&json).map_err(to_internal_error)?;

    Ok(Json(value))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use axum::body::Body;
    use axum::http::{self, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use type_bridge_core_lib::ast::{Clause, Constraint, LiteralValue, Pattern, Value};

    use super::*;
    use crate::test_helpers::{MockExecutor, make_pipeline, make_simple_clauses};

    /// Build an invalid clause (references nonexistent attr) as a serde_json::Value.
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

    fn app(executor: MockExecutor, with_schema: bool) -> Router {
        let pipeline = Arc::new(make_pipeline(executor, with_schema));
        create_router(pipeline)
    }

    async fn body_json(response: Response) -> serde_json::Value {
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

    // =============================================
    // PipelineError IntoResponse tests
    // =============================================

    #[test]
    fn into_response_config_error() {
        let resp = PipelineError::Config("bad".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn into_response_connection_error() {
        let resp = PipelineError::Connection("down".into()).into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn into_response_query_execution_error() {
        let resp = PipelineError::QueryExecution("fail".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn into_response_validation_error() {
        let resp = PipelineError::Validation("invalid".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn into_response_parse_error() {
        let resp = PipelineError::Parse("syntax".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn into_response_schema_error() {
        let resp = PipelineError::Schema("missing".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn into_response_interceptor_error() {
        let resp = PipelineError::Interceptor("denied".into()).into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn into_response_internal_error() {
        let resp = PipelineError::Internal("oops".into()).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn into_response_body_structure() {
        let resp = PipelineError::Config("bad config".into()).into_response();
        let json = body_json(resp).await;
        assert_eq!(json["status"], "error");
        assert_eq!(json["error"]["code"], "CONFIG_ERROR");
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("bad config")
        );
    }

    // =============================================
    // handle_health tests
    // =============================================

    #[tokio::test]
    async fn health_returns_200_with_status() {
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

    #[tokio::test]
    async fn health_connected_true() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let json = body_json(resp).await;
        assert_eq!(json["typedb_connected"], true);
    }

    // =============================================
    // handle_schema tests
    // =============================================

    #[tokio::test]
    async fn schema_no_schema_returns_500() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .uri("/schema")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn schema_with_schema_returns_200() {
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
    async fn schema_post_not_allowed() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .method("POST")
            .uri("/schema")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    // =============================================
    // handle_query tests
    // =============================================

    #[tokio::test]
    async fn query_success() {
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
        assert!(!json["metadata"]["request_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn raw_query_success() {
        let router = app(MockExecutor::new(), false);
        let body = serde_json::json!({
            "transaction_type": "read",
            "query": "match $p isa person; fetch { \"person\": { $p.* } };"
        });
        let req = json_request("POST", "/query/raw", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["status"], "ok");
        assert!(!json["metadata"]["request_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn raw_query_parse_error() {
        let router = app(MockExecutor::new(), false);
        let body = serde_json::json!({
            "transaction_type": "read",
            "query": "this is not valid typeql!!!"
        });
        let req = json_request("POST", "/query/raw", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let json = body_json(resp).await;
        assert_eq!(json["error"]["code"], "PARSE_ERROR");
    }

    #[tokio::test]
    async fn query_executor_failure() {
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
    async fn query_bad_json() {
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
    async fn query_database_override() {
        let executor = MockExecutor::new();
        let calls = executor.calls.clone();
        let pipeline = Arc::new(make_pipeline(executor, false));
        let router = create_router(pipeline);

        let body = serde_json::json!({
            "database": "custom_db",
            "transaction_type": "read",
            "clauses": []
        });
        let req = json_request("POST", "/query", body);
        router.oneshot(req).await.unwrap();

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded[0].0, "custom_db");
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

    #[tokio::test]
    async fn query_validation_failure() {
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

    // =============================================
    // handle_validate tests
    // =============================================

    #[tokio::test]
    async fn validate_valid_query() {
        let router = app(MockExecutor::new(), true);
        let clauses = serde_json::to_value(make_simple_clauses()).unwrap();
        let body = serde_json::json!({
            "clauses": clauses
        });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["is_valid"], true);
        assert!(json["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validate_invalid_query() {
        let router = app(MockExecutor::new(), true);
        let body = serde_json::json!({ "clauses": invalid_clause_json() });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["is_valid"], false);
        assert!(!json["errors"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validate_no_schema() {
        let router = app(MockExecutor::new(), false);
        let body = serde_json::json!({ "clauses": [] });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let json = body_json(resp).await;
        assert_eq!(json["error"]["code"], "SCHEMA_ERROR");
    }

    #[tokio::test]
    async fn validate_error_structure() {
        let router = app(MockExecutor::new(), true);
        let body = serde_json::json!({ "clauses": invalid_clause_json() });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        let json = body_json(resp).await;

        let error = &json["errors"][0];
        assert!(error["code"].is_string());
        assert!(error["message"].is_string());
        assert!(error["path"].is_string());
    }

    #[tokio::test]
    async fn validate_bad_json() {
        let router = app(MockExecutor::new(), true);
        let req = Request::builder()
            .method("POST")
            .uri("/query/validate")
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn validate_empty_clauses() {
        let router = app(MockExecutor::new(), true);
        let body = serde_json::json!({ "clauses": [] });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp).await;
        assert_eq!(json["is_valid"], true);
    }

    // =============================================
    // Router integration tests
    // =============================================

    #[tokio::test]
    async fn router_health_route_exists() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn router_schema_route_exists() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .uri("/schema")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn router_query_route_exists() {
        let router = app(MockExecutor::new(), false);
        let body = serde_json::json!({
            "transaction_type": "read",
            "clauses": []
        });
        let req = json_request("POST", "/query", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn router_validate_route_exists() {
        let router = app(MockExecutor::new(), true);
        let body = serde_json::json!({ "clauses": [] });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn router_unknown_route_returns_404() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .uri("/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // =============================================
    // Content-type tests
    // =============================================

    #[tokio::test]
    async fn health_response_is_json() {
        let router = app(MockExecutor::new(), false);
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let content_type = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
        assert!(content_type.to_str().unwrap().contains("application/json"));
    }

    #[tokio::test]
    async fn error_response_is_json() {
        let router = app(MockExecutor::failing("fail"), false);
        let body = serde_json::json!({
            "transaction_type": "read",
            "clauses": []
        });
        let req = json_request("POST", "/query", body);
        let resp = router.oneshot(req).await.unwrap();
        let content_type = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
        assert!(content_type.to_str().unwrap().contains("application/json"));
    }

    #[tokio::test]
    async fn query_response_is_json() {
        let router = app(MockExecutor::new(), false);
        let body = serde_json::json!({
            "transaction_type": "read",
            "clauses": []
        });
        let req = json_request("POST", "/query", body);
        let resp = router.oneshot(req).await.unwrap();
        let content_type = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
        assert!(content_type.to_str().unwrap().contains("application/json"));
    }

    #[tokio::test]
    async fn validate_response_is_json() {
        let router = app(MockExecutor::new(), true);
        let body = serde_json::json!({ "clauses": [] });
        let req = json_request("POST", "/query/validate", body);
        let resp = router.oneshot(req).await.unwrap();
        let content_type = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
        assert!(content_type.to_str().unwrap().contains("application/json"));
    }
}
