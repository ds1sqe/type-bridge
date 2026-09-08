//! Shared fixtures for external server integration tests.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use type_bridge_core_lib::ast::{Clause, Constraint, LiteralValue, Pattern, Value};
use type_bridge_server::error::PipelineError;
use type_bridge_server::executor::QueryExecutor;
use type_bridge_server::pipeline::{PipelineBuilder, QueryPipeline};
use type_bridge_server::schema_source::InMemorySchemaSource;

pub const SIMPLE_SCHEMA: &str = r#"
define
    attribute name, value string;
    attribute age, value long;
    attribute start-date, value date;
    entity person,
        owns name @key,
        owns age;
    entity company,
        owns name @key;
    relation employment,
        relates employee,
        relates employer,
        owns start-date;
"#;

pub fn make_simple_clauses() -> Vec<Clause> {
    vec![
        Clause::Match(vec![Pattern::Entity {
            variable: "p".to_owned(),
            type_name: "person".to_owned(),
            constraints: vec![Constraint::Has {
                attr_name: "name".to_owned(),
                value: Value::Literal(LiteralValue {
                    value: serde_json::json!("Alice"),
                    value_type: "string".to_owned(),
                }),
            }],
            is_strict: false,
        }]),
        Clause::Fetch(vec![]),
    ]
}

#[derive(Clone)]
pub struct MockExecutor {
    pub fail_with: Arc<Mutex<Option<String>>>,
    pub calls: Arc<Mutex<Vec<(String, String, String)>>>,
    pub result: Arc<Mutex<serde_json::Value>>,
    pub connected: Arc<Mutex<bool>>,
}

impl Default for MockExecutor {
    fn default() -> Self {
        Self::new()
    }
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
        *executor.fail_with.lock().unwrap() = Some(message.to_owned());
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
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, PipelineError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.calls.lock().unwrap().push((
                database.to_owned(),
                typeql.to_owned(),
                transaction_type.to_owned(),
            ));

            if let Some(message) = self.fail_with.lock().unwrap().as_ref() {
                return Err(PipelineError::QueryExecution(message.clone()));
            }

            Ok(self.result.lock().unwrap().clone())
        })
    }

    fn is_connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }
}

pub fn make_pipeline(executor: MockExecutor, with_schema: bool) -> QueryPipeline {
    let mut builder = PipelineBuilder::new(executor).with_default_database("test_db");

    if with_schema {
        builder = builder.with_schema_source(InMemorySchemaSource::new(SIMPLE_SCHEMA));
    }

    builder.build().expect("build test pipeline")
}
