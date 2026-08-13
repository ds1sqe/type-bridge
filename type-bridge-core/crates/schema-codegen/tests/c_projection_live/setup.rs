use std::env;
use std::io::{self, Write};

use type_bridge_orm::{ConnectOptions, Database, TxType};

const PROVIDER_SCHEMA: &str = include_str!("../acceptance/provider-3.12.1.tql");

fn required(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} must be configured and non-empty"))
}

fn connect_options() -> ConnectOptions {
    ConnectOptions {
        http_port: required("TYPEDB_HTTP_PORT")
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .expect("TYPEDB_HTTP_PORT must be an integer from 1 through 65535"),
        tls: false,
        server_version: None,
    }
}

#[tokio::main]
async fn main() {
    let mode = env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("expected setup or cleanup mode"));
    assert!(matches!(mode.as_str(), "setup" | "cleanup"));
    let address = required("TYPEDB_ADDRESS");
    let database_name = required("TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE");
    let username = env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let database = Database::connect_with_options(
        &address,
        &database_name,
        &username,
        &password,
        connect_options(),
    )
    .await
    .expect("the exact C projection administration connection opens");
    let version = database
        .server_version()
        .expect("the server exposes authoritative version evidence");
    assert_eq!(
        (version.major, version.minor, version.patch),
        (3, 12, 1),
        "the generated C entity journey requires exact TypeDB 3.12.1",
    );

    match mode.as_str() {
        "setup" => {
            assert!(
                !database
                    .database_exists()
                    .await
                    .expect("the isolated database absence check succeeds"),
                "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE must name an absent database",
            );
            database
                .create_database()
                .await
                .expect("the isolated generated C database is created");
            println!("generated C isolated database created");
            io::stdout()
                .flush()
                .expect("the database-created marker is flushed");
            database
                .execute_raw(PROVIDER_SCHEMA, TxType::Schema)
                .await
                .expect("the exact provider schema is defined");
            println!("generated C provider schema setup: passed");
        }
        "cleanup" => {
            if database
                .database_exists()
                .await
                .expect("the isolated database cleanup probe succeeds")
            {
                database
                    .delete_database()
                    .await
                    .expect("the isolated generated C database is deleted");
            }
            println!("generated C isolated database cleanup: passed");
        }
        _ => unreachable!(),
    }
    database
        .close()
        .expect("the generated C administration connection closes");
}
