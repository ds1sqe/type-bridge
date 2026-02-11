use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use type_bridge_core_lib::ast::Clause;

/// Request body for POST /query (structured AST).
#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub database: Option<String>,
    pub transaction_type: String,
    pub clauses: Vec<Clause>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Request body for POST /query/raw (raw TypeQL).
#[derive(Debug, Deserialize)]
pub struct RawQueryRequest {
    pub database: Option<String>,
    pub transaction_type: String,
    pub query: String,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Request body for POST /query/validate.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    pub clauses: Vec<Clause>,
}

/// Successful query response.
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    pub status: String,
    pub results: serde_json::Value,
    pub metadata: ResponseMetadata,
}

#[derive(Debug, Serialize)]
pub struct ResponseMetadata {
    pub request_id: String,
    pub execution_time_ms: u64,
    pub interceptors_applied: Vec<String>,
}

/// Validation response.
#[derive(Debug, Serialize)]
pub struct ValidateResponse {
    pub status: String,
    pub is_valid: bool,
    pub errors: Vec<serde_json::Value>,
}

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub typedb_connected: bool,
}
