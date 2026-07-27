#![allow(dead_code)]

use std::env;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_orm::*;

use super::typedb::{connect_options_from_env, ensure_database_exists};

type_bridge_orm::include_schema!("tests/test_schema.tql");

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
/// TypeDB 3.12.1 baseline. On the legacy 3.8/3.10/3.11 lanes production
/// rejects those capabilities before I/O instead of emulating them, so the
/// conformance probes skip rather than assert 3.12 behavior.
pub fn server_supports_v2_conformance(db: &Database) -> bool {
    db.server_version()
        .is_some_and(|version| version.major > 3 || (version.major == 3 && version.minor >= 12))
}

pub async fn sync_person_schema(db: &Database) {
    let mut schema = SchemaManager::new(db);
    schema.register_entity::<Person>();
    schema
        .sync_schema(true, false)
        .await
        .expect("person schema sync failed");
}

pub async fn sync_full_schema(db: &Database) {
    let mut schema = SchemaManager::new(db);
    schema.register_entity::<Person>();
    schema.register_entity::<Company>();
    schema.register_relation::<Employment>();
    schema
        .sync_schema(true, false)
        .await
        .expect("schema sync failed");
}

pub fn unique_label(prefix: &str) -> String {
    let id = NEXT_LABEL_ID.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}-{}-{id}", process::id())
}
