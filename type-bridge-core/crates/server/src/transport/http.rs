use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use crate::error::PipelineError;
use crate::pipeline::{QueryInput, QueryPipeline, RawQueryInput, ValidateInput};
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
    let output = pipeline
        .execute_raw(RawQueryInput {
            database: req.database,
            transaction_type: req.transaction_type,
            query: req.query,
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

async fn handle_health(
    State(pipeline): State<Arc<QueryPipeline>>,
) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        typedb_connected: pipeline.is_connected(),
    })
}

async fn handle_schema(
    State(pipeline): State<Arc<QueryPipeline>>,
) -> Result<Json<serde_json::Value>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let json = schema
        .to_json()
        .map_err(|e| PipelineError::Schema(e.to_string()))?;

    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| PipelineError::Internal(e.to_string()))?;

    Ok(Json(value))
}
