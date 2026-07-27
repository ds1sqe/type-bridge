//! Downstream-style compile checks for the additive typed TLS ORM surface.

use type_bridge_orm::{
    ConnectOptions, Database, SecureConnectError, SecureConnectOptions, SecureResult, TlsMode,
};

fn accepts_secure_error(result: SecureResult<()>) -> Option<SecureConnectError> {
    result.err()
}

#[test]
fn released_boolean_options_adapt_without_changing_the_public_struct() {
    let released = ConnectOptions {
        http_port: 8443,
        tls: true,
        server_version: None,
    };
    let copied = released;
    let secure = SecureConnectOptions::from(released);
    let ConnectOptions {
        http_port,
        tls,
        server_version,
    } = copied;

    assert_eq!(http_port, 8443);
    assert!(tls);
    assert_eq!(server_version, None);
    assert_eq!(secure.tls_mode, TlsMode::NativeRoots);
    SecureConnectOptions::default()
        .validate_transport()
        .expect("plaintext policy preflights without constructing a host");
    assert!(accepts_secure_error(Ok(())).is_none());

    // Constructing these futures performs no I/O, but type-checks the exact
    // released connect entry points and their `ConnectOptions` parameter.
    drop(Database::connect(
        "localhost:1729",
        "compat",
        "admin",
        "password",
    ));
    drop(Database::connect_with_options(
        "localhost:1729",
        "compat",
        "admin",
        "password",
        ConnectOptions::default(),
    ));
}
