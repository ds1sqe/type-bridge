use std::collections::HashMap;
use std::time::Instant;

use type_bridge_core_lib::_schema::TypeSchema;
use type_bridge_core_lib::ast::Clause;
use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_core_lib::validation::ValidationEngine;

use crate::error::PipelineError;
use crate::executor::QueryExecutor;
use crate::interceptor::crud_interceptor::{CrudInterceptor, CrudInterceptorAdapter};
use crate::interceptor::{Interceptor, InterceptorChain, RequestContext};
#[cfg(feature = "v2-query")]
use crate::interceptor::{V2PolicyOutcome, V2PolicyRequest};
use crate::schema_source::SchemaSource;

/// Input for a structured (AST-based) query.
pub struct QueryInput {
    /// Database override, or `None` to use the pipeline default.
    pub database: Option<String>,
    /// Provider transaction mode requested by the caller.
    pub transaction_type: String,
    /// Structured query clauses to validate and execute.
    pub clauses: Vec<Clause>,
    /// Transport and application metadata exposed to interceptors.
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Input for a validation-only request.
pub struct ValidateInput {
    /// Structured query clauses to validate without execution.
    pub clauses: Vec<Clause>,
}

/// Output from a successful pipeline execution.
#[derive(Debug)]
pub struct QueryOutput {
    /// Provider result encoded as JSON.
    pub results: serde_json::Value,
    /// Unique identifier assigned to the request.
    pub request_id: String,
    /// Whole-pipeline execution duration in milliseconds.
    pub execution_time_ms: u64,
    /// Interceptor names applied to the request, in request order.
    pub interceptors_applied: Vec<String>,
}

/// Output from a validation-only request.
#[derive(Debug)]
pub struct ValidateOutput {
    /// Whether every supplied clause passed static validation.
    pub is_valid: bool,
    /// Deterministically ordered validation findings.
    pub errors: Vec<ValidationErrorDetail>,
}

/// A single validation error.
#[derive(Debug)]
pub struct ValidationErrorDetail {
    /// Stable machine-readable validation code.
    pub code: String,
    /// Human-readable validation explanation.
    pub message: String,
    /// Logical location of the invalid query element.
    pub path: String,
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn log_query_execution(database: &str, transaction_type: &str, typeql: &str) {
    tracing::info!(database, transaction_type, "Executing query");
    tracing::debug!(typeql, "Compiled TypeQL");
}

/// Transport-agnostic query pipeline.
///
/// Encapsulates the full query lifecycle: validate → intercept → compile → execute → intercept.
/// Use [`PipelineBuilder`] to construct an instance.
///
/// # Example
///
/// This example is ignored because it relies on application-defined executor
/// and schema-source implementations.
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
    skip_validation: bool,
}

impl QueryPipeline {
    /// Prove every configured policy explicitly covers typed V2 requests.
    ///
    /// Server startup calls this before constructing the V2 router/listener;
    /// request admission rechecks it defensively through
    /// [`Self::v2_request_context`].
    #[cfg(feature = "v2-query")]
    pub fn validate_v2_coverage(&self) -> Result<(), PipelineError> {
        self.interceptor_chain
            .ensure_v2_coverage()
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Construct one V2 policy context after proving every configured
    /// interceptor explicitly supports typed plans.
    #[cfg(feature = "v2-query")]
    pub fn v2_request_context(
        &self,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<RequestContext, PipelineError> {
        self.validate_v2_coverage()?;
        Ok(RequestContext {
            request_id: uuid::Uuid::new_v4().to_string(),
            client_id: "unknown".to_owned(),
            database: self.default_database.clone(),
            transaction_type: "read".to_owned(),
            metadata,
            timestamp: chrono::Utc::now(),
            crud_info: None,
        })
    }

    /// Authenticate and rate-limit V2 transport metadata before envelope
    /// decoding or other attacker-controlled contract work.
    #[cfg(feature = "v2-query")]
    pub async fn begin_v2_request(
        &self,
        context: &mut RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_v2_transport(context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Authorize one exact validated V2 plan before replay/provider admission.
    #[cfg(feature = "v2-query")]
    pub async fn authorize_v2_request(
        &self,
        request: &V2PolicyRequest<'_>,
        context: &mut RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_v2_request(request, context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Run configured response and audit policies for every V2 outcome.
    #[cfg(feature = "v2-query")]
    pub async fn finish_v2_request(
        &self,
        outcome: &V2PolicyOutcome<'_>,
        context: &RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_v2_response(outcome, context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

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
            crud_info: None,
        };

        // Validate against schema
        if !self.skip_validation
            && let Some(schema) = &self.schema
        {
            let result = self
                .validation_engine
                .validate_query(&input.clauses, schema);
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
        log_query_execution(&database, &input.transaction_type, &typeql);

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

    /// Validate clauses against the loaded schema without executing.
    pub fn validate(&self, input: &ValidateInput) -> Result<ValidateOutput, PipelineError> {
        let schema = self
            .schema
            .as_ref()
            .ok_or_else(|| PipelineError::Schema("No schema loaded".to_string()))?;

        let result = self
            .validation_engine
            .validate_query(&input.clauses, schema);

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
/// This example is ignored because it relies on application-defined executor,
/// schema-source, and interceptor implementations.
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
    skip_validation: bool,
}

impl PipelineBuilder {
    /// Create a new builder with the given query executor.
    pub fn new(executor: impl QueryExecutor + 'static) -> Self {
        Self {
            executor: Box::new(executor),
            schema_source: None,
            interceptors: Vec::new(),
            default_database: String::new(),
            skip_validation: false,
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

    /// Add a CRUD-aware interceptor to the pipeline chain.
    ///
    /// The interceptor is automatically wrapped in a [`CrudInterceptorAdapter`]
    /// that extracts [`CrudInfo`](crate::interceptor::CrudInfo) and delegates
    /// to the CRUD-specific hooks.
    pub fn with_crud_interceptor(self, interceptor: impl CrudInterceptor + 'static) -> Self {
        self.with_interceptor(CrudInterceptorAdapter::new(interceptor))
    }

    /// Skip schema validation during query execution.
    ///
    /// The schema is still loaded (and accessible via [`QueryPipeline::schema`]),
    /// but queries are not validated against it before execution.
    pub fn with_skip_validation(mut self) -> Self {
        self.skip_validation = true;
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
            skip_validation: self.skip_validation,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use type_bridge_core_lib::ast::{Constraint, Pattern, Value};

    use super::*;
    use crate::interceptor::traits::InterceptError;
    use crate::test_helpers::{MockExecutor, make_pipeline, make_simple_clauses};

    fn init_tracing() -> tracing::subscriber::DefaultGuard {
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    // --- Helper interceptors ---

    struct PassthroughInterceptor {
        name: String,
    }

    impl Interceptor for PassthroughInterceptor {
        fn name(&self) -> &str {
            &self.name
        }
        fn on_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(clauses) })
        }
    }

    struct RejectingRequestInterceptor;

    impl Interceptor for RejectingRequestInterceptor {
        fn name(&self) -> &str {
            "rejector"
        }
        fn on_request<'a>(
            &'a self,
            _clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(InterceptError::AccessDenied {
                    reason: "test rejection".into(),
                })
            })
        }

        #[cfg(feature = "v2-query")]
        fn supports_v2(&self) -> bool {
            true
        }

        #[cfg(feature = "v2-query")]
        fn on_v2_transport<'a>(
            &'a self,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async {
                Err(InterceptError::AccessDenied {
                    reason: "test rejection".into(),
                })
            })
        }
    }

    struct RejectingResponseInterceptor;

    impl Interceptor for RejectingResponseInterceptor {
        fn name(&self) -> &str {
            "resp-rejector"
        }
        fn on_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(clauses) })
        }
        fn on_response<'a>(
            &'a self,
            _result: &'a serde_json::Value,
            _ctx: &'a RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
            Box::pin(async { Err(InterceptError::Internal("response rejected".into())) })
        }
    }

    struct CountingInterceptor {
        name: String,
        count: Arc<AtomicUsize>,
    }

    impl Interceptor for CountingInterceptor {
        fn name(&self) -> &str {
            &self.name
        }
        fn on_request<'a>(
            &'a self,
            clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async move {
                self.count.fetch_add(1, Ordering::SeqCst);
                Ok(clauses)
            })
        }
    }

    /// SchemaSource that always fails.
    struct FailingSchemaSource;

    impl crate::schema_source::SchemaSource for FailingSchemaSource {
        fn load(&self) -> Result<TypeSchema, PipelineError> {
            Err(PipelineError::Schema("source failed".into()))
        }
    }

    fn make_query_input(clauses: Vec<Clause>) -> QueryInput {
        QueryInput {
            database: None,
            transaction_type: "read".to_string(),
            clauses,
            metadata: HashMap::new(),
        }
    }

    fn make_query_input_with_db(clauses: Vec<Clause>, db: &str) -> QueryInput {
        QueryInput {
            database: Some(db.to_string()),
            transaction_type: "read".to_string(),
            clauses,
            metadata: HashMap::new(),
        }
    }

    // =============================================
    // PipelineBuilder tests
    // =============================================

    #[test]
    fn builder_without_schema_source() {
        let pipeline = PipelineBuilder::new(MockExecutor::new()).build().unwrap();
        assert!(pipeline.schema().is_none());
    }

    #[test]
    fn builder_with_valid_schema_source() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        assert!(pipeline.schema().is_some());
        let schema = pipeline.schema().unwrap();
        assert!(schema.entities.contains_key("person"));
    }

    #[test]
    fn builder_with_failing_schema_source() {
        let result = PipelineBuilder::new(MockExecutor::new())
            .with_schema_source(FailingSchemaSource)
            .build();
        let err = result.err().expect("Expected build error");
        assert!(matches!(&err, PipelineError::Schema(msg) if msg.contains("source failed")));
    }

    #[test]
    fn builder_with_default_database() {
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_default_database("mydb")
            .build()
            .unwrap();
        assert_eq!(pipeline.default_database(), "mydb");
    }

    #[test]
    fn builder_default_empty_database() {
        let pipeline = PipelineBuilder::new(MockExecutor::new()).build().unwrap();
        assert_eq!(pipeline.default_database(), "");
    }

    #[tokio::test]
    async fn builder_with_interceptors() {
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(PassthroughInterceptor {
                name: "first".into(),
            })
            .with_interceptor(PassthroughInterceptor {
                name: "second".into(),
            })
            .build()
            .unwrap();
        assert!(pipeline.schema().is_none());

        let input = make_query_input(vec![]);
        let output = pipeline.execute_query(input).await.unwrap();
        assert_eq!(output.interceptors_applied, vec!["first", "second"]);
    }

    #[tokio::test]
    #[cfg(feature = "v2-query")]
    async fn v2_policy_rejection_is_fail_closed() {
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(RejectingRequestInterceptor)
            .build()
            .unwrap();
        let mut context = pipeline
            .v2_request_context(HashMap::from([(
                "transport".to_owned(),
                serde_json::json!("v2"),
            )]))
            .expect("V2-aware policy coverage");
        let error = pipeline
            .begin_v2_request(&mut context)
            .await
            .expect_err("the same request policy must gate V2 envelopes");
        assert!(
            matches!(error, PipelineError::Interceptor(message) if message.contains("test rejection"))
        );
    }

    #[cfg(feature = "v2-query")]
    struct RewritingInterceptor;

    #[cfg(feature = "v2-query")]
    impl Interceptor for RewritingInterceptor {
        fn name(&self) -> &str {
            "rewriter"
        }

        fn on_request<'a>(
            &'a self,
            _clauses: Vec<Clause>,
            _ctx: &'a mut RequestContext,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>>
        {
            Box::pin(async { Ok(make_simple_clauses()) })
        }
    }

    #[tokio::test]
    #[cfg(feature = "v2-query")]
    async fn v2_rejects_legacy_ast_policies_without_typed_coverage() {
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(RewritingInterceptor)
            .build()
            .unwrap();
        let startup_error = pipeline
            .validate_v2_coverage()
            .expect_err("startup must reject a legacy-only policy before routing");
        assert!(
            matches!(startup_error, PipelineError::Interceptor(message) if message.contains("does not declare typed V2 coverage"))
        );
        let error = pipeline
            .v2_request_context(HashMap::new())
            .expect_err("request admission must retain the defensive recheck");
        assert!(
            matches!(error, PipelineError::Interceptor(message) if message.contains("does not declare typed V2 coverage"))
        );
    }

    // =============================================
    // execute_query tests
    // =============================================

    #[tokio::test]
    async fn execute_query_uses_input_database() {
        let executor = MockExecutor::new();
        let calls = executor.calls.clone();
        let pipeline = make_pipeline(executor, false);

        let input = make_query_input_with_db(vec![], "custom_db");
        pipeline.execute_query(input).await.unwrap();

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded[0].0, "custom_db");
    }

    #[tokio::test]
    async fn execute_query_uses_default_database_when_none() {
        let executor = MockExecutor::new();
        let calls = executor.calls.clone();
        let pipeline = make_pipeline(executor, false);

        let input = make_query_input(vec![]);
        pipeline.execute_query(input).await.unwrap();

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded[0].0, "test_db"); // from make_pipeline
    }

    #[tokio::test]
    async fn execute_query_skips_validation_when_no_schema() {
        let pipeline = make_pipeline(MockExecutor::new(), false);
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "x".to_string(),
            type_name: "nonexistent_type".to_string(),
            constraints: vec![],
            is_strict: false,
        }])];
        let input = make_query_input(clauses);
        let result = pipeline.execute_query(input).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_query_validates_when_schema_present_valid() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        let input = make_query_input(make_simple_clauses());
        let result = pipeline.execute_query(input).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_query_validates_when_schema_present_invalid() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        let clauses = vec![Clause::Match(vec![Pattern::Entity {
            variable: "p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "nonexistent_attr".to_string(),
                value: Value::Literal(type_bridge_core_lib::ast::LiteralValue {
                    value: serde_json::json!("val"),
                    value_type: "string".to_string(),
                }),
            }],
            is_strict: false,
        }])];
        let input = make_query_input(clauses);
        let result = pipeline.execute_query(input).await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::Validation(msg) if msg.contains("validation error")));
    }

    #[tokio::test]
    async fn execute_query_request_interceptor_failure() {
        assert_eq!(RejectingRequestInterceptor.name(), "rejector");
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(RejectingRequestInterceptor)
            .build()
            .unwrap();
        let input = make_query_input(vec![]);
        let result = pipeline.execute_query(input).await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::Interceptor(msg) if msg.contains("test rejection")));
    }

    #[tokio::test]
    async fn execute_query_executor_failure() {
        let pipeline = make_pipeline(MockExecutor::failing("db crash"), false);
        let input = make_query_input(vec![]);
        let result = pipeline.execute_query(input).await;
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::QueryExecution(msg) if msg.contains("db crash")));
    }

    #[tokio::test]
    async fn execute_query_response_interceptor_failure() {
        assert_eq!(RejectingResponseInterceptor.name(), "resp-rejector");
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_interceptor(RejectingResponseInterceptor)
            .build()
            .unwrap();
        let input = make_query_input(vec![]);
        let result = pipeline.execute_query(input).await;
        let err = result.unwrap_err();
        assert!(
            matches!(&err, PipelineError::Interceptor(msg) if msg.contains("response rejected"))
        );
    }

    #[tokio::test]
    async fn execute_query_success_output_fields() {
        let _guard = init_tracing();
        let count = Arc::new(AtomicUsize::new(0));
        let pipeline =
            PipelineBuilder::new(MockExecutor::with_result(serde_json::json!({"ok": true})))
                .with_default_database("test_db")
                .with_interceptor(CountingInterceptor {
                    name: "counter".into(),
                    count: count.clone(),
                })
                .build()
                .unwrap();

        let input = make_query_input(vec![]);
        let output = pipeline.execute_query(input).await.unwrap();

        assert!(!output.request_id.is_empty());
        assert_eq!(output.results, serde_json::json!({"ok": true}));
        assert_eq!(output.interceptors_applied, vec!["counter"]);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn execute_query_empty_clauses_success() {
        let pipeline = make_pipeline(MockExecutor::new(), false);
        let input = make_query_input(vec![]);
        let result = pipeline.execute_query(input).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn execute_query_compiled_typeql_in_metadata() {
        let executor = MockExecutor::new();
        let calls = executor.calls.clone();
        let pipeline = make_pipeline(executor, false);

        let clauses = make_simple_clauses();
        let input = make_query_input(clauses);
        pipeline.execute_query(input).await.unwrap();

        let recorded = calls.lock().unwrap();
        assert!(!recorded[0].1.is_empty());
    }

    #[tokio::test]
    async fn execute_query_passes_transaction_type() {
        let executor = MockExecutor::new();
        let calls = executor.calls.clone();
        let pipeline = make_pipeline(executor, false);

        let input = QueryInput {
            database: None,
            transaction_type: "write".to_string(),
            clauses: vec![],
            metadata: HashMap::new(),
        };
        pipeline.execute_query(input).await.unwrap();

        let recorded = calls.lock().unwrap();
        assert_eq!(recorded[0].2, "write");
    }

    // =============================================
    // validate tests
    // =============================================

    #[test]
    fn validate_no_schema_returns_error() {
        let pipeline = make_pipeline(MockExecutor::new(), false);
        let input = ValidateInput { clauses: vec![] };
        let result = pipeline.validate(&input);
        let err = result.unwrap_err();
        assert!(matches!(&err, PipelineError::Schema(msg) if msg.contains("No schema loaded")));
    }

    #[test]
    fn validate_valid_clauses() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        let input = ValidateInput {
            clauses: make_simple_clauses(),
        };
        let result = pipeline.validate(&input).unwrap();
        assert!(result.is_valid);
        assert!(result.errors.is_empty());
    }

    #[test]
    fn validate_invalid_clauses() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        let input = ValidateInput {
            clauses: vec![Clause::Match(vec![Pattern::Entity {
                variable: "p".to_string(),
                type_name: "person".to_string(),
                constraints: vec![Constraint::Has {
                    attr_name: "nonexistent_attr".to_string(),
                    value: Value::Literal(type_bridge_core_lib::ast::LiteralValue {
                        value: serde_json::json!("val"),
                        value_type: "string".to_string(),
                    }),
                }],
                is_strict: false,
            }])],
        };
        let result = pipeline.validate(&input).unwrap();
        assert!(!result.is_valid);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn validate_error_detail_fields() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        let input = ValidateInput {
            clauses: vec![Clause::Match(vec![Pattern::Entity {
                variable: "x".to_string(),
                type_name: "person".to_string(),
                constraints: vec![Constraint::Has {
                    attr_name: "nonexistent_attr".to_string(),
                    value: Value::Literal(type_bridge_core_lib::ast::LiteralValue {
                        value: serde_json::json!("val"),
                        value_type: "string".to_string(),
                    }),
                }],
                is_strict: false,
            }])],
        };
        let result = pipeline.validate(&input).unwrap();
        assert!(!result.is_valid);
        let error = &result.errors[0];
        assert!(!error.code.is_empty());
        assert!(!error.message.is_empty());
    }

    #[test]
    fn validate_empty_clauses_with_schema() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        let input = ValidateInput { clauses: vec![] };
        let result = pipeline.validate(&input).unwrap();
        assert!(result.is_valid);
    }

    // =============================================
    // Accessor tests
    // =============================================

    #[test]
    fn schema_returns_some_when_loaded() {
        let pipeline = make_pipeline(MockExecutor::new(), true);
        assert!(pipeline.schema().is_some());
    }

    #[test]
    fn schema_returns_none_when_not_loaded() {
        let pipeline = make_pipeline(MockExecutor::new(), false);
        assert!(pipeline.schema().is_none());
    }

    #[test]
    fn is_connected_delegates_to_executor() {
        let executor = MockExecutor::new();
        *executor.connected.lock().unwrap() = true;
        let pipeline = make_pipeline(executor, false);
        assert!(pipeline.is_connected());
    }

    #[test]
    fn is_connected_false_when_executor_disconnected() {
        let executor = MockExecutor::new();
        *executor.connected.lock().unwrap() = false;
        let pipeline = make_pipeline(executor, false);
        assert!(!pipeline.is_connected());
    }

    #[test]
    fn default_database_returns_configured_value() {
        let pipeline = PipelineBuilder::new(MockExecutor::new())
            .with_default_database("my_database")
            .build()
            .unwrap();
        assert_eq!(pipeline.default_database(), "my_database");
    }

    #[test]
    fn default_database_empty_when_not_set() {
        let pipeline = PipelineBuilder::new(MockExecutor::new()).build().unwrap();
        assert_eq!(pipeline.default_database(), "");
    }
}
