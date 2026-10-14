//! Identity shared by the sdk-v4 live producers.
#![allow(dead_code)]

#[path = "../../../../../tests/support/sdk_profile_matrix.rs"]
pub mod sdk_profile_matrix;

use type_bridge_typedb_runtime::{ConnectOptions, TypeDBRuntime};

/// The live server an sdk-v4 report on the requested profile declares.
///
/// Connects to the configured TypeDB and fails unless the detected server is
/// exactly the sdk-v4 matrix entry's live server.
pub async fn requested_v4_server_version() -> String {
    let profile = sdk_profile_matrix::requested(4);
    let runtime = TypeDBRuntime::connect(
        &std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "127.0.0.1:1729".to_owned()),
        &std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        &std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
        ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".to_owned())
                .parse()
                .expect("HTTP port is valid"),
            ..ConnectOptions::default()
        },
    )
    .await
    .expect("sdk-v4 server-version probe connects");
    let detected = runtime.server_version().map(|version| version.to_string());
    profile.require_server_version(detected.as_deref());
    profile.live_server_version
}

/// [`requested_v4_server_version`] for a synchronous test.
pub fn requested_v4_server_version_blocking() -> String {
    tokio::runtime::Runtime::new()
        .expect("sdk-v4 server-version probe runtime builds")
        .block_on(requested_v4_server_version())
}
