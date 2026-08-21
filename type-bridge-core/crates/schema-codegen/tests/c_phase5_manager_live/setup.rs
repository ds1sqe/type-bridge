use std::env;
use std::io::{self, Write};

use type_bridge_orm::{ConnectOptions, Database, TxType};

const PROVIDER_SCHEMA: &str = include_str!("../workforce-v3/provider.tql");

fn required(name: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} must be configured and non-empty"))
}

fn http_port() -> u16 {
    let value = required("TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT");
    assert!(
        value.bytes().all(|byte| byte.is_ascii_digit()),
        "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT must be an ASCII integer",
    );
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .expect("TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT must be in 1..65535")
}

#[tokio::main]
async fn main() {
    let mode = env::args()
        .nth(1)
        .unwrap_or_else(|| panic!("expected setup or cleanup mode"));
    assert!(matches!(mode.as_str(), "setup" | "cleanup"));
    let database = Database::connect_with_options(
        &required("TYPE_BRIDGE_PHASE5_MANAGER_LIVE_ADDRESS"),
        &required("TYPE_BRIDGE_PHASE5_MANAGER_LIVE_DATABASE"),
        &env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        &env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
        ConnectOptions {
            http_port: http_port(),
            tls: false,
            server_version: None,
        },
    )
    .await
    .expect("C Phase-5 manager administration connection opens");
    let version = database
        .server_version()
        .expect("server exposes authoritative version evidence");
    assert_eq!(
        (version.major, version.minor, version.patch),
        (3, 12, 3),
        "C Phase-5 manager evidence requires exact TypeDB 3.12.3",
    );

    match mode.as_str() {
        "setup" => {
            assert!(
                !database
                    .database_exists()
                    .await
                    .expect("isolated database absence check succeeds"),
                "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_DATABASE must be absent",
            );
            database
                .create_database()
                .await
                .expect("isolated C Phase-5 manager database creates");
            println!("Phase-5 C manager live database created");
            io::stdout()
                .flush()
                .expect("database-created marker flushes");
            database
                .execute_raw(PROVIDER_SCHEMA, TxType::Schema)
                .await
                .expect("exact Workforce V3 provider schema installs");
            println!("Phase-5 C manager live provider setup: passed");
        }
        "cleanup" => {
            if database
                .database_exists()
                .await
                .expect("isolated database cleanup probe succeeds")
            {
                database
                    .delete_database()
                    .await
                    .expect("isolated C Phase-5 manager database deletes");
            }
            println!("Phase-5 C manager live database cleanup: passed");
        }
        _ => unreachable!(),
    }
    database
        .close()
        .expect("C Phase-5 administration connection closes");
}
