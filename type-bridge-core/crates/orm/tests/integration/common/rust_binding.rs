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
