//! Shared test utilities for the type-bridge-server crate.

use std::sync::{Arc, Mutex};

use type_bridge_core_lib::ast::{Clause, Constraint, LiteralValue, Pattern, Value};

use crate::error::PipelineError;
use crate::executor::QueryExecutor;
use crate::pipeline::{PipelineBuilder, QueryPipeline};
use crate::schema_source::InMemorySchemaSource;

/// A minimal TypeQL schema for testing.
pub const SIMPLE_SCHEMA: &str = r#"
define
    attribute name, value string;
    attribute age, value long;
    entity person,
        owns name @key,
        owns age;
"#;

/// Create a simple `Vec<Clause>` for a match+fetch query.
pub fn make_simple_clauses() -> Vec<Clause> {
    vec![
        Clause::Match(vec![Pattern::Entity {
            variable: "p".to_string(),
            type_name: "person".to_string(),
            constraints: vec![Constraint::Has {
                attr_name: "name".to_string(),
                value: Value::Literal(LiteralValue {
                    value: serde_json::json!("Alice"),
                    value_type: "string".to_string(),
                }),
            }],
            is_strict: false,
        }]),
        Clause::Fetch(vec![]),
    ]
}

/// Configurable mock executor for testing.
///
/// By default returns a success response. Set `fail_with` to make it return errors.
#[derive(Clone)]
pub struct MockExecutor {
    /// If set, all execute() calls return this error message.
    pub fail_with: Arc<Mutex<Option<String>>>,
    /// Tracks calls: (database, typeql, tx_type).
    pub calls: Arc<Mutex<Vec<(String, String, String)>>>,
    /// The result to return on success.
    pub result: Arc<Mutex<serde_json::Value>>,
    /// Whether is_connected returns true.
    pub connected: Arc<Mutex<bool>>,
}

impl MockExecutor {
    pub fn new() -> Self {
        Self {
            fail_with: Arc::new(Mutex::new(None)),
            calls: Arc::new(Mutex::new(Vec::new())),
            result: Arc::new(Mutex::new(serde_json::json!([{"name": "Alice"}]))),
            connected: Arc::new(Mutex::new(true)),
        }
    }

    pub fn failing(message: &str) -> Self {
        let executor = Self::new();
        *executor.fail_with.lock().unwrap() = Some(message.to_string());
        executor
    }

    pub fn with_result(result: serde_json::Value) -> Self {
        let executor = Self::new();
        *executor.result.lock().unwrap() = result;
        executor
    }
}

impl QueryExecutor for MockExecutor {
    fn execute<'a>(
        &'a self,
        database: &'a str,
        typeql: &'a str,
        transaction_type: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((
                database.to_string(),
                typeql.to_string(),
                transaction_type.to_string(),
            ));

            if let Some(msg) = self.fail_with.lock().unwrap().as_ref() {
                return Err(PipelineError::QueryExecution(msg.clone()));
            }

            Ok(self.result.lock().unwrap().clone())
        })
    }

    fn is_connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }
}

/// Build a pipeline with a mock executor and optional schema.
pub fn make_pipeline(executor: MockExecutor, with_schema: bool) -> QueryPipeline {
    let mut builder = PipelineBuilder::new(executor)
        .with_default_database("test_db");

    if with_schema {
        builder = builder.with_schema_source(InMemorySchemaSource::new(SIMPLE_SCHEMA));
    }

    builder.build().expect("Failed to build test pipeline")
}
