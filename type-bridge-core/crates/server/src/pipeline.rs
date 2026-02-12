use std::collections::HashMap;
use std::time::Instant;

use type_bridge_core_lib::ast::Clause;
use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_core_lib::query_parser;
use type_bridge_core_lib::schema::TypeSchema;
use type_bridge_core_lib::validation::ValidationEngine;

use crate::error::PipelineError;
use crate::executor::QueryExecutor;
use crate::interceptor::{Interceptor, InterceptorChain, RequestContext};
use crate::schema_source::SchemaSource;

/// Input for a structured (AST-based) query.
pub struct QueryInput {
    pub database: Option<String>,
    pub transaction_type: String,
    pub clauses: Vec<Clause>,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Input for a raw TypeQL query.
pub struct RawQueryInput {
    pub database: Option<String>,
    pub transaction_type: String,
    pub query: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Input for a validation-only request.
pub struct ValidateInput {
    pub clauses: Vec<Clause>,
}

/// Output from a successful pipeline execution.
pub struct QueryOutput {
    pub results: serde_json::Value,
    pub request_id: String,
    pub execution_time_ms: u64,
    pub interceptors_applied: Vec<String>,
}

/// Output from a validation-only request.
pub struct ValidateOutput {
    pub is_valid: bool,
    pub errors: Vec<ValidationErrorDetail>,
}

/// A single validation error.
pub struct ValidationErrorDetail {
    pub code: String,
    pub message: String,
    pub path: String,
}

/// Transport-agnostic query pipeline.
///
/// Encapsulates the full query lifecycle: validate → intercept → compile → execute → intercept.
/// Use [`PipelineBuilder`] to construct an instance.
///
/// # Example
///
/// ```rust,ignore
/// use type_bridge_server::{PipelineBuilder, QueryInput};
///
/// let pipeline = PipelineBuilder::new(my_executor)
///     .with_schema_source(my_schema_source)
///     .with_default_database("my_db")
///     .build()?;
///
/// let output = pipeline.execute_query(QueryInput { ... }).await?;
/// ```
pub struct QueryPipeline {
    schema: Option<TypeSchema>,
    validation_engine: ValidationEngine,
    interceptor_chain: InterceptorChain,
    default_database: String,
    executor: Box<dyn QueryExecutor>,
}

impl QueryPipeline {
    /// Execute a structured (AST-based) query through the full pipeline.
    pub async fn execute_query(&self, input: QueryInput) -> Result<QueryOutput, PipelineError> {
        let start = Instant::now();
        let request_id = uuid::Uuid::new_v4().to_string();
        let database = input
            .database
            .unwrap_or_else(|| self.default_database.clone());

        let mut ctx = RequestContext {
            request_id: request_id.clone(),
            client_id: "unknown".to_string(),
            database: database.clone(),
            transaction_type: input.transaction_type.clone(),
            metadata: input.metadata,
            timestamp: chrono::Utc::now(),
        };

        // Validate against schema
        if let Some(schema) = &self.schema {
            let result = self.validation_engine.validate_query(&input.clauses, schema);
            if !result.is_valid {
                return Err(PipelineError::Validation(format!(
                    "{} validation error(s)",
                    result.errors.len()
                )));
            }
        }

        // Run request interceptors
        let clauses = self
            .interceptor_chain
            .execute_request(input.clauses, &mut ctx)
            .await
            .map_err(|e| PipelineError::Interceptor(e.to_string()))?;

        // Compile to TypeQL
        let compiler = QueryCompiler::new();
        let typeql = compiler.compile(&clauses);
        ctx.metadata.insert(
            "compiled_typeql".to_string(),
            serde_json::Value::String(typeql.clone()),
        );

        // Execute
        tracing::info!(
            database = database.as_str(),
            transaction_type = input.transaction_type.as_str(),
            "Executing query"
        );
        tracing::debug!(typeql = typeql.as_str(), "Compiled TypeQL");

        let results = self
            .executor
            .execute(&database, &typeql, &input.transaction_type)
            .await?;

        // Run response interceptors
        self.interceptor_chain
            .execute_response(&results, &ctx)
            .await
            .map_err(|e| PipelineError::Interceptor(e.to_string()))?;

        let elapsed = start.elapsed().as_millis() as u64;

        Ok(QueryOutput {
            results,
            request_id,
            execution_time_ms: elapsed,
            interceptors_applied: self
                .interceptor_chain
                .interceptor_names()
                .into_iter()
                .map(String::from)
                .collect(),
        })
    }

    /// Execute a raw TypeQL query through the full pipeline (parse → validate → intercept → compile → execute).
    pub async fn execute_raw(&self, input: RawQueryInput) -> Result<QueryOutput, PipelineError> {
        let clauses = query_parser::parse_typeql_query(&input.query)
            .map_err(|e| PipelineError::Parse(e.to_string()))?;

        self.execute_query(QueryInput {
            database: input.database,
            transaction_type: input.transaction_type,
            clauses,
            metadata: input.metadata,
        })
        .await
    }

    /// Validate clauses against the loaded schema without executing.
    pub fn validate(&self, input: &ValidateInput) -> Result<ValidateOutput, PipelineError> {
        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

        let result = self.validation_engine.validate_query(&input.clauses, schema);

        let errors = result
            .errors
            .iter()
            .map(|e| ValidationErrorDetail {
                code: e.code.clone(),
                message: e.message.clone(),
                path: e.path.clone(),
            })
            .collect();

        Ok(ValidateOutput {
            is_valid: result.is_valid,
            errors,
        })
    }

    /// Get the loaded schema, if any.
    pub fn schema(&self) -> Option<&TypeSchema> {
        self.schema.as_ref()
    }

    /// Check if the backend executor is connected.
    pub fn is_connected(&self) -> bool {
        self.executor.is_connected()
    }

    /// Get the default database name.
    pub fn default_database(&self) -> &str {
        &self.default_database
    }
}

/// Builder for constructing a [`QueryPipeline`].
///
/// # Example
///
/// ```rust,ignore
/// use type_bridge_server::PipelineBuilder;
///
/// let pipeline = PipelineBuilder::new(my_executor)
///     .with_schema_source(FileSchemaSource::new("schema.tql"))
///     .with_interceptor(AuditLogInterceptor::new(&config)?)
///     .with_default_database("my_db")
///     .build()?;
/// ```
pub struct PipelineBuilder {
    executor: Box<dyn QueryExecutor>,
    schema_source: Option<Box<dyn SchemaSource>>,
    interceptors: Vec<Box<dyn Interceptor>>,
    default_database: String,
}

impl PipelineBuilder {
    /// Create a new builder with the given query executor.
    pub fn new(executor: impl QueryExecutor + 'static) -> Self {
        Self {
            executor: Box::new(executor),
            schema_source: None,
            interceptors: Vec::new(),
            default_database: String::new(),
        }
    }

    /// Set the schema source. The schema will be loaded during [`build()`](Self::build).
    pub fn with_schema_source(mut self, source: impl SchemaSource + 'static) -> Self {
        self.schema_source = Some(Box::new(source));
        self
    }

    /// Add an interceptor to the pipeline chain.
    pub fn with_interceptor(mut self, interceptor: impl Interceptor + 'static) -> Self {
        self.interceptors.push(Box::new(interceptor));
        self
    }

    /// Set the default database name used when requests don't specify one.
    pub fn with_default_database(mut self, database: impl Into<String>) -> Self {
        self.default_database = database.into();
        self
    }

    /// Build the pipeline, loading the schema if a source was provided.
    pub fn build(self) -> Result<QueryPipeline, PipelineError> {
        let schema = match self.schema_source {
            Some(source) => Some(source.load()?),
            None => None,
        };

        Ok(QueryPipeline {
            schema,
            validation_engine: ValidationEngine::new(),
            interceptor_chain: InterceptorChain::new(self.interceptors),
            default_database: self.default_database,
            executor: self.executor,
        })
    }
}
