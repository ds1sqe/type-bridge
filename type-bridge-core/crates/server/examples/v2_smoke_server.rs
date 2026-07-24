//! Minimal V2 envelope server for cross-binding smoke tests.
//!
//! Reads its whole configuration from the environment, builds one
//! [`V2QueryState`] from canonical declared-schema bytes, and serves the
//! versioned V2 routes until killed. Requires `--features v2-query`.
//!
//! Environment:
//! - `SMOKE_TYPEDB_ADDRESS` (e.g. `localhost:1729`)
//! - `SMOKE_TYPEDB_USERNAME` / `SMOKE_TYPEDB_PASSWORD`
//! - `SMOKE_DATABASE` — existing database name
//! - `SMOKE_DECLARED_B64` — base64 canonical declared-schema bytes
//! - `SMOKE_SCOPE` / `SMOKE_PROFILE` — managed scope and semantic profile
//! - `SMOKE_PORT` — listen port on 127.0.0.1
//! - `SMOKE_TYPEDB_HTTP_PORT` — TypeDB HTTP discovery port (default 8000)
//! - `SMOKE_TYPEDB_TLS` — optional exact `true` / `false` transport switch
//! - `SMOKE_TYPEDB_TLS_ROOT_CA` — optional custom CA; requires TLS `true`
//! - `SMOKE_TLS_CERT` / `SMOKE_TLS_KEY` — optional inbound HTTPS identity;
//!   both must be present

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan_capability_vocabulary;
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_orm::session::backend::QueryV2AnswerLimits;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};
use type_bridge_server::config::{
    InboundTlsSection, OutboundTlsMode, SecureTypeDBSection, TypeDBSection,
};
use type_bridge_server::pipeline::PipelineBuilder;
use type_bridge_server::transport::v2::{V2QueryState, create_v2_router};
use type_bridge_server::typedb::TypeDBClient;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn canonical_env_path(name: &str, path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path)
        .unwrap_or_else(|error| panic!("{name} must name a resolvable physical file: {error}"))
}

fn typedb_tls_mode() -> OutboundTlsMode {
    let enabled = std::env::var("SMOKE_TYPEDB_TLS").ok();
    let root = std::env::var_os("SMOKE_TYPEDB_TLS_ROOT_CA").map(PathBuf::from);
    typedb_tls_mode_from(enabled.as_deref(), root).unwrap_or_else(|error| panic!("{error}"))
}

fn typedb_tls_mode_from(
    enabled: Option<&str>,
    root: Option<PathBuf>,
) -> Result<OutboundTlsMode, String> {
    let mode = match (enabled, root) {
        (None | Some("false"), None) => OutboundTlsMode::Disabled,
        (Some("true"), None) => OutboundTlsMode::NativeRoots,
        (Some("true"), Some(path)) => {
            OutboundTlsMode::CustomRootCa(std::fs::canonicalize(path).map_err(|error| {
                format!("SMOKE_TYPEDB_TLS_ROOT_CA must name a resolvable physical file: {error}")
            })?)
        }
        (None, Some(_)) => {
            return Err("SMOKE_TYPEDB_TLS_ROOT_CA requires SMOKE_TYPEDB_TLS=true".to_owned());
        }
        (Some("false"), Some(_)) => {
            return Err("SMOKE_TYPEDB_TLS_ROOT_CA contradicts SMOKE_TYPEDB_TLS=false".to_owned());
        }
        (Some(other), _) => {
            return Err(format!(
                "SMOKE_TYPEDB_TLS must be true or false, got {other:?}"
            ));
        }
    };
    Ok(mode)
}

async fn inbound_tls() -> Option<axum_server::tls_rustls::RustlsConfig> {
    let cert = std::env::var_os("SMOKE_TLS_CERT").map(PathBuf::from);
    let key = std::env::var_os("SMOKE_TLS_KEY").map(PathBuf::from);
    match (cert, key) {
        (None, None) => None,
        (Some(cert_path), Some(key_path)) => Some(
            InboundTlsSection::from_paths(
                canonical_env_path("SMOKE_TLS_CERT", cert_path),
                canonical_env_path("SMOKE_TLS_KEY", key_path),
            )
            .load()
            .await
            .expect("SMOKE_TLS_CERT/SMOKE_TLS_KEY form a valid bounded identity"),
        ),
        _ => panic!("SMOKE_TLS_CERT and SMOKE_TLS_KEY must be supplied together"),
    }
}

fn decode_b64(text: &str) -> Vec<u8> {
    const TABLE: &[i8] = &{
        let mut table = [-1i8; 256];
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut index = 0;
        while index < alphabet.len() {
            table[alphabet[index] as usize] = index as i8;
            index += 1;
        }
        table
    };
    let mut bytes = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let value = TABLE[byte as usize];
        assert!(value >= 0, "invalid base64 input");
        buffer = (buffer << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    bytes
}

#[tokio::main]
async fn main() {
    // Load and cross-check the inbound identity before any provider I/O.
    let inbound_tls = inbound_tls().await;
    // Resolve caller path aliases before reading connection credentials. The
    // validated server types below receive only physical paths.
    let tls_mode = typedb_tls_mode();
    let address = env("SMOKE_TYPEDB_ADDRESS");
    let database_name = env("SMOKE_DATABASE");
    let username = env("SMOKE_TYPEDB_USERNAME");
    let password = env("SMOKE_TYPEDB_PASSWORD");
    let http_port = std::env::var("SMOKE_TYPEDB_HTTP_PORT")
        .unwrap_or_else(|_| "8000".to_owned())
        .parse::<u16>()
        .expect("SMOKE_TYPEDB_HTTP_PORT is a u16");
    let secure_config = SecureTypeDBSection::new(
        TypeDBSection {
            address: address.clone(),
            database: database_name.clone(),
            username: username.clone(),
            password: password.clone(),
            http_port,
            server_version: None,
        },
        tls_mode,
    );
    let prepared_connection = TypeDBClient::prepare_secure_transport(&secure_config)
        .expect("outbound transport policy is valid");
    let declared_bytes = decode_b64(&env("SMOKE_DECLARED_B64"));
    let declared = decode_declared_schema(&declared_bytes).expect("declared schema decodes");
    let profile = SemanticProfileId::new(env("SMOKE_PROFILE")).expect("profile");
    let resolved = resolve(&declared, &profile).expect("schema resolves");
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new(env("SMOKE_SCOPE")).expect("scope"),
        profile,
        declared.required_capabilities().clone(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed state");
    let database = prepared_connection
        .connect_database()
        .await
        .expect("database connects");

    let mut advertised = query_plan_capability_vocabulary();
    if database.supports_given_stage() {
        advertised.insert(type_bridge_contract::query_given_rows_capability());
    }
    let state = Arc::new(
        V2QueryState::new_query_only(
            advertised,
            QueryV2AnswerLimits::default(),
            database,
            declared,
            delta_context,
            managed,
            resolved,
        )
        .expect("executor advertisement is canonical"),
    );
    let policy_client = TypeDBClient::connect_prepared_secure(&prepared_connection)
        .await
        .expect("policy pipeline connects");
    let pipeline = Arc::new(
        PipelineBuilder::new(policy_client)
            .with_default_database(database_name)
            .build()
            .expect("policy pipeline builds"),
    );
    let router = create_v2_router(pipeline, state);
    let address = SocketAddr::from((
        [127, 0, 0, 1],
        env("SMOKE_PORT").parse::<u16>().expect("port"),
    ));
    println!("v2-smoke-server-ready");
    if let Some(tls) = inbound_tls {
        axum_server::bind_rustls(address, tls)
            .serve(router.into_make_service())
            .await
            .expect("HTTPS server runs");
    } else {
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .expect("listener binds");
        axum::serve(listener, router).await.expect("server runs");
    }
}

#[cfg(test)]
mod tests {
    use super::typedb_tls_mode_from;
    use std::path::PathBuf;

    #[test]
    fn tls_contradictions_precede_custom_root_path_io() {
        let missing = PathBuf::from("path-that-must-not-be-resolved.pem");
        let omitted = typedb_tls_mode_from(None, Some(missing.clone())).unwrap_err();
        assert!(omitted.contains("requires SMOKE_TYPEDB_TLS=true"));
        assert!(!omitted.contains("resolvable physical file"));

        let disabled = typedb_tls_mode_from(Some("false"), Some(missing.clone())).unwrap_err();
        assert!(disabled.contains("contradicts SMOKE_TYPEDB_TLS=false"));
        assert!(!disabled.contains("resolvable physical file"));

        let enabled = typedb_tls_mode_from(Some("true"), Some(missing)).unwrap_err();
        assert!(enabled.contains("resolvable physical file"));
    }
}
