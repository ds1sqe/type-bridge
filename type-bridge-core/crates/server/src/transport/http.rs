use super::staged_state::{
    await_before_deadline, response_audit_deadline, staged_request_metadata,
};
use crate::error::PipelineError;
use crate::interceptor::V3PolicyOutcome;
use crate::pipeline::QueryPipeline;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

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
        let failure = self.failure();
        failure_response(self, failure)
    }
}

fn failure_response(
    error: PipelineError,
    failure: Option<type_bridge_contract::failure::Failure>,
) -> Response {
    let (status, code) = match &error {
        PipelineError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "CONFIG_ERROR"),
        PipelineError::Connection(_) => (StatusCode::SERVICE_UNAVAILABLE, "CONNECTION_ERROR"),
        PipelineError::UnsupportedVersion(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "UNSUPPORTED_VERSION")
        }
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
            message: error.to_string(),
            details: failure.map(|failure| failure.host_json()),
        },
    };

    (status, axum::Json(body)).into_response()
}

/// Health-only mount used when staged execution is disabled.
pub fn create_health_router(connected: bool, pipeline: Arc<QueryPipeline>) -> Router {
    Router::new()
        .route(
            "/v3/raw",
            post(move |headers: HeaderMap| {
                let pipeline = Arc::clone(&pipeline);
                async move { raw_disabled_with_pipeline(&pipeline, &headers).await }
            }),
        )
        .route(
            "/health",
            get(move || async move { health_response(connected) }),
        )
        .fallback(route_removed)
}

pub(crate) async fn route_removed() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error":"route_removed","use":"/v3/staged"})),
    )
}

#[derive(Serialize)]
pub(crate) struct HealthBody {
    status: &'static str,
    version: &'static str,
    typedb_connected: bool,
}

pub(crate) fn health_response(typedb_connected: bool) -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        version: "1.5.11",
        typedb_connected,
    })
}

pub(crate) async fn raw_disabled() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({"error":"raw_disabled"})),
    )
        .into_response()
}

pub(super) async fn raw_disabled_with_pipeline(
    pipeline: &QueryPipeline,
    headers: &HeaderMap,
) -> Response {
    let mut metadata = staged_request_metadata(headers, "raw");
    metadata.insert("query_format".into(), serde_json::json!("raw-typeql"));
    let ctx = pipeline.request_context(metadata);
    let outcome = V3PolicyOutcome::new(false, "raw_disabled", 0);
    let _ = await_before_deadline(
        response_audit_deadline(Instant::now()),
        pipeline.finish_v3_request(&outcome, &ctx),
    )
    .await;
    raw_disabled().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn disabled_staged_router_audits_raw_refusals() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("disabled.jsonl");
        let audit = crate::interceptor::audit_log::AuditLogInterceptor::new(
            &crate::config::AuditLogConfig {
                output: "file".into(),
                file_path: path.to_str().unwrap().into(),
            },
        )
        .unwrap();
        let pipeline = crate::pipeline::PipelineBuilder::new()
            .with_interceptor(audit)
            .build()
            .unwrap();
        let router = create_health_router(false, Arc::new(pipeline));
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v3/raw")
                    .body(axum::body::Body::from("malformed and never collected"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let records = std::fs::read_to_string(path).unwrap();
        let record: serde_json::Value = serde_json::from_str(records.trim()).unwrap();
        assert_eq!(record["diagnostic_code"], "raw_disabled");
        assert_eq!(record["status"], "error");
    }
}
