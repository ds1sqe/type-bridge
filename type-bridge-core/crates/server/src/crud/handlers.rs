//! Axum CRUD handlers for entity and relation endpoints.
//!
//! Each handler extracts the path/body, builds AST clauses via the builder,
//! passes them through the [`QueryPipeline`], and returns a [`CrudResponse`].

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;

use crate::error::PipelineError;
use crate::pipeline::{QueryInput, QueryPipeline};

use super::builder;
use super::types::*;

/// POST /entities/{type_name}
///
/// Insert a new entity with the given attributes.
pub async fn handle_entity_insert(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path(type_name): Path<String>,
    Json(req): Json<EntityInsertRequest>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_entity_insert(&type_name, &req.attributes, schema)?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: "write".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// GET /entities/{type_name}
///
/// Fetch entities with optional filters, sort, limit, and offset.
pub async fn handle_entity_fetch(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path(type_name): Path<String>,
    Query(req): Query<EntityFetchQuery>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_entity_fetch(
        &type_name,
        &[],
        &[],
        req.limit,
        req.offset,
        schema,
    )?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: "read".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// Query parameters for GET /entities/{type_name}.
#[derive(Debug, serde::Deserialize)]
pub struct EntityFetchQuery {
    pub database: Option<String>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

/// GET /entities/{type_name}/{iid}
///
/// Fetch a single entity by its internal identifier.
pub async fn handle_entity_get_by_iid(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path((type_name, iid)): Path<(String, String)>,
    Query(params): Query<DatabaseParam>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_entity_fetch_by_iid(&type_name, &iid, schema)?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: params.database,
            transaction_type: "read".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// PUT /entities/{type_name}/{iid}
///
/// Update an entity's attributes by IID.
pub async fn handle_entity_update(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path((type_name, iid)): Path<(String, String)>,
    Json(req): Json<EntityUpdateRequest>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses =
        builder::build_entity_update_by_iid(&type_name, &iid, &req.attributes, schema)?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: "write".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// DELETE /entities/{type_name}/{iid}
///
/// Delete an entity by its internal identifier.
pub async fn handle_entity_delete(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path((type_name, iid)): Path<(String, String)>,
    Query(params): Query<DatabaseParam>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_entity_delete_by_iid(&type_name, &iid, schema)?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: params.database,
            transaction_type: "write".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// POST /relations/{type_name}
///
/// Insert a new relation with role players and optional attributes.
pub async fn handle_relation_insert(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path(type_name): Path<String>,
    Json(req): Json<RelationInsertRequest>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_relation_insert(
        &type_name,
        &req.role_players,
        &req.attributes,
        schema,
    )?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: "write".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// GET /relations/{type_name}
///
/// Fetch relations with optional limit and offset.
pub async fn handle_relation_fetch(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path(type_name): Path<String>,
    Query(req): Query<EntityFetchQuery>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_relation_fetch(
        &type_name,
        &[],
        &[],
        req.limit,
        req.offset,
        schema,
    )?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: req.database,
            transaction_type: "read".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// DELETE /relations/{type_name}/{iid}
///
/// Delete a relation by its internal identifier.
pub async fn handle_relation_delete(
    State(pipeline): State<Arc<QueryPipeline>>,
    Path((type_name, iid)): Path<(String, String)>,
    Query(params): Query<DatabaseParam>,
) -> Result<Json<CrudResponse>, PipelineError> {
    let schema = pipeline
        .schema()
        .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

    let clauses = builder::build_relation_delete_by_iid(&type_name, &iid, schema)?;

    let typeql = type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses);

    let output = pipeline
        .execute_query(QueryInput {
            database: params.database,
            transaction_type: "write".to_string(),
            clauses,
            metadata: HashMap::new(),
        })
        .await?;

    Ok(Json(CrudResponse {
        status: "ok".to_string(),
        results: output.results,
        metadata: CrudMetadata {
            request_id: output.request_id,
            execution_time_ms: output.execution_time_ms,
            typeql,
        },
    }))
}

/// Optional database query parameter.
#[derive(Debug, serde::Deserialize)]
pub struct DatabaseParam {
    pub database: Option<String>,
}
