use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // variants used once TypeDB driver is integrated
pub enum ServerError {
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("TypeDB connection error: {0}")]
    Connection(String),
    #[error("Query execution error: {0}")]
    QueryExecution(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Schema error: {0}")]
    Schema(String),
    #[error("Interceptor error: {0}")]
    Interceptor(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub status: String,
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            ServerError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, "CONFIG_ERROR"),
            ServerError::Connection(_) => (StatusCode::SERVICE_UNAVAILABLE, "CONNECTION_ERROR"),
            ServerError::QueryExecution(_) => (StatusCode::BAD_REQUEST, "QUERY_EXECUTION_ERROR"),
            ServerError::Validation(_) => (StatusCode::BAD_REQUEST, "VALIDATION_FAILED"),
            ServerError::Parse(_) => (StatusCode::BAD_REQUEST, "PARSE_ERROR"),
            ServerError::Schema(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SCHEMA_ERROR"),
            ServerError::Interceptor(_) => (StatusCode::FORBIDDEN, "INTERCEPTOR_ERROR"),
            ServerError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
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
