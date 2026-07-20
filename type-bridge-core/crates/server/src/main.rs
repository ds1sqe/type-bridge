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

    // Build router and serve. The V2 surface is opt-in and fail-closed:
    // an enabled [v2] section either produces working routes or aborts
    // startup — it never degrades to silent 404s.
    #[cfg(feature = "v2-query")]
    let router = if config.v2.enabled {
        let state = build_v2_state(&config).await?;
        tracing::info!(
            declared_schema = config.v2.declared_schema_file.as_str(),
            "V2 query surface enabled: /v2/query, /v2/capabilities"
        );
        transport::v2::create_router_with_v2(Arc::new(pipeline), Arc::new(state))
    } else {
        transport::http::create_router(Arc::new(pipeline))
    };
    #[cfg(not(feature = "v2-query"))]
    let router = {
        if config.v2.enabled {
            return Err(
                "v2.enabled is set but this binary was built without the v2-query feature".into(),
            );
        }
        transport::http::create_router(Arc::new(pipeline))
    };

    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .map_err(|e| format!("Invalid listen address: {}", e))?;

    tracing::info!(%addr, "Server listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;

    Ok(())
}

/// Construct the V2 executor state from the validated `[v2]` config section.
///
/// Every failure is a startup error: missing schema file, undecodable
/// declared schema, unresolvable profile/scope, or an unreachable database
/// all abort the server rather than serving a partial surface. Schema
/// resolution runs under the schema's own required capability set — the
/// operator supplied the artifact, so its declared requirements are the
/// authority; request-level gating uses the advertised query vocabulary.
#[cfg(feature = "v2-query")]
async fn build_v2_state(
    config: &ServerConfig,
) -> Result<transport::v2::V2QueryState, Box<dyn std::error::Error>> {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::query_plan_capability_vocabulary;
    use type_bridge_contract::schema::decode_declared_schema;
    use type_bridge_orm::session::backend::BoundedAnswerLimits;
    use type_bridge_orm::session::database::Database;
    use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};

    if config.v2.declared_schema_file.is_empty() {
        return Err("v2.enabled requires v2.declared_schema_file".into());
    }
    let bytes = std::fs::read(&config.v2.declared_schema_file)
        .map_err(|e| format!("cannot read v2.declared_schema_file: {e}"))?;
    let declared = decode_declared_schema(&bytes).map_err(|e| {
        format!("v2.declared_schema_file is not a canonical declared schema: {e:?}")
    })?;
    let profile = SemanticProfileId::new(&config.v2.profile)
        .map_err(|e| format!("invalid v2.profile: {e:?}"))?;
    let scope =
        ManagedScopeId::new(&config.v2.scope).map_err(|e| format!("invalid v2.scope: {e:?}"))?;
    let resolved = resolve(&declared, &profile)
        .map_err(|e| format!("v2 declared schema does not resolve: {e:?}"))?;
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(scope, profile, declared.required_capabilities().clone()),
    )
    .map_err(|e| format!("v2 managed schema state rejected: {e:?}"))?;
    let database = Database::connect(
        &config.typedb.address,
        &config.typedb.database,
        &config.typedb.username,
        &config.typedb.password,
    )
    .await
    .map_err(|e| format!("v2 database connection failed: {e}"))?;

    Ok(transport::v2::V2QueryState {
        advertised: query_plan_capability_vocabulary(),
        ceilings: BoundedAnswerLimits::default(),
        database,
        managed,
        resolved,
    })
}
