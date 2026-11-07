use std::env;
use std::io::{self, Write};

use type_bridge_orm::{ConnectOptions, Database, TxType};

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
        .expect("the harness supplies the exact owned database name");
    let mode = env::args().nth(1).unwrap_or_else(|| "setup".to_owned());
    assert!(matches!(mode.as_str(), "setup" | "cleanup"));
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());

    let db = Database::connect_with_options(
        &address,
        &database,
        &username,
        &password,
        connect_options(),
    )
    .await
    .expect("generated live database connects");
    if mode == "cleanup" {
        if db
            .database_exists()
            .await
            .expect("owned database existence is readable")
        {
            db.delete_database().await.expect("owned database deletes");
        }
        assert!(
            !db.database_exists()
                .await
                .expect("owned database absence is readable")
        );
        db.close()
            .expect("cleanup administration connection closes");
        println!("generated Rust isolated database cleanup: passed");
        return;
    }
    assert!(
        !db.database_exists()
            .await
            .expect("initial database absence is readable"),
        "the harness database must initially be absent"
    );
    db.create_database()
        .await
        .expect("generated live database is created");
    println!("generated Rust isolated database created");
    io::stdout()
        .flush()
        .expect("database-created marker flushes");
    db.execute_raw(PROVIDER_SCHEMA, TxType::Schema)
        .await
        .expect("generated provider schema defines");

    let semantic_profile = env::var("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")
        .unwrap_or_else(|_| "typedb-3.13.6/v1".to_owned());
    let exported_schema = db.schema_text().await.expect("provider schema exports");
    for annotation in [
        r#"@doc("cyclic relation player")"#,
        r#"@doc("membership player")"#,
        r#"@doc("employment player")"#,
        r#"@doc("robot membership player")"#,
    ] {
        assert!(
            exported_schema.contains(annotation),
            "exported TypeDB {semantic_profile} schema omitted {annotation}:\n{exported_schema}"
        );
    }

    db.close().expect("generated live database closes");
    println!("generated provider schema setup: passed");
}
