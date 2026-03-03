//! Interceptor chain integration tests through the full pipeline.
//!
//! Tests interceptor ordering, metadata propagation, audit log behavior,
//! and multi-interceptor interactions via `QueryPipeline`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use type_bridge_core_lib::ast::Clause;
use type_bridge_server::config::AuditLogConfig;
use type_bridge_server::interceptor::audit_log::AuditLogInterceptor;
use type_bridge_server::interceptor::{InterceptError, Interceptor, RequestContext};
use type_bridge_server::pipeline::{PipelineBuilder, QueryInput};
use type_bridge_server::test_helpers::MockExecutor;

// ── Helper interceptors ──────────────────────────────────────────────

struct OrderTrackingInterceptor {
    name: String,
    order: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Interceptor for OrderTrackingInterceptor {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            self.order
                .lock()
                .unwrap()
                .push(format!("req:{}", self.name));
            Ok(clauses)
        })
    }

    fn on_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            self.order
                .lock()
                .unwrap()
                .push(format!("resp:{}", self.name));
            Ok(())
        })
    }
}

struct MetadataWriterInterceptor {
    key: String,
    value: serde_json::Value,
}

impl Interceptor for MetadataWriterInterceptor {
    fn name(&self) -> &str {
        "metadata-writer"
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            ctx.metadata.insert(self.key.clone(), self.value.clone());
            Ok(clauses)
        })
    }
}

struct MetadataReaderInterceptor {
    key: String,
    found: Arc<std::sync::Mutex<Option<serde_json::Value>>>,
}

impl Interceptor for MetadataReaderInterceptor {
    fn name(&self) -> &str {
        "metadata-reader"
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            let val = ctx.metadata.get(&self.key).cloned();
            *self.found.lock().unwrap() = val;
            Ok(clauses)
        })
    }
}

struct CountingInterceptor {
    name: String,
    request_count: Arc<AtomicUsize>,
    response_count: Arc<AtomicUsize>,
}

impl CountingInterceptor {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            request_count: Arc::new(AtomicUsize::new(0)),
            response_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl Interceptor for CountingInterceptor {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        _ctx: &'a mut RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            Ok(clauses)
        })
    }

    fn on_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        _ctx: &'a RequestContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            self.response_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async {
            Err(InterceptError::AccessDenied {
                reason: "denied".into(),
            })
        })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn make_query_input() -> QueryInput {
    QueryInput {
        database: None,
        transaction_type: "read".to_string(),
        clauses: vec![],
        metadata: HashMap::new(),
    }
}

// ── Tests: Chain ordering ────────────────────────────────────────────

#[tokio::test]
async fn chain_three_interceptors_request_forward_response_reverse() {
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(OrderTrackingInterceptor {
            name: "first".into(),
            order: order.clone(),
        })
        .with_interceptor(OrderTrackingInterceptor {
            name: "second".into(),
            order: order.clone(),
        })
        .with_interceptor(OrderTrackingInterceptor {
            name: "third".into(),
            order: order.clone(),
        })
        .build()
        .unwrap();

    pipeline.execute_query(make_query_input()).await.unwrap();

    let calls = order.lock().unwrap();
    assert_eq!(
        *calls,
        vec![
            "req:first",
            "req:second",
            "req:third",
            "resp:third",
            "resp:second",
            "resp:first",
        ]
    );
}

#[tokio::test]
async fn first_rejects_others_skipped() {
    let second = CountingInterceptor::new("second");
    let third = CountingInterceptor::new("third");
    let second_req = second.request_count.clone();
    let third_req = third.request_count.clone();

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(RejectingRequestInterceptor)
        .with_interceptor(second)
        .with_interceptor(third)
        .build()
        .unwrap();

    let result = pipeline.execute_query(make_query_input()).await;
    assert!(result.is_err());
    assert_eq!(second_req.load(Ordering::SeqCst), 0);
    assert_eq!(third_req.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn middle_rejects_later_skipped_earlier_ran() {
    let first = CountingInterceptor::new("first");
    let third = CountingInterceptor::new("third");
    let first_req = first.request_count.clone();
    let third_req = third.request_count.clone();

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(first)
        .with_interceptor(RejectingRequestInterceptor)
        .with_interceptor(third)
        .build()
        .unwrap();

    let result = pipeline.execute_query(make_query_input()).await;
    assert!(result.is_err());
    assert_eq!(first_req.load(Ordering::SeqCst), 1); // first ran
    assert_eq!(third_req.load(Ordering::SeqCst), 0); // third skipped
}

// ── Tests: Metadata propagation ──────────────────────────────────────

#[tokio::test]
async fn interceptor_adds_metadata_downstream_sees_it() {
    let found = Arc::new(std::sync::Mutex::new(None));

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(MetadataWriterInterceptor {
            key: "tenant_id".into(),
            value: serde_json::json!("acme-corp"),
        })
        .with_interceptor(MetadataReaderInterceptor {
            key: "tenant_id".into(),
            found: found.clone(),
        })
        .build()
        .unwrap();

    pipeline.execute_query(make_query_input()).await.unwrap();

    let val = found.lock().unwrap().clone();
    assert_eq!(val, Some(serde_json::json!("acme-corp")));
}

// ── Tests: Audit log through pipeline ────────────────────────────────

#[tokio::test]
async fn audit_log_writes_complete_entry_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");

    let config = AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().to_string(),
    };
    let audit = AuditLogInterceptor::new(&config).unwrap();

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(audit)
        .build()
        .unwrap();

    pipeline.execute_query(make_query_input()).await.unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(entry["status"], "ok");
    assert!(entry["request_id"].is_string());
    assert!(entry["database"].is_string());
    assert!(entry["timestamp"].is_string());
}

#[tokio::test]
async fn audit_log_appends_multiple_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");

    let config = AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().to_string(),
    };
    let audit = AuditLogInterceptor::new(&config).unwrap();

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(audit)
        .build()
        .unwrap();

    pipeline.execute_query(make_query_input()).await.unwrap();
    pipeline.execute_query(make_query_input()).await.unwrap();
    pipeline.execute_query(make_query_input()).await.unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.trim().lines().collect();
    assert_eq!(lines.len(), 3);

    // Each line should be valid JSON
    for line in &lines {
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(entry["status"], "ok");
    }
}

#[tokio::test]
async fn audit_log_records_clause_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");

    let config = AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().to_string(),
    };
    let audit = AuditLogInterceptor::new(&config).unwrap();

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(audit)
        .build()
        .unwrap();

    // Execute with 2 clauses
    let input = QueryInput {
        database: None,
        transaction_type: "read".to_string(),
        clauses: vec![Clause::Match(vec![]), Clause::Fetch(vec![])],
        metadata: HashMap::new(),
    };
    pipeline.execute_query(input).await.unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(entry["clause_count"], 2);
}

// ── Tests: Pipeline with multiple interceptors ───────────────────────

#[tokio::test]
async fn pipeline_with_multiple_interceptors_ordering() {
    let count1 = CountingInterceptor::new("counter1");
    let count2 = CountingInterceptor::new("counter2");
    let req1 = count1.request_count.clone();
    let resp1 = count1.response_count.clone();
    let req2 = count2.request_count.clone();
    let resp2 = count2.response_count.clone();

    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(count1)
        .with_interceptor(count2)
        .build()
        .unwrap();

    let output = pipeline.execute_query(make_query_input()).await.unwrap();

    assert_eq!(req1.load(Ordering::SeqCst), 1);
    assert_eq!(req2.load(Ordering::SeqCst), 1);
    assert_eq!(resp1.load(Ordering::SeqCst), 1);
    assert_eq!(resp2.load(Ordering::SeqCst), 1);
    assert_eq!(
        output.interceptors_applied,
        vec!["counter1", "counter2"]
    );
}

#[tokio::test]
async fn pipeline_request_metadata_propagates_to_response_interceptors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");

    let config = AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().to_string(),
    };
    let audit = AuditLogInterceptor::new(&config).unwrap();

    // Metadata writer runs first, sets a value.
    // Audit log runs second, stores clause_count in metadata and writes in on_response.
    let pipeline = PipelineBuilder::new(MockExecutor::new())
        .with_interceptor(MetadataWriterInterceptor {
            key: "custom_marker".into(),
            value: serde_json::json!("test-value"),
        })
        .with_interceptor(audit)
        .build()
        .unwrap();

    let output = pipeline.execute_query(make_query_input()).await.unwrap();

    assert_eq!(
        output.interceptors_applied,
        vec!["metadata-writer", "audit-log"]
    );

    // Audit entry should have been written
    let content = std::fs::read_to_string(&path).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(entry["status"], "ok");
}

#[tokio::test]
async fn pipeline_with_audit_interceptor_full_flow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.jsonl");

    let config = AuditLogConfig {
        output: "file".into(),
        file_path: path.to_str().unwrap().to_string(),
    };
    let audit = AuditLogInterceptor::new(&config).unwrap();

    let executor = MockExecutor::with_result(serde_json::json!({"data": [1, 2, 3]}));
    let calls = executor.calls.clone();

    let pipeline = PipelineBuilder::new(executor)
        .with_default_database("test_db")
        .with_interceptor(audit)
        .build()
        .unwrap();

    let output = pipeline.execute_query(make_query_input()).await.unwrap();

    // Verify execution happened
    assert_eq!(output.results, serde_json::json!({"data": [1, 2, 3]}));
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "test_db");

    // Verify audit log was written
    let content = std::fs::read_to_string(&path).unwrap();
    let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
    assert_eq!(entry["database"], "test_db");
    assert_eq!(entry["status"], "ok");
}
