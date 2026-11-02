use crate::error::PipelineError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

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

#[cfg(feature = "typedb")]
impl IntoResponse for crate::typedb::ExecuteError {
    fn into_response(self) -> Response {
        let failure = self.failure();
        failure_response(self.into(), failure)
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
pub fn create_health_router(connected: bool) -> Router {
    Router::new()
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
