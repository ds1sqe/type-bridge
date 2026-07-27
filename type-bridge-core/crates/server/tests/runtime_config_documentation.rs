use std::path::PathBuf;

use type_bridge_server::config::{OutboundTlsMode, RuntimeServerConfig, V2AuthorityMode};

#[test]
fn documented_runtime_config_is_executable() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runtime-server.toml");
    let config = RuntimeServerConfig::from_file(fixture.to_str().unwrap()).unwrap();

    assert_eq!(config.server.host, "0.0.0.0");
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.typedb.database(), "my_database");
    assert_eq!(config.schema.source_file, "schema.tql");
    assert_eq!(config.interceptors.enabled, ["audit-log"]);
    assert!(config.v2.enabled);
    assert_eq!(config.v2.declared_schema_file, "declared-schema.json");
    assert_eq!(config.v2.scope, "production");
    assert_eq!(config.v2.profile, "typedb-3.12.1/v1");
    assert_eq!(config.v2.authority_mode, V2AuthorityMode::Managed);
    let audit_log = config.interceptors.audit_log.unwrap();
    assert_eq!(audit_log.output, "file");
    assert_eq!(audit_log.file_path, "/var/log/audit.jsonl");
    assert!(matches!(
        config.typedb.tls_mode,
        OutboundTlsMode::CustomRootCa(ref path)
            if path.ends_with("tests/fixtures/certs/root.pem")
    ));
    let inbound = config.inbound_tls.unwrap();
    assert!(
        inbound
            .cert_path
            .ends_with("tests/fixtures/certs/server.pem")
    );
    assert!(
        inbound
            .key_path
            .ends_with("tests/fixtures/certs/server.key")
    );
}
