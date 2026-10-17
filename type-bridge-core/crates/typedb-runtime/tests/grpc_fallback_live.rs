//! gRPC-only discovery and database lifecycle on supported TypeDB releases.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use type_bridge_core_lib::version::Version;
use type_bridge_typedb_runtime::{ConnectOptions, TypeDBRuntime};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_only_fallback_selects_the_server_band_live() {
    let Ok(address) = std::env::var("TYPEDB_GRPC_FALLBACK_ADDRESS") else {
        eprintln!("skipping gRPC fallback test: TYPEDB_GRPC_FALLBACK_ADDRESS is not set");
        return;
    };
    let expected_version: Version = std::env::var("TYPEDB_GRPC_FALLBACK_VERSION")
        .expect("live fallback requires the expected server version")
        .parse()
        .expect("expected server version must parse");
    // Hold a local non-HTTP endpoint open so discovery deterministically fails
    // without relying on the host's default TypeDB HTTP port being unused.
    let unavailable_http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let http_port = unavailable_http.local_addr().unwrap().port();
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let runtime = tokio::time::timeout(
        Duration::from_secs(30),
        TypeDBRuntime::connect(
            &address,
            &username,
            &password,
            ConnectOptions {
                http_port,
                tls: false,
                server_version: None,
            },
        ),
    )
    .await
    .expect("gRPC fallback must finish within 30 seconds")
    .expect("gRPC discovery must reconnect on the accepted band");
    assert_eq!(runtime.server_version(), Some(expected_version));
    assert_eq!(runtime.supports_given_rows(), expected_version.minor >= 12);

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let database = format!("tb_grpc_fallback_{}_{}", std::process::id(), suffix);
    assert!(!runtime.database_exists(&database).await.unwrap());
    runtime.create_database(&database).await.unwrap();
    assert!(runtime.database_exists(&database).await.unwrap());
    runtime.delete_database(&database).await.unwrap();
    runtime.force_close().unwrap();
}
