#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
use std::io::Write;
use std::sync::Arc;

use tokio::sync::Mutex;

#[cfg(feature = "v2-query")]
use super::traits::V3PolicyOutcome;
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
    fn write_entry<T: serde::Serialize + ?Sized>(
        &mut self,
        entry: &T,
    ) -> Result<(), std::io::Error> {
        let json = serde_json::to_string(entry).map_err(std::io::Error::other)?;
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

/// Interceptor that writes one structured JSON audit record per outcome.
pub struct AuditLogInterceptor {
    writer: Arc<Mutex<AuditWriter>>,
}

#[cfg(feature = "v2-query")]
#[derive(Debug, serde::Serialize)]
struct V3AuditEntry {
    timestamp: String,
    request_id: String,
    client_id: String,
    database: String,
    transaction_type: String,
    clause_count: usize,
    compiled_typeql: Option<String>,
    status: String,
    transport: &'static str,
    diagnostic_code: String,
    response_bytes: usize,
}

impl AuditLogInterceptor {
    /// Build an audit interceptor from a validated destination configuration.
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

    fn entry_from_context(ctx: &RequestContext, status: &str) -> AuditEntry {
        AuditEntry {
            timestamp: ctx.timestamp.to_rfc3339(),
            request_id: ctx.request_id.clone(),
            client_id: ctx.client_id.clone(),
            database: ctx.database.clone(),
            transaction_type: ctx.transaction_type.clone(),
            clause_count: ctx
                .metadata
                .get("audit_clause_count")
                .and_then(|value| value.as_u64())
                .unwrap_or(0) as usize,
            compiled_typeql: ctx
                .metadata
                .get("compiled_typeql")
                .and_then(|value| value.as_str())
                .map(String::from),
            status: status.to_owned(),
        }
    }

    #[cfg(feature = "v2-query")]
    async fn write_v3_context(
        &self,
        ctx: &RequestContext,
        outcome: &V3PolicyOutcome<'_>,
    ) -> Result<(), InterceptError> {
        let base = Self::entry_from_context(ctx, if outcome.success() { "ok" } else { "error" });
        let entry = V3AuditEntry {
            timestamp: base.timestamp,
            request_id: base.request_id,
            client_id: base.client_id,
            database: base.database,
            transaction_type: base.transaction_type,
            clause_count: base.clause_count,
            compiled_typeql: base.compiled_typeql,
            status: base.status,
            transport: "v3",
            diagnostic_code: outcome.code().to_owned(),
            response_bytes: outcome.response_bytes(),
        };
        let mut writer = self.writer.lock().await;
        writer.write_entry(&entry).map_err(io_to_intercept_error)
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

    /// The audit log authorizes nothing; it observes V3 replies through
    /// [`Interceptor::on_v3_response`], which `/v3/staged` also runs.
    #[cfg(feature = "v2-query")]
    fn supports_v3(&self) -> bool {
        true
    }

    #[cfg(feature = "v2-query")]
    fn on_v3_response<'a>(
        &'a self,
        outcome: &'a V3PolicyOutcome<'a>,
        ctx: &'a RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), InterceptError>> + Send + 'a>>
    {
        Box::pin(async move { self.write_v3_context(ctx, outcome).await })
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
        let err = AuditLogInterceptor::new(&config)
            .err()
            .expect("Expected error for empty file_path");
        assert!(
            err.contains("file_path is required"),
            "Unexpected error: {err}"
        );
    }

    #[test]
    fn new_file_output_with_invalid_path_fails() {
        let config = AuditLogConfig {
            output: "file".into(),
            file_path: "/nonexistent/directory/audit.log".into(),
        };
        let err = AuditLogInterceptor::new(&config)
            .err()
            .expect("Expected error for invalid path");
        assert!(err.contains("Failed to open"), "Unexpected error: {err}");
    }

    #[test]
    fn new_unknown_output_type_fails() {
        let config = AuditLogConfig {
            output: "kafka".into(),
            file_path: String::new(),
        };
        let err = AuditLogInterceptor::new(&config)
            .err()
            .expect("Expected error for unknown output type");
        assert!(
            err.contains("Unknown audit-log output type: kafka"),
            "Unexpected error: {err}"
        );
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

    #[tokio::test]
    #[cfg(feature = "v2-query")]
    async fn staged_failure_audit_records_only_redacted_deterministic_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let interceptor = AuditLogInterceptor::new(&AuditLogConfig {
            output: "file".into(),
            file_path: path.to_string_lossy().into_owned(),
        })
        .unwrap();
        let mut context = make_ctx();
        context.timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .expect("fixed timestamp")
            .with_timezone(&chrono::Utc);
        interceptor
            .on_v3_response(
                &V3PolicyOutcome::new(false, "query_remote_provider_failed", 128),
                &context,
            )
            .await
            .unwrap();

        let content = std::fs::read_to_string(path).unwrap();
        let entry: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(entry["status"], "error");
        assert_eq!(entry["transport"], "v3");
        assert_eq!(entry["diagnostic_code"], "query_remote_provider_failed");
        assert_eq!(entry["response_bytes"], 128);
        assert_eq!(entry.as_object().expect("entry object").len(), 11);
        assert!(!content.contains("plan"));
        assert!(!content.contains("envelope"));
        assert_eq!(
            content.trim(),
            r#"{"timestamp":"2024-01-01T00:00:00+00:00","request_id":"req-123","client_id":"client-1","database":"test-db","transaction_type":"read","clause_count":0,"compiled_typeql":null,"status":"error","transport":"v3","diagnostic_code":"query_remote_provider_failed","response_bytes":128}"#,
        );
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
