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
    fn write_entry(&mut self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        let json = serde_json::to_string(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
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

#[async_trait::async_trait]
impl Interceptor for AuditLogInterceptor {
    fn name(&self) -> &str {
        "audit-log"
    }

    async fn on_request(
        &self,
        clauses: Vec<Clause>,
        ctx: &mut RequestContext,
    ) -> Result<Vec<Clause>, InterceptError> {
        ctx.metadata.insert(
            "audit_clause_count".to_string(),
            serde_json::json!(clauses.len()),
        );
        Ok(clauses)
    }

    async fn on_response(
        &self,
        _result: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
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
            .map_err(|e| InterceptError::Internal(e.to_string()))
    }
}
