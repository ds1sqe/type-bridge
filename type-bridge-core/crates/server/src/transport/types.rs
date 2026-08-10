use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use type_bridge_core_lib::ast::Clause;

/// Request body for POST /query (structured AST).
#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    /// Database override, or `None` to use the server default.
    pub database: Option<String>,
    /// Provider transaction mode requested by the caller.
    pub transaction_type: String,
    /// Structured query clauses to validate and execute.
    pub clauses: Vec<Clause>,
    /// Transport and application metadata exposed to interceptors.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Request body for POST /query/raw.
#[derive(Debug, Deserialize)]
pub struct RawQueryRequest {
    /// Database override, or `None` to use the server default.
    pub database: Option<String>,
    /// Provider transaction mode requested by the caller.
    pub transaction_type: String,
    /// Raw TypeQL query text to parse and execute.
    pub query: String,
    /// Transport and application metadata exposed to interceptors.
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Request body for POST /query/validate.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    /// Structured query clauses to validate without execution.
    pub clauses: Vec<Clause>,
}

/// Successful query response.
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    /// Stable response status, normally `ok`.
    pub status: String,
    /// Provider result encoded as JSON.
    pub results: serde_json::Value,
    /// Pipeline execution metadata.
    pub metadata: ResponseMetadata,
}

#[derive(Debug, Serialize)]
/// Metadata returned with a successful query result.
pub struct ResponseMetadata {
    /// Unique identifier assigned to the request.
    pub request_id: String,
    /// Whole-pipeline execution duration in milliseconds.
    pub execution_time_ms: u64,
    /// Interceptor names applied to the request, in request order.
    pub interceptors_applied: Vec<String>,
}

/// Validation response.
#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    /// Stable response status, normally `ok`.
    pub status: String,
    /// Whether every supplied clause passed static validation.
    pub is_valid: bool,
    /// Validation findings encoded for the HTTP wire.
    pub errors: Vec<serde_json::Value>,
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Service health status.
    pub status: String,
    /// Released V1 HTTP contract version.
    pub version: String,
    /// Whether the configured TypeDB endpoint is reachable.
    pub typedb_connected: bool,
}
