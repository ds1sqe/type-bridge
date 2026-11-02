use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use type_bridge_server::config::StagedAuthorityMode;
use type_bridge_server::config::{AuditLogConfig, RuntimeServerConfig};
use type_bridge_server::interceptor::audit_log::AuditLogInterceptor;
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::transport;

use type_bridge_server::typedb::PreparedSecureTypeDBConnection;
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

const SUPPORTED_INTERCEPTORS: &[&str] = &["audit-log"];

/// Reject configured policy names this binary cannot actually construct.
///
/// This check runs immediately after parsing configuration, before TLS files,
/// provider connections, routers, or listeners exist. An operator request for
/// authentication or rate limiting must never degrade into an unprotected
/// server because a name was misspelled or the implementation is absent.
fn validate_configured_interceptors(enabled: &[String]) -> Result<(), String> {
    let unsupported = enabled
        .iter()
        .filter(|name| !SUPPORTED_INTERCEPTORS.contains(&name.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if unsupported.is_empty() {
        return Ok(());
    }
    Err(format!(
        "unsupported configured interceptor(s): {}; this binary supports only: {}",
        unsupported.join(", "),
        SUPPORTED_INTERCEPTORS.join(", "),
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = RuntimeServerConfig::from_file(&cli.config)?;
    if config.staged.raw_enabled && !config.staged.enabled {
        return Err("staged.raw_enabled requires staged.enabled and its schema authority".into());
    }
    validate_configured_interceptors(&config.interceptors.enabled)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    // Parse and cross-check inbound identity before any outbound provider I/O.
    // A malformed or mismatched cert/key therefore cannot leave a plaintext
    // listener or partially started server behind.
    let inbound_tls = if let Some(tls) = &config.inbound_tls {
        Some(tls.load().await?)
    } else {
        None
    };
    // Capture outbound trust once for the configured executor.
    let outbound_transport = TypeDBClient::prepare_secure_transport(&config.typedb)
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

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
        database = config.typedb.database(),
        "Starting type-bridge-server"
    );

    let mut builder = PipelineBuilder::new().with_default_database(config.typedb.database());

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
            other => return Err(format!("unsupported configured interceptor: {other}").into()),
        }
    }

    let pipeline = builder
        .build()
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    if config.staged.enabled {
        pipeline.validate_v3_coverage()?;
    }

    let router = if config.staged.enabled {
        let state = build_staged_state(&config, &outbound_transport).await?;
        tracing::info!(
            schema_authority = config.staged.schema_authority_file.as_str(),
            "Staged query surface enabled: /v3/staged, /v3/capabilities"
        );
        transport::v3::create_v3_router(Arc::new(pipeline), Arc::new(state))
    } else {
        transport::http::create_health_router(false, Arc::new(pipeline))
    };

    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .map_err(|e| format!("Invalid listen address: {}", e))?;

    tracing::info!(%addr, tls = inbound_tls.is_some(), "Server listening");

    if let Some(tls) = inbound_tls {
        axum_server::bind_rustls(addr, tls)
            .serve(router.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;
    }

    Ok(())
}

/// Construct the staged executor state from the validated `[staged]` config section.
///
/// Every failure is a startup error: missing authority, an unverifiable
/// source-free envelope, a profile mismatch, or an unreachable database all
/// abort the server rather than serving a partial surface. Scope, profile,
/// declared schema, resolved semantics, and managed state come from one
/// constructor-verified artifact; deployment configuration cannot repeat or
/// override them.
async fn build_staged_state(
    config: &RuntimeServerConfig,
    outbound_transport: &PreparedSecureTypeDBConnection,
) -> Result<transport::staged_state::StagedQueryState, Box<dyn std::error::Error>> {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary;
    use type_bridge_orm::session::backend::AnswerLimits;
    use type_bridge_schema::{
        ManagedDeltaContext, decode_schema_authority, schema_authority_capability_vocabulary,
    };

    if config.staged.schema_authority_file.is_empty() {
        return Err("staged.enabled requires staged.schema_authority_file".into());
    }
    let bytes = config
        .staged_schema_authority_bytes()
        .map_err(|error| format!("cannot use staged.schema_authority_file: {error}"))?
        .ok_or("staged.schema_authority_file was not captured during configuration loading")?;

    // The generic server and every generated package reconstruct authority
    // through the same closed vocabulary. Operation surfaces continue to
    // advertise their narrower executable capability sets independently.
    let available_capabilities = schema_authority_capability_vocabulary();
    let authority = decode_schema_authority(bytes, &available_capabilities).map_err(|error| {
        format!(
            "staged.schema_authority_file is not a verified schema authority ({:?})",
            error.code()
        )
    })?;
    let declared = authority.declared_schema().clone();
    let profile = authority.semantic_profile().id().clone();
    let managed = authority.managed_state().clone();
    let delta_context = ManagedDeltaContext::new(
        authority.managed_scope().id().clone(),
        profile.clone(),
        authority.required_capabilities().clone(),
    );
    let database = outbound_transport
        .connect_database()
        .await
        .map_err(|e| format!("staged database connection failed: {e}"))?;

    let server_version = database.server_version().ok_or(
        "staged requires an exact server-version observation; configure a reachable HTTP probe or explicit server_version",
    )?;
    let negotiated_profile = SemanticProfileId::new(
        type_bridge_core_lib::version::semantic_profile_id(&server_version).ok_or_else(|| {
            format!("connected TypeDB {server_version} has no supported semantic profile")
        })?,
    )
    .map_err(|e| format!("negotiated semantic profile is invalid: {e:?}"))?;
    if profile != negotiated_profile {
        return Err(format!(
            "schema-authority profile does not match the connected server semantic profile (artifact {profile}, negotiated {negotiated_profile})"
        )
        .into());
    }

    // Advertise only the native staged plan vocabulary.
    let mut advertised = query_plan_v3_capability_vocabulary();
    if database.supports_given_stage() {
        advertised.insert(type_bridge_contract::query_given_rows_capability());
    }

    let state = match config.staged.authority_mode {
        StagedAuthorityMode::Managed => transport::staged_state::StagedQueryState::new(
            advertised,
            AnswerLimits::default(),
            database,
            declared,
            delta_context,
            managed,
        ),
        StagedAuthorityMode::QueryOnly => {
            transport::staged_state::StagedQueryState::new_query_only(
                advertised,
                AnswerLimits::default(),
                database,
                declared,
                delta_context,
                managed,
            )
        }
    }
    .map_err(|error| format!("staged executor advertisement rejected: {error:?}"))?
    .with_query_limits(config.staged.resolved_query_limits()?)
    .map_err(|error| format!("staged query limits rejected: {error:?}"))?;
    let state = state
        .with_staged_query_limits(config.staged.resolved_staged_query_limits()?)
        .map_err(|error| format!("staged query limits rejected: {error:?}"))?
        .with_answer_limits(config.staged.resolved_answer_limits()?)
        .map_err(|error| format!("staged answer limits rejected: {error:?}"))?
        .with_admission_window(std::time::Duration::from_millis(
            config.staged.admission_window_milliseconds.unwrap_or(
                type_bridge_contract::query_remote::DEFAULT_ADMISSION_WINDOW_MILLISECONDS,
            ),
        ))
        .map_err(|error| format!("request admission window rejected: {error:?}"))?
        .with_unauthenticated_staged_writes(config.staged.allow_unauthenticated_staged_writes);
    let state = if config.staged.raw_enabled {
        state.with_raw_executor(outbound_transport.connect_raw_runtime().await?)
    } else {
        state
    };
    state
        .verify_startup_authority()
        .await
        .map_err(|error| format!("staged startup authority rejected: {error:?}"))?;
    Ok(state)
}

#[cfg(test)]
mod tests {

    use super::{AuditLogConfig, AuditLogInterceptor, validate_configured_interceptors};

    #[test]
    fn audit_log_is_the_exact_supported_startup_name() {
        validate_configured_interceptors(&["audit-log".to_owned()])
            .expect("the implemented audit interceptor remains supported");
        validate_configured_interceptors(&[]).expect("an empty policy chain remains supported");
    }

    #[test]
    fn staged_security_policy_names_and_typos_fail_closed_before_startup() {
        for name in ["auth", "rate-limiter", "custom", "audit_log"] {
            let error = validate_configured_interceptors(&[name.to_owned()])
                .expect_err("an unimplemented policy must abort startup");
            assert!(error.contains(name), "{error}");
            assert!(error.contains("supports only: audit-log"), "{error}");
        }
    }

    #[test]
    fn every_unsupported_name_is_reported_in_one_startup_error() {
        let error = validate_configured_interceptors(&[
            "audit-log".to_owned(),
            "auth".to_owned(),
            "custom".to_owned(),
        ])
        .expect_err("mixed implemented and unimplemented policies must abort");
        assert!(error.contains("auth, custom"), "{error}");
    }

    #[test]
    fn build_capability_preflight_precedes_tls_parsing_and_provider_construction_after_config_load()
    {
        let source = include_str!("main.rs");
        let preflight = source
            .find("validate_configured_interceptors(&config.interceptors.enabled)")
            .expect("main calls the build-capability preflight");
        for operation in [
            "tls.load().await",
            "TypeDBClient::prepare_secure_transport",
            "build_staged_state(&config,",
        ] {
            let operation = source
                .find(operation)
                .expect("startup operation remains present");
            assert!(preflight < operation, "preflight must precede {operation}");
        }
    }

    #[test]
    fn supported_audit_log_declares_staged_coverage() {
        use type_bridge_server::interceptor::Interceptor;

        let interceptor = AuditLogInterceptor::new(&AuditLogConfig {
            output: "stdout".to_owned(),
            file_path: String::new(),
        })
        .expect("audit interceptor");
        assert!(interceptor.supports_v3());
    }
}
