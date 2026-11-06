#![allow(dead_code)]

use std::env;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_orm::Database;

use super::typedb::{connect_options_from_env, ensure_database_exists};

static NEXT_LABEL_ID: AtomicU64 = AtomicU64::new(1);

pub async fn setup_db() -> Database {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".to_string());
    let database = env::var("TYPE_BRIDGE_RUST_BINDING_INTG_DATABASE")
        .unwrap_or_else(|_| "type_bridge_test".into());
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_string());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_string());

    ensure_database_exists(
        &address,
        &database,
        &username,
        &password,
        "Rust ORM integration",
    )
    .await;

    Database::connect_with_options(
        &address,
        &database,
        &username,
        &password,
        connect_options_from_env(),
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "Rust ORM integration requires TypeDB at {address} \
                 database {database}: {error}"
        )
    })
}

/// True when the live server proved a TypeDB 3.12+ identity at connect time.
///
/// The V2 conformance probes (native `given` transport, prepared semantic
/// profiles, `@doc`/`@meta` on subtypes and functions) are defined against the
/// TypeDB 3.12.1 semantic baseline. Custom backends must prove a compatible
/// server identity before the conformance probes execute.
pub fn server_supports_conformance(db: &Database) -> bool {
    db.server_version()
        .is_some_and(|version| version.major > 3 || (version.major == 3 && version.minor >= 12))
}

pub fn unique_label(prefix: &str) -> String {
    let id = NEXT_LABEL_ID.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{}-{id}", process::id())
}
