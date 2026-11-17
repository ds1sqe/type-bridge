//! Downstream-style compile checks for the additive typed TLS ORM surface.

use type_bridge_orm::{
    ConnectOptions, Database, SecureConnectError, SecureConnectOptions, SecureResult, TlsMode,
};

fn accepts_secure_error(result: SecureResult<()>) -> Option<SecureConnectError> {
    result.err()
}

#[test]
fn boolean_options_adapt_with_extensible_public_struct() {
    let mut released = ConnectOptions::default();
    released.http_port = 8443;
    released.tls = true;
    let copied = released;
    let secure = SecureConnectOptions::from(released);
    let ConnectOptions {
        http_port,
        tls,
        server_version,
        ..
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

#[test]
fn secure_connect_diagnostic_mapping_is_hidden_and_redacts_private_causes() {
    use type_bridge_orm::lower_secure_connect_error;
    const SECRET: &str = "password=tb270-secret /private/tb270-secret-key.pem";
    let error = SecureConnectError::Runtime(type_bridge_typedb_runtime::RuntimeError::Connection(
        SECRET.into(),
    ));
    let diagnostic = lower_secure_connect_error(error);
    let visible = serde_json::to_string(&diagnostic.host_json()).unwrap();
    assert_eq!(diagnostic.code().as_str(), "provider_operation_failed");
    assert!(!visible.contains("tb270-secret"));
    assert!(!visible.contains("/private/"));

    let missing = std::env::temp_dir().join(format!("tb270-secret-{}-key.pem", std::process::id()));
    assert!(!missing.exists());
    let mut options = SecureConnectOptions::default();
    options.tls_mode = TlsMode::CustomRootCa(missing.clone());
    let tls_error = options.prepare_transport().unwrap_err();
    let diagnostic = lower_secure_connect_error(tls_error);
    let visible = serde_json::to_string(&diagnostic.host_json()).unwrap();
    assert_eq!(diagnostic.code().as_str(), "tls_custom_root_ca_unreadable");
    assert!(!visible.contains("tb270-secret"));
    assert!(!visible.contains(missing.to_str().unwrap()));

    let root = include_str!("../src/lib.rs");
    assert!(root.contains("#[doc(hidden)]\npub use session::lower_secure_connect_error;"));
    assert!(!include_str!("../../rust/src/lib.rs").contains("lower_secure_connect_error"));
}

#[test]
fn raw_provider_state_seams_remain_hidden() {
    assert!(
        include_str!("../src/session/backend.rs").contains("#[doc(hidden)]\n    fn raw_is_open")
    );
    assert!(
        include_str!("../../typedb-runtime/src/raw.rs")
            .contains("#[doc(hidden)]\n    pub fn is_open")
    );
}
