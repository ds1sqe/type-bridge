use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};

use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_core_lib::query_parser;

use crate::error::ServerError;
use crate::interceptor::RequestContext;
use crate::server::AppState;
use crate::transport::types::*;

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/query", post(handle_query))
        .route("/query/raw", post(handle_raw_query))
        .route("/query/validate", post(handle_validate))
        .route("/health", get(handle_health))
        .route("/schema", get(handle_schema))
        .with_state(state)
}

async fn handle_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, ServerError> {
    let start = Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();
    let database = req
        .database
        .unwrap_or_else(|| state.default_database.clone());

    let mut ctx = RequestContext {
        request_id: request_id.clone(),
        client_id: "unknown".to_string(),
        database: database.clone(),
        transaction_type: req.transaction_type.clone(),
        metadata: req.metadata,
        timestamp: chrono::Utc::now(),
    };

    // Validate query against schema
    if let Some(schema) = &state.schema {
        let result = state.validation_engine.validate_query(&req.clauses, schema);
        if !result.is_valid {
            return Err(ServerError::Validation(format!(
                "{} validation error(s)",
                result.errors.len()
            )));
        }
    }

    // Run interceptor chain
    let clauses = state
        .interceptor_chain
        .execute_request(req.clauses, &mut ctx)
        .await
        .map_err(|e| ServerError::Interceptor(e.to_string()))?;

    // Compile to TypeQL
    let compiler = QueryCompiler::new();
    let typeql = compiler.compile(&clauses);
    ctx.metadata.insert(
        "compiled_typeql".to_string(),
        serde_json::Value::String(typeql.clone()),
    );

    // Execute against TypeDB
    let results = state
        .execute_query(&database, &typeql, &req.transaction_type)
        .await?;

    // Run response interceptors
    state
        .interceptor_chain
        .execute_response(&results, &ctx)
        .await
        .map_err(|e| ServerError::Interceptor(e.to_string()))?;

    let elapsed = start.elapsed().as_millis() as u64;

    Ok(Json(QueryResponse {
        status: "ok".to_string(),
        results,
        metadata: ResponseMetadata {
            request_id,
            execution_time_ms: elapsed,
            interceptors_applied: state.interceptor_chain.interceptor_names().into_iter().map(String::from).collect(),
        },
    }))
}

async fn handle_raw_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RawQueryRequest>,
) -> Result<Json<QueryResponse>, ServerError> {
    let start = Instant::now();
    let request_id = uuid::Uuid::new_v4().to_string();
    let database = req
        .database
        .unwrap_or_else(|| state.default_database.clone());

    // Parse raw TypeQL to AST
    let clauses = query_parser::parse_typeql_query(&req.query)
        .map_err(|e| ServerError::Parse(e.to_string()))?;

    let mut ctx = RequestContext {
        request_id: request_id.clone(),
        client_id: "unknown".to_string(),
        database: database.clone(),
        transaction_type: req.transaction_type.clone(),
        metadata: req.metadata,
        timestamp: chrono::Utc::now(),
    };

    // Validate query against schema
    if let Some(schema) = &state.schema {
        let result = state.validation_engine.validate_query(&clauses, schema);
        if !result.is_valid {
            return Err(ServerError::Validation(format!(
                "{} validation error(s)",
                result.errors.len()
            )));
        }
    }

    // Run interceptor chain
    let clauses = state
        .interceptor_chain
        .execute_request(clauses, &mut ctx)
        .await
        .map_err(|e| ServerError::Interceptor(e.to_string()))?;

    // Compile to TypeQL
    let compiler = QueryCompiler::new();
    let typeql = compiler.compile(&clauses);
    ctx.metadata.insert(
        "compiled_typeql".to_string(),
        serde_json::Value::String(typeql.clone()),
    );

    // Execute against TypeDB
    let results = state
        .execute_query(&database, &typeql, &req.transaction_type)
        .await?;

    // Run response interceptors
    state
        .interceptor_chain
        .execute_response(&results, &ctx)
        .await
        .map_err(|e| ServerError::Interceptor(e.to_string()))?;

    let elapsed = start.elapsed().as_millis() as u64;

    Ok(Json(QueryResponse {
        status: "ok".to_string(),
        results,
        metadata: ResponseMetadata {
            request_id,
            execution_time_ms: elapsed,
            interceptors_applied: state.interceptor_chain.interceptor_names().into_iter().map(String::from).collect(),
        },
    }))
}

async fn handle_validate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, ServerError> {
    let schema = state
        .schema
        .as_ref()
        .ok_or_else(|| ServerError::Schema("No schema loaded".to_string()))?;

    let result = state.validation_engine.validate_query(&req.clauses, schema);

    let errors: Vec<serde_json::Value> = result
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
        is_valid: result.is_valid,
        errors,
    }))
}

async fn handle_health(
    State(state): State<Arc<AppState>>,
) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        typedb_connected: state.typedb_connected(),
    })
}

async fn handle_schema(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ServerError> {
    let schema = state
        .schema
        .as_ref()
        .ok_or_else(|| ServerError::Schema("No schema loaded".to_string()))?;

    let json = schema
        .to_json()
        .map_err(|e| ServerError::Schema(e.to_string()))?;

    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| ServerError::Internal(e.to_string()))?;

    Ok(Json(value))
}
