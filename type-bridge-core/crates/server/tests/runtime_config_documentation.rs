use std::path::PathBuf;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_schema::decode_schema_authority;
use type_bridge_server::config::{OutboundTlsMode, RuntimeServerConfig, StagedAuthorityMode};

#[test]
fn documented_runtime_config_is_executable() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/runtime-server.toml");
    let config = RuntimeServerConfig::from_file(fixture.to_str().unwrap()).unwrap();

    assert_eq!(config.server.host, "0.0.0.0");
    assert_eq!(config.server.port, 8080);
    assert_eq!(config.typedb.database(), "my_database");
    assert_eq!(config.interceptors.enabled, ["audit-log"]);
    assert!(config.staged.enabled);
    assert_eq!(config.staged.schema_authority_file, "schema-authority.json");
    let authority = decode_schema_authority(
        config.staged_schema_authority_bytes().unwrap().unwrap(),
        &CapabilitySet::new(),
    )
    .expect("documented source-free authority is canonical and verified");
    assert_eq!(authority.managed_scope().id().as_str(), "production");
    assert_eq!(
        authority.semantic_profile().id().as_str(),
        "typedb-3.12.1/v1"
    );
    assert_eq!(config.staged.authority_mode, StagedAuthorityMode::Managed);
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
