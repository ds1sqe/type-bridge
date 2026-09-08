use std::env;
use std::io::{self, Write};

use type_bridge_orm::{ConnectOptions, Database, TxType};

const PROVIDER_SCHEMA: &str = include_str!("../provider.tql");

fn required(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} must be configured and non-empty"))
}

fn setting(suffix: &str) -> String {
    let prefix = env::var("TYPE_BRIDGE_TEST_ENV_PREFIX").expect("test environment prefix is set");
    required(&format!("{prefix}_{suffix}"))
}

fn http_port() -> u16 {
    let value = setting("HTTP_PORT");
    assert!(
        value.bytes().all(|byte| byte.is_ascii_digit()),
        "TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT must be an ASCII integer",
    );
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .expect("TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT must be in 1..65535")
}

#[tokio::main]
async fn main() {
    let mode = env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("expected setup or cleanup mode"));
    assert!(matches!(mode.as_str(), "setup" | "cleanup"));
    let database = Database::connect_with_options(
        &setting("ADDRESS"),
        &setting("DATABASE"),
        &env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        &env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
        ConnectOptions {
            http_port: http_port(),
            tls: false,
            server_version: None,
        },
    )
    .await
    .expect("the C test administration connection opens");
    let version = database
        .server_version()
        .expect("the server exposes authoritative version evidence");
    assert_eq!(
        (version.major, version.minor, version.patch),
        (3, 12, 3),
        "the C test subset requires exact TypeDB 3.12.3",
    );

    match mode.as_str() {
        "setup" => {
            assert!(
                !database
                    .database_exists()
                    .await
                    .expect("the isolated database absence check succeeds"),
                "TYPE_BRIDGE_PROJECTED_LIVE_DATABASE must name an absent database",
            );
            database
                .create_database()
                .await
                .expect("the isolated C test database creates");
            println!("C test database created");
            io::stdout()
                .flush()
                .expect("the database-created marker flushes");
            database
                .execute_raw(PROVIDER_SCHEMA, TxType::Schema)
                .await
                .expect("the exact Sdk V3 provider schema installs");
            println!("C test provider schema setup: passed");
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
                    .expect("the isolated C test database deletes");
            }
            println!("Projected C live database cleanup: passed");
        }
        _ => unreachable!(),
    }
    database
        .close()
        .expect("the C Projected administration connection closes");
}
