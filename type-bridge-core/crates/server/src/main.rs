use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use type_bridge_server::config::{AuditLogConfig, ServerConfig};
use type_bridge_server::interceptor::audit_log::AuditLogInterceptor;
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::schema_source::FileSchemaSource;
use type_bridge_server::transport;
use type_bridge_server::typedb::TypeDBClient;

#[derive(Parser)]
#[command(
    name = "type-bridge-server",
    version,
    about = "TypeDB query proxy server"
)]
struct Cli {
    /// Path to the server configuration file
    #[arg(short, long, default_value = "server.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = ServerConfig::from_file(&cli.config)?;

    // Initialize logging
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));

    match config.logging.format.as_str() {
        "json" => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    }

    tracing::info!(
        host = config.server.host.as_str(),
        port = config.server.port,
        database = config.typedb.database.as_str(),
        "Starting type-bridge-server"
    );

    // Connect to TypeDB
    let client = TypeDBClient::connect(&config.typedb)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    tracing::info!("TypeDB driver connected successfully");

    // Build pipeline
    let mut builder = PipelineBuilder::new(client).with_default_database(&config.typedb.database);

    if !config.schema.source_file.is_empty() {
        builder = builder.with_schema_source(FileSchemaSource::new(&config.schema.source_file));
        tracing::info!(file = config.schema.source_file.as_str(), "Loading schema");
    }

    for name in &config.interceptors.enabled {
        match name.as_str() {
            "audit-log" => {
                let audit_config =
                    config
                        .interceptors
                        .audit_log
                        .clone()
                        .unwrap_or(AuditLogConfig {
                            output: "stdout".to_string(),
                            file_path: String::new(),
                        });
                let interceptor = AuditLogInterceptor::new(&audit_config)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
                builder = builder.with_interceptor(interceptor);
                tracing::info!("Enabled interceptor: audit-log");
            }
            other => {
                tracing::warn!(name = other, "Unknown interceptor, skipping");
            }
        }
    }

    let pipeline = builder
        .build()
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    // Build router and serve
    let router = transport::http::create_router(Arc::new(pipeline));
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .map_err(|e| format!("Invalid listen address: {}", e))?;

    tracing::info!(%addr, "Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}
