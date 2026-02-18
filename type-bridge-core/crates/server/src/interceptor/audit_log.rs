use std::io::Write;
use std::sync::Arc;

use tokio::sync::Mutex;
use type_bridge_core_lib::ast::Clause;

use super::traits::{InterceptError, Interceptor, RequestContext};
use crate::config::AuditLogConfig;

#[derive(Debug, serde::Serialize)]
struct AuditEntry {
    timestamp: String,
    request_id: String,
    client_id: String,
    database: String,
    transaction_type: String,
    clause_count: usize,
    compiled_typeql: Option<String>,
    status: String,
}

enum AuditWriter {
    Stdout,
    File(std::fs::File),
}

impl AuditWriter {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn write_entry(&mut self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        let json = serde_json::to_string(entry)
            .map_err(std::io::Error::other)?;
        match self {
            AuditWriter::Stdout => {
                println!("{}", json);
                Ok(())
            }
            AuditWriter::File(f) => {
                writeln!(f, "{}", json)?;
                f.flush()
            }
        }
    }
}

pub struct AuditLogInterceptor {
    writer: Arc<Mutex<AuditWriter>>,
}

impl AuditLogInterceptor {
    pub fn new(config: &AuditLogConfig) -> Result<Self, String> {
        let writer = match config.output.as_str() {
            "stdout" => AuditWriter::Stdout,
            "file" => {
                if config.file_path.is_empty() {
                    return Err("audit-log file_path is required when output is 'file'".into());
                }
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&config.file_path)
                    .map_err(|e| format!("Failed to open audit log file: {}", e))?;
                AuditWriter::File(file)
            }
            other => return Err(format!("Unknown audit-log output type: {}", other)),
        };
        Ok(Self {
            writer: Arc::new(Mutex::new(writer)),
        })
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn io_to_intercept_error(e: std::io::Error) -> InterceptError {
    InterceptError::Internal(e.to_string())
}

impl Interceptor for AuditLogInterceptor {
    fn name(&self) -> &str {
        "audit-log"
    }

    fn on_request<'a>(
        &'a self,
        clauses: Vec<Clause>,
        ctx: &'a mut RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Clause>, InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            ctx.metadata.insert(
                "audit_clause_count".to_string(),
                serde_json::json!(clauses.len()),
            );
            Ok(clauses)
        })
    }

    fn on_response<'a>(
        &'a self,
        _result: &'a serde_json::Value,
        ctx: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), InterceptError>> + Send + 'a>> {
        Box::pin(async move {
            let entry = AuditEntry {
                timestamp: ctx.timestamp.to_rfc3339(),
                request_id: ctx.request_id.clone(),
                client_id: ctx.client_id.clone(),
                database: ctx.database.clone(),
                transaction_type: ctx.transaction_type.clone(),
                clause_count: ctx
                    .metadata
                    .get("audit_clause_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                compiled_typeql: ctx
                    .metadata
                    .get("compiled_typeql")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                status: "ok".to_string(),
            };

            let mut writer = self.writer.lock().await;
            writer
                .write_entry(&entry)
                .map_err(io_to_intercept_error)
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::AuditLogConfig;

    fn make_ctx() -> RequestContext {
        RequestContext {
            request_id: "req-123".into(),
            client_id: "client-1".into(),
            database: "test-db".into(),
            transaction_type: "read".into(),
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now(),
        }
    }

    // --- Constructor tests ---

    #[test]
    fn new_stdout_output_succeeds() {
        let config = AuditLogConfig {
            output: "stdout".into(),
            file_path: String::new(),
        };
        let result = AuditLogInterceptor::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn new_file_output_with_valid_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let result = AuditLogInterceptor::new(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn new_file_output_with_empty_path_fails() {
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: String::new(),
        };
        let err = AuditLogInterceptor::new(&config).err().expect("Expected error for empty file_path");
        assert!(err.contains("file_path is required"), "Unexpected error: {err}");
    }

    #[test]
    fn new_file_output_with_invalid_path_fails() {
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: "/nonexistent/directory/audit.log".into(),
        };
        let err = AuditLogInterceptor::new(&config).err().expect("Expected error for invalid path");
        assert!(err.contains("Failed to open"), "Unexpected error: {err}");
    }

    #[test]
    fn new_unknown_output_type_fails() {
        let config = AuditLogConfig {
            output: "kafka".into(),
            file_path: String::new(),
        };
        let err = AuditLogInterceptor::new(&config).err().expect("Expected error for unknown output type");
        assert!(err.contains("Unknown audit-log output type: kafka"), "Unexpected error: {err}");
    }

    // --- name() ---

    #[test]
    fn name_returns_audit_log() {
        let config = AuditLogConfig {
            output: "stdout".into(),
            file_path: String::new(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        assert_eq!(interceptor.name(), "audit-log");
    }

    // --- on_request tests ---

    #[tokio::test]
    async fn on_request_inserts_clause_count_metadata() {
        let config = AuditLogConfig {
            output: "stdout".into(),
            file_path: String::new(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        let mut ctx = make_ctx();

        let clauses = vec![
            Clause::Match(vec![]),
            Clause::Fetch(vec![]),
        ];
        interceptor.on_request(clauses, &mut ctx).await.unwrap();

        let count = ctx.metadata.get("audit_clause_count").unwrap();
        assert_eq!(count, &serde_json::json!(2));
    }

    #[tokio::test]
    async fn on_request_returns_clauses_unchanged() {
        let config = AuditLogConfig {
            output: "stdout".into(),
            file_path: String::new(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        let mut ctx = make_ctx();

        let clauses = vec![Clause::Fetch(vec![])];
        let result = interceptor.on_request(clauses, &mut ctx).await.unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn on_request_empty_clauses_count_zero() {
        let config = AuditLogConfig {
            output: "stdout".into(),
            file_path: String::new(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        let mut ctx = make_ctx();

        interceptor.on_request(vec![], &mut ctx).await.unwrap();
        let count = ctx.metadata.get("audit_clause_count").unwrap();
        assert_eq!(count, &serde_json::json!(0));
    }

    // --- on_response tests ---

    #[tokio::test]
    async fn on_response_writes_audit_entry_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();

        let mut ctx = make_ctx();
        ctx.metadata.insert("audit_clause_count".into(), serde_json::json!(3));
        ctx.metadata.insert("compiled_typeql".into(), serde_json::json!("match $p isa person;"));

        interceptor
            .on_response(&serde_json::json!({}), &ctx)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["request_id"], "req-123");
        assert_eq!(entry["client_id"], "client-1");
        assert_eq!(entry["database"], "test-db");
        assert_eq!(entry["transaction_type"], "read");
        assert_eq!(entry["clause_count"], 3);
        assert_eq!(entry["compiled_typeql"], "match $p isa person;");
        assert_eq!(entry["status"], "ok");
    }

    #[tokio::test]
    async fn on_response_clause_count_absent_defaults_to_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        let ctx = make_ctx(); // no metadata

        interceptor
            .on_response(&serde_json::json!({}), &ctx)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["clause_count"], 0);
    }

    #[tokio::test]
    async fn on_response_compiled_typeql_absent_is_null() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        let ctx = make_ctx();

        interceptor
            .on_response(&serde_json::json!({}), &ctx)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert!(entry["compiled_typeql"].is_null());
    }

    #[tokio::test]
    async fn on_response_timestamp_from_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();
        let ctx = make_ctx();
        let expected_ts = ctx.timestamp.to_rfc3339();

        interceptor
            .on_response(&serde_json::json!({}), &ctx)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["timestamp"].as_str().unwrap(), expected_ts);
    }

    #[tokio::test]
    async fn on_response_with_both_metadata_present() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();

        let mut ctx = make_ctx();
        ctx.metadata.insert("audit_clause_count".into(), serde_json::json!(5));
        ctx.metadata.insert("compiled_typeql".into(), serde_json::json!("match $x isa thing;"));

        interceptor
            .on_response(&serde_json::json!({"data": true}), &ctx)
            .await
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["clause_count"], 5);
        assert_eq!(entry["compiled_typeql"], "match $x isa thing;");
    }

    #[tokio::test]
    async fn on_response_multiple_writes_append() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: path.to_str().unwrap().to_string(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();

        let ctx = make_ctx();
        interceptor.on_response(&serde_json::json!({}), &ctx).await.unwrap();
        interceptor.on_response(&serde_json::json!({}), &ctx).await.unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[tokio::test]
    async fn on_response_stdout_writer_succeeds() {
        let config = AuditLogConfig {
            output: "stdout".into(),
            file_path: String::new(),
        };
        let interceptor = AuditLogInterceptor::new(&config).unwrap();

        let mut ctx = make_ctx();
        ctx.metadata.insert("audit_clause_count".into(), serde_json::json!(1));
        ctx.metadata.insert("compiled_typeql".into(), serde_json::json!("match $x isa thing;"));

        // Stdout writer should succeed without error
        let result = interceptor
            .on_response(&serde_json::json!({"data": true}), &ctx)
            .await;
        assert!(result.is_ok());
    }

    // --- AuditEntry serde ---

    #[test]
    fn audit_entry_serializes_with_none_typeql() {
        let entry = AuditEntry {
            timestamp: "2024-01-01T00:00:00Z".into(),
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            clause_count: 0,
            compiled_typeql: None,
            status: "ok".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["compiled_typeql"].is_null());
    }

    #[test]
    fn audit_entry_serializes_with_some_typeql() {
        let entry = AuditEntry {
            timestamp: "2024-01-01T00:00:00Z".into(),
            request_id: "req-1".into(),
            client_id: "client-1".into(),
            database: "db".into(),
            transaction_type: "read".into(),
            clause_count: 2,
            compiled_typeql: Some("match $p isa person;".into()),
            status: "ok".into(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["compiled_typeql"], "match $p isa person;");
        assert_eq!(value["clause_count"], 2);
    }
}
