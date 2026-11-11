//! Declared-version gRPC connections and database lifecycle on supported releases.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use type_bridge_core_lib::version::Version;
use type_bridge_typedb_runtime::{ConnectOptions, TypeDBRuntime};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_only_requires_an_exact_version_declaration_live() {
    let Ok(address) = std::env::var("TYPEDB_GRPC_DECLARED_ADDRESS") else {
        eprintln!("skipping declared-version gRPC test: TYPEDB_GRPC_DECLARED_ADDRESS is not set");
        return;
    };
    let expected_version: Version = std::env::var("TYPEDB_GRPC_DECLARED_VERSION")
        .expect("live declaration requires the expected server version")
        .parse()
        .expect("expected server version must parse");
    // Hold a local non-HTTP endpoint open so discovery deterministically fails
    // without relying on the host's default TypeDB HTTP port being unused.
    let unavailable_http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = unavailable_http.local_addr().unwrap().port();
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let options = ConnectOptions {
        http_port,
        tls: false,
        server_version: None,
    };
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        TypeDBRuntime::connect(&address, &username, &password, options),
    )
    .await
    .expect("HTTP refusal must finish within 30 seconds")
    .err()
    .expect("unavailable HTTP discovery must refuse");
    assert!(
        error
            .to_string()
            .contains("reachable HTTP version endpoint")
    );
    let runtime = tokio::time::timeout(
        Duration::from_secs(30),
        TypeDBRuntime::connect(
            &address,
            &username,
            &password,
            ConnectOptions {
                http_port,
                tls: false,
                server_version: Some(expected_version),
            },
        ),
    )
    .await
    .expect("declared gRPC connection must finish within 30 seconds")
    .expect("an exact supported declaration must connect");
    assert_eq!(runtime.server_version(), Some(expected_version));
    assert!(runtime.supports_given_rows());

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let prefix = std::env::var("TB_SDK_DATABASE_PREFIX").unwrap_or_else(|_| "tb".into());
    let database = format!("{prefix}_grpc_declared_{}_{}", std::process::id(), suffix);
    assert!(!runtime.database_exists(&database).await.unwrap());
    runtime.create_database(&database).await.unwrap();
    assert!(runtime.database_exists(&database).await.unwrap());
    runtime.delete_database(&database).await.unwrap();
    runtime.force_close().unwrap();
}
