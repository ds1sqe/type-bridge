use std::env;

use type_bridge_orm::{ConnectOptions, Database, TxType, ensure_database_exists};

const PROVIDER_SCHEMA: &str = include_str!("provider.tql");

fn connect_options() -> ConnectOptions {
    let mut options = ConnectOptions::default();
    options.http_port = env::var("TYPEDB_HTTP_PORT")
        .unwrap_or_else(|_| "8000".to_owned())
        .parse()
        .expect("TYPEDB_HTTP_PORT is a valid nonzero u16");
    options.tls = env::var("TYPE_BRIDGE_RUST_PROJECTION_TLS").as_deref() == Ok("1");
    options
}

#[tokio::main]
async fn main() {
    let address = env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1729".to_owned());
    let database = env::var("TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE")
        .unwrap_or_else(|_| format!("type_bridge_rust_projection_live_{}", std::process::id()));
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());

    ensure_database_exists(&address, &database, &username, &password, connect_options())
        .await
        .expect("generated live database is created");
    let db = Database::connect_with_options(
        &address,
        &database,
        &username,
        &password,
        connect_options(),
    )
    .await
    .expect("generated live database connects");
    db.execute_raw(PROVIDER_SCHEMA, TxType::Schema)
        .await
        .expect("generated provider schema defines");

    let semantic_profile = env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
        .unwrap_or_else(|_| "typedb-3.12.1/v1".to_owned());
    if semantic_profile == "typedb-3.12.1/v1" {
        let exported_schema = db.schema_text().await.expect("provider schema exports");
        for annotation in [
            r#"@doc("cyclic relation player")"#,
            r#"@doc("membership player")"#,
            r#"@doc("employment player")"#,
            r#"@doc("robot membership player")"#,
        ] {
            assert!(
                exported_schema.contains(annotation),
                "exported TypeDB 3.12 schema omitted {annotation}:\n{exported_schema}"
            );
        }
    }

    db.close().expect("generated live database closes");
    println!("generated provider schema setup: passed");
}
