use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use type_bridge_core_lib::schema::TypeSchema;
use type_bridge_core_lib::validation::ValidationEngine;

use crate::config::{AuditLogConfig, ServerConfig};
use crate::error::ServerError;
use crate::interceptor::audit_log::AuditLogInterceptor;
use crate::interceptor::{InterceptorChain, Interceptor};

/// Shared application state, accessible from all request handlers.
pub struct AppState {
    pub schema: Option<TypeSchema>,
    pub validation_engine: ValidationEngine,
    pub interceptor_chain: InterceptorChain,
    pub default_database: String,
    typedb_connected: AtomicBool,
}

impl AppState {
    pub fn typedb_connected(&self) -> bool {
        self.typedb_connected.load(Ordering::Relaxed)
    }

    /// Execute a query against TypeDB.
    ///
    /// For the MVP without the TypeDB Rust driver, this returns
    /// a stub indicating the compiled query. This will be replaced
    /// with the actual TypeDB driver integration.
    pub async fn execute_query(
        &self,
        database: &str,
        typeql: &str,
        transaction_type: &str,
    ) -> Result<serde_json::Value, ServerError> {
        // TODO: Replace with actual TypeDB driver execution
        // For now, return the compiled query as a stub response
        tracing::info!(
            database = database,
            transaction_type = transaction_type,
            "Executing query"
        );
        tracing::debug!(typeql = typeql, "Compiled TypeQL");

        Ok(serde_json::json!({
            "stub": true,
            "message": "TypeDB driver not yet integrated",
            "compiled_typeql": typeql,
            "database": database,
            "transaction_type": transaction_type
        }))
    }
}

/// Build the application state from config.
pub fn build_app_state(config: &ServerConfig) -> Result<Arc<AppState>, ServerError> {
    // Load schema if configured
    let schema = if !config.schema.source_file.is_empty() {
        let content = std::fs::read_to_string(&config.schema.source_file)
            .map_err(|e| ServerError::Schema(format!("Failed to read schema file: {}", e)))?;
        let schema = TypeSchema::from_typeql(&content)
            .map_err(|e| ServerError::Schema(format!("Failed to parse schema: {}", e)))?;
        tracing::info!(file = config.schema.source_file.as_str(), "Loaded schema");
        Some(schema)
    } else {
        tracing::info!("No schema file configured, running without schema validation");
        None
    };

    let validation_engine = ValidationEngine::new();

    // Build interceptor chain
    let mut interceptors: Vec<Box<dyn Interceptor>> = Vec::new();
    for name in &config.interceptors.enabled {
        match name.as_str() {
            "audit-log" => {
                let audit_config = config.interceptors.audit_log.clone().unwrap_or(AuditLogConfig {
                    output: "stdout".to_string(),
                    file_path: String::new(),
                });
                let interceptor = AuditLogInterceptor::new(&audit_config)
                    .map_err(|e| ServerError::Config(format!("Failed to create audit-log interceptor: {}", e)))?;
                interceptors.push(Box::new(interceptor));
                tracing::info!("Enabled interceptor: audit-log");
            }
            other => {
                tracing::warn!(name = other, "Unknown interceptor, skipping");
            }
        }
    }
    let interceptor_chain = InterceptorChain::new(interceptors);

    Ok(Arc::new(AppState {
        schema,
        validation_engine,
        interceptor_chain,
        default_database: config.typedb.database.clone(),
        typedb_connected: AtomicBool::new(false),
    }))
}
