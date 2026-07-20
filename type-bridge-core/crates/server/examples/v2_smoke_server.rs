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

use std::sync::Arc;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan_capability_vocabulary;
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_orm::session::backend::BoundedAnswerLimits;
use type_bridge_orm::session::database::Database;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state, resolve};
use type_bridge_server::transport::v2::{V2QueryState, create_v2_router};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
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
    let declared_bytes = decode_b64(&env("SMOKE_DECLARED_B64"));
    let declared = decode_declared_schema(&declared_bytes).expect("declared schema decodes");
    let profile = SemanticProfileId::new(env("SMOKE_PROFILE")).expect("profile");
    let resolved = resolve(&declared, &profile).expect("schema resolves");
    let managed = managed_schema_state(
        &declared,
        &ManagedDeltaContext::new(
            ManagedScopeId::new(env("SMOKE_SCOPE")).expect("scope"),
            profile,
            CapabilitySet::new(),
        ),
    )
    .expect("managed state");
    let database = Database::connect(
        &env("SMOKE_TYPEDB_ADDRESS"),
        &env("SMOKE_DATABASE"),
        &env("SMOKE_TYPEDB_USERNAME"),
        &env("SMOKE_TYPEDB_PASSWORD"),
    )
    .await
    .expect("database connects");

    let state = Arc::new(V2QueryState {
        advertised: query_plan_capability_vocabulary(),
        ceilings: BoundedAnswerLimits::default(),
        database,
        managed,
        resolved,
    });
    let router = create_v2_router(state);
    let listener = tokio::net::TcpListener::bind((
        "127.0.0.1",
        env("SMOKE_PORT").parse::<u16>().expect("port"),
    ))
    .await
    .expect("listener binds");
    println!("v2-smoke-server-ready");
    axum::serve(listener, router).await.expect("server runs");
}
