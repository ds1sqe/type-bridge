use type_bridge::ConnectionOptions;

#[test]
fn connection_options_defaults_and_redaction() {
    let opts = ConnectionOptions::new("localhost:1729", "mydb");
    assert_eq!(opts.address(), "localhost:1729");
    assert_eq!(opts.database(), "mydb");
    assert_eq!(opts.get_http_port(), 8000);
    assert!(!opts.is_tls());

    let opts_with_creds =
        ConnectionOptions::new("localhost:1729", "mydb").credentials("admin", "secret_pass");
    let debug_str = format!("{opts_with_creds:?}");
    assert!(
        debug_str.contains("[REDACTED]"),
        "Debug output must redact secret password"
    );
    assert!(
        !debug_str.contains("secret_pass"),
        "Debug output must never expose secret password in plaintext"
    );
}
