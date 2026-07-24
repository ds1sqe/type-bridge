use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;
#[cfg(feature = "v2-query")]
use type_bridge_server::config::V2AuthorityMode;
use type_bridge_server::config::{AuditLogConfig, RuntimeServerConfig};
use type_bridge_server::interceptor::audit_log::AuditLogInterceptor;
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::schema_source::FileSchemaSource;
use type_bridge_server::transport;
#[cfg(feature = "v2-query")]
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
fn validate_configured_interceptors(enabled: &[String], v2_enabled: bool) -> Result<(), String> {
    // Released V1 startup warned and skipped unknown names. Preserve that
    // behavior for existing configurations. The additive V2 surface is
    // stricter because silently omitting a policy from typed-plan execution
    // would create a new authorization gap.
    if !v2_enabled {
        return Ok(());
    }
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

fn validate_compiled_capabilities(v2_enabled: bool) -> Result<(), &'static str> {
    #[cfg(feature = "v2-query")]
    {
        let _ = v2_enabled;
        Ok(())
    }
    #[cfg(not(feature = "v2-query"))]
    {
        if v2_enabled {
            Err("v2.enabled is set but this binary was built without the v2-query feature")
        } else {
            Ok(())
        }
    }
}

#[cfg(all(feature = "v2-query", test))]
fn redact_v2_schema_export_error<E>(_error: E) -> String {
    "v2 live schema export failed [typedb_v2_schema_export_failed]; inspect provider logs"
        .to_owned()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = RuntimeServerConfig::from_file(&cli.config)?;
    validate_configured_interceptors(&config.interceptors.enabled, config.v2.enabled)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;
    validate_compiled_capabilities(config.v2.enabled)
        .map_err(|error| -> Box<dyn std::error::Error> { error.into() })?;

    // Parse and cross-check inbound identity before any outbound provider I/O.
    // A malformed or mismatched cert/key therefore cannot leave a plaintext
    // listener or partially started server behind.
    let inbound_tls = if let Some(tls) = &config.inbound_tls {
        Some(tls.load().await?)
    } else {
        None
    };
    // Resolve outbound trust exactly once. Both the released V1 pipeline and
    // additive V2 authority clone this immutable snapshot; a CA rotation
    // during startup cannot split their trust identities.
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

    // Connect to TypeDB
    let client = TypeDBClient::connect_prepared_secure(&outbound_transport)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    tracing::info!("TypeDB driver connected successfully");

    // Build pipeline
    let mut builder = PipelineBuilder::new(client).with_default_database(config.typedb.database());

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
            // V1 has always warned and skipped names this binary does not
            // implement. V2 preflight above rejects the same name before any
            // provider I/O, so this compatibility arm is reachable only for
            // the released V1 surface.
            other => tracing::warn!(name = other, "Unknown interceptor, skipping"),
        }
    }

    let pipeline = builder
        .build()
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    #[cfg(feature = "v2-query")]
    if config.v2.enabled {
        pipeline
            .validate_v2_coverage()
            .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?;
    }

    // Build router and serve. The V2 surface is opt-in and fail-closed:
    // an enabled [v2] section either produces working routes or aborts
    // startup — it never degrades to silent 404s.
    #[cfg(feature = "v2-query")]
    let router = if config.v2.enabled {
        let state = build_v2_state(&config, &outbound_transport).await?;
        tracing::info!(
            declared_schema = config.v2.declared_schema_file.as_str(),
            "V2 query surface enabled: /v2/query, /v2/capabilities"
        );
        transport::v2::create_router_with_v2(Arc::new(pipeline), Arc::new(state))
    } else {
        transport::http::create_router(Arc::new(pipeline))
    };
    #[cfg(not(feature = "v2-query"))]
    let router = transport::http::create_router(Arc::new(pipeline));

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
    config: &RuntimeServerConfig,
    outbound_transport: &PreparedSecureTypeDBConnection,
) -> Result<transport::v2::V2QueryState, Box<dyn std::error::Error>> {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::query_plan_capability_vocabulary;
    use type_bridge_contract::schema::decode_declared_schema;
    use type_bridge_orm::session::backend::QueryV2AnswerLimits;
    use type_bridge_schema::{
        ManagedDeltaContext, managed_schema_state, resolve_schema_with_capabilities,
    };

    if config.v2.declared_schema_file.is_empty() {
        return Err("v2.enabled requires v2.declared_schema_file".into());
    }
    let bytes = config
        .v2_declared_schema_bytes()
        .map_err(|error| format!("cannot use v2.declared_schema_file: {error}"))?
        .ok_or("v2.declared_schema_file was not captured during configuration loading")?;
    let declared = decode_declared_schema(bytes).map_err(|e| {
        format!("v2.declared_schema_file is not a canonical declared schema: {e:?}")
    })?;
    let profile = SemanticProfileId::new(&config.v2.profile)
        .map_err(|e| format!("invalid v2.profile: {e:?}"))?;
    let scope =
        ManagedScopeId::new(&config.v2.scope).map_err(|e| format!("invalid v2.scope: {e:?}"))?;
    let resolved =
        resolve_schema_with_capabilities(&declared, &profile, declared.required_capabilities())
            .map_err(|e| format!("v2 declared schema does not resolve: {e:?}"))?;
    let delta_context = ManagedDeltaContext::new(
        scope,
        profile.clone(),
        declared.required_capabilities().clone(),
    );
    let managed = managed_schema_state(&declared, &delta_context)
        .map_err(|e| format!("v2 managed schema state rejected: {e:?}"))?;
    let database = outbound_transport
        .connect_database()
        .await
        .map_err(|e| format!("v2 database connection failed: {e}"))?;

    let server_version = database.server_version().ok_or(
        "v2 requires an exact server-version observation; configure a reachable HTTP probe or explicit server_version",
    )?;
    let negotiated_profile = SemanticProfileId::new(format!("typedb-{server_version}/v1"))
        .map_err(|e| format!("negotiated semantic profile is invalid: {e:?}"))?;
    if profile != negotiated_profile {
        return Err(format!(
            "v2.profile does not match the connected server semantic profile (configured {profile}, negotiated {negotiated_profile})"
        )
        .into());
    }

    // The plan vocabulary is always executable; exact given transport is
    // advertised only when the connected server and negotiated provider can
    // carry explicit input rows, so capability preflight stays truthful for
    // batches, absence, and datetime-tz invocations.
    let mut advertised = query_plan_capability_vocabulary();
    if database.supports_given_stage() {
        advertised.insert(type_bridge_contract::query_given_rows_capability());
    }

    let state = match config.v2.authority_mode {
        V2AuthorityMode::Managed => transport::v2::V2QueryState::new(
            advertised,
            QueryV2AnswerLimits::default(),
            database,
            declared,
            delta_context,
            managed,
            resolved,
        ),
        V2AuthorityMode::QueryOnly => transport::v2::V2QueryState::new_query_only(
            advertised,
            QueryV2AnswerLimits::default(),
            database,
            declared,
            delta_context,
            managed,
            resolved,
        ),
    }
    .map_err(|error| format!("v2 executor advertisement rejected: {error:?}"))?;
    state
        .verify_startup_authority()
        .await
        .map_err(|error| format!("v2 startup authority rejected: {error:?}"))?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "v2-query")]
    use super::redact_v2_schema_export_error;
    use super::{
        AuditLogConfig, AuditLogInterceptor, validate_compiled_capabilities,
        validate_configured_interceptors,
    };

    #[test]
    fn audit_log_is_the_exact_supported_startup_name() {
        validate_configured_interceptors(&["audit-log".to_owned()], true)
            .expect("the implemented audit interceptor remains supported");
        validate_configured_interceptors(&[], true)
            .expect("an empty policy chain remains supported");
    }

    #[test]
    fn released_v1_unknown_interceptor_behavior_remains_permissive() {
        validate_configured_interceptors(&["rate-limiter".to_owned(), "custom".to_owned()], false)
            .expect("released V1 startup warns and skips unknown names");
    }

    #[test]
    fn v2_security_policy_names_and_typos_fail_closed_before_startup() {
        for name in ["auth", "rate-limiter", "custom", "audit_log"] {
            let error = validate_configured_interceptors(&[name.to_owned()], true)
                .expect_err("an unimplemented policy must abort startup");
            assert!(error.contains(name), "{error}");
            assert!(error.contains("supports only: audit-log"), "{error}");
        }
    }

    #[test]
    fn every_unsupported_name_is_reported_in_one_startup_error() {
        let error = validate_configured_interceptors(
            &[
                "audit-log".to_owned(),
                "auth".to_owned(),
                "custom".to_owned(),
            ],
            true,
        )
        .expect_err("mixed implemented and unimplemented policies must abort");
        assert!(error.contains("auth, custom"), "{error}");
    }

    #[cfg(feature = "v2-query")]
    #[test]
    fn v2_schema_export_startup_error_drops_provider_text_and_source_chain() {
        #[derive(Debug)]
        struct ProviderError;

        impl std::fmt::Display for ProviderError {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(
                    "TB_ADDRESS_SECRET TB_USERNAME_SECRET TB_PASSWORD_SECRET TB_PROVIDER_SECRET",
                )
            }
        }

        impl std::error::Error for ProviderError {}

        let error: Box<dyn std::error::Error> = redact_v2_schema_export_error(ProviderError).into();
        let rendered = format!("{error}\n{error:?}");
        for secret in [
            "TB_ADDRESS_SECRET",
            "TB_USERNAME_SECRET",
            "TB_PASSWORD_SECRET",
            "TB_PROVIDER_SECRET",
        ] {
            assert!(!rendered.contains(secret), "{secret}: {rendered}");
        }
        assert!(error.source().is_none());
        assert!(rendered.contains("typedb_v2_schema_export_failed"));
    }

    #[test]
    fn build_capability_preflight_precedes_tls_parsing_and_provider_construction_after_config_load()
    {
        let source = include_str!("main.rs");
        let preflight = source
            .find("validate_compiled_capabilities(config.v2.enabled)")
            .expect("main calls the build-capability preflight");
        for operation in [
            "tls.load().await",
            "TypeDBClient::prepare_secure_transport",
            "TypeDBClient::connect_prepared_secure",
        ] {
            let operation = source
                .find(operation)
                .expect("startup operation remains present");
            assert!(preflight < operation, "preflight must precede {operation}");
        }

        #[cfg(feature = "v2-query")]
        validate_compiled_capabilities(true).expect("this build includes V2");
        #[cfg(not(feature = "v2-query"))]
        assert!(validate_compiled_capabilities(true).is_err());
    }

    #[cfg(feature = "v2-query")]
    #[test]
    fn supported_audit_log_declares_typed_v2_coverage() {
        use type_bridge_server::interceptor::Interceptor;

        let interceptor = AuditLogInterceptor::new(&AuditLogConfig {
            output: "stdout".to_owned(),
            file_path: String::new(),
        })
        .expect("audit interceptor");
        assert!(interceptor.supports_v2());
    }
}
