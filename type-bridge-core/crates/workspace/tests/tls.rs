use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_schema::SystemSchemaSourceService;
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, SecretReference,
    SecretReferenceService, TypeBridgeConfig, TypeBridgeConfigServices, TypeBridgeConfigSpec,
    WorkspaceConfigError, WorkspaceConfigErrorCode, WorkspaceRoot, WorkspaceServiceError,
    WorkspaceSourceService, WorkspaceTransportPolicy,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

const TEST_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIBgDCCASegAwIBAgIUPHDUu9WL36yvTmFeNFZVe/qhClcwCgYIKoZIzj0EAwIw\n\
HTEbMBkGA1UEAwwSUnVzdGxzIFJvYnVzdCBSb290MCAXDTc1MDEwMTAwMDAwMFoY\n\
DzQwOTYwMTAxMDAwMDAwWjAdMRswGQYDVQQDDBJSdXN0bHMgUm9idXN0IFJvb3Qw\n\
WTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASW/VkDFs5iGDQvH8jaXYT4jMx66jo+\n\
5CWKyMt4OlTDdBfKfnmQ9LYeK/PsYfJ8wVizuSlPzXi9je8SnyYejGP3o0MwQTAP\n\
BgNVHQ8BAf8EBQMDB4QAMB0GA1UdDgQWBBRqY/oMENJbNo7y39iL6GW3tDs0rzAP\n\
BgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0cAMEQCIEUbrmSUjANju9nNpFop\n\
PAl9Wh8tBxI5IY+BPh466+aUAiA1/9+prypt6s3Doo0GDsnoFGJi1UBivUg1qdik\n\
cy4eNw==\n\
-----END CERTIFICATE-----\n";

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-workspace-tls-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("config")).unwrap();
        fs::create_dir_all(path.join("certs")).unwrap();
        Self(path)
    }

    fn root(&self) -> WorkspaceRoot {
        WorkspaceRoot::new(fs::canonicalize(&self.0).unwrap()).unwrap()
    }

    fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct AcceptSecrets;

impl SecretReferenceService for AcceptSecrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

struct RejectUnexpectedSecretValidation;

impl SecretReferenceService for RejectUnexpectedSecretValidation {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        panic!("TLS configuration must fail before secret-reference validation")
    }
}

struct AcceptExtensions;

impl ExtensionRegistryService for AcceptExtensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

struct RejectReadableFile;

impl WorkspaceSourceService for RejectReadableFile {
    fn canonicalize_workspace_root(&self, root: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        fs::canonicalize(root).map_err(|_| WorkspaceServiceError::new("root_unavailable"))
    }

    fn canonicalize_workspace_path(&self, path: &Path) -> Result<PathBuf, WorkspaceServiceError> {
        fs::canonicalize(path).map_err(|_| WorkspaceServiceError::new("path_unavailable"))
    }

    fn readable_workspace_file_len(&self, _path: &Path) -> Result<u64, WorkspaceServiceError> {
        Err(WorkspaceServiceError::new("file_unreadable"))
    }
}

fn yaml(transport: &str) -> String {
    format!(
        r#"format: typebridge.workspace/v1
schema:
  root: ../schema/schema.yaml
  ownership: exclusive
  managed-scope: example-schema
compatibility:
  semantic-profile: typedb-3.12.1/v1
migrations:
  directory: ../migrations/v2
  app-label: example
environments:
  dev:
    uri: localhost:1729
    database: example
{transport}    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
"#
    )
}

fn resolve(
    directory: &TempDirectory,
    transport: &str,
    source: &dyn WorkspaceSourceService,
) -> Result<TypeBridgeConfig, WorkspaceConfigError> {
    resolve_with_secrets(directory, transport, source, &AcceptSecrets)
}

fn resolve_with_secrets(
    directory: &TempDirectory,
    transport: &str,
    source: &dyn WorkspaceSourceService,
    secrets: &dyn SecretReferenceService,
) -> Result<TypeBridgeConfig, WorkspaceConfigError> {
    let origin = ConfigOrigin::new(
        directory.root(),
        "config/typebridge.yaml",
        "TLS workspace fixture",
    )
    .unwrap();
    TypeBridgeConfigSpec::parse_yaml(yaml(transport), origin)
        .unwrap()
        .resolve(&TypeBridgeConfigServices::new(
            source,
            secrets,
            &AcceptExtensions,
        ))
}

fn policy(config: &TypeBridgeConfig) -> &WorkspaceTransportPolicy {
    config.environment("dev").unwrap().transport_policy()
}

#[test]
fn explicit_tls_truth_table_resolves_to_one_typed_policy() {
    let directory = TempDirectory::new("truth-table");
    directory.write("certs/root.pem", TEST_CA_PEM);
    let source = SystemSchemaSourceService;

    assert_eq!(
        policy(&resolve(&directory, "", &source).unwrap()),
        &WorkspaceTransportPolicy::Disabled
    );
    assert_eq!(
        policy(&resolve(&directory, "    tls: 'false'\n", &source).unwrap()),
        &WorkspaceTransportPolicy::Disabled
    );
    assert_eq!(
        policy(&resolve(&directory, "    tls: 'true'\n", &source).unwrap()),
        &WorkspaceTransportPolicy::NativeRoots
    );

    let custom = resolve(
        &directory,
        "    tls: 'true'\n    tls-root-ca: ../certs/root.pem\n",
        &source,
    )
    .unwrap();
    let WorkspaceTransportPolicy::CustomRootCa(root_ca) = policy(&custom) else {
        panic!("expected a custom-root transport policy");
    };
    assert_eq!(
        root_ca.as_path(),
        fs::canonicalize(directory.0.join("certs/root.pem"))
            .unwrap()
            .as_path()
    );
}

#[test]
fn root_ca_without_enabled_tls_is_a_spanned_contradiction() {
    let directory = TempDirectory::new("contradictions");
    directory.write("certs/root.pem", TEST_CA_PEM);
    let source = SystemSchemaSourceService;

    let omitted = resolve(&directory, "    tls-root-ca: ../certs/root.pem\n", &source).unwrap_err();
    assert_eq!(
        omitted.code(),
        WorkspaceConfigErrorCode::TlsRootCaRequiresTls
    );
    assert_eq!(omitted.origin(), Some("TLS workspace fixture"));
    assert_eq!(omitted.source_span().unwrap().line(), 15);

    let disabled = resolve(
        &directory,
        "    tls: 'false'\n    tls-root-ca: ../certs/root.pem\n",
        &source,
    )
    .unwrap_err();
    assert_eq!(
        disabled.code(),
        WorkspaceConfigErrorCode::TlsRootCaWithDisabledTls
    );
    assert_eq!(disabled.source_span().unwrap().line(), 16);
}

#[test]
fn invalid_boolean_and_lexical_paths_are_rejected_at_the_authored_field() {
    let directory = TempDirectory::new("lexical-errors");
    let source = SystemSchemaSourceService;

    let invalid_boolean = resolve(&directory, "    tls: 'yes'\n", &source).unwrap_err();
    assert_eq!(
        invalid_boolean.code(),
        WorkspaceConfigErrorCode::InvalidTlsBoolean
    );
    assert_eq!(invalid_boolean.source_span().unwrap().line(), 15);

    for authored in ["", "../../outside.pem", "env:TYPEDB_ROOT_CA"] {
        let error = resolve(
            &directory,
            &format!("    tls: 'true'\n    tls-root-ca: '{authored}'\n"),
            &source,
        )
        .unwrap_err();
        assert_eq!(error.code(), WorkspaceConfigErrorCode::PathNotConfined);
        assert_eq!(error.source_span().unwrap().line(), 16);
    }
}

#[test]
fn empty_directory_and_unreadable_root_ca_files_fail_before_secret_validation() {
    let directory = TempDirectory::new("file-shape");
    directory.write("certs/empty.pem", []);
    directory.write("certs/root.pem", TEST_CA_PEM);
    let source = SystemSchemaSourceService;

    for root in ["../certs/empty.pem", "../certs"] {
        let error = resolve(
            &directory,
            &format!("    tls: 'true'\n    tls-root-ca: {root}\n"),
            &source,
        )
        .unwrap_err();
        assert_eq!(error.code(), WorkspaceConfigErrorCode::InvalidTlsRootCa);
        assert_eq!(error.source_span().unwrap().line(), 16);
    }

    let unreadable = resolve_with_secrets(
        &directory,
        "    tls: 'true'\n    tls-root-ca: ../certs/root.pem\n",
        &RejectReadableFile,
        &RejectUnexpectedSecretValidation,
    )
    .unwrap_err();
    assert_eq!(
        unreadable.code(),
        WorkspaceConfigErrorCode::InvalidTlsRootCa
    );
    assert_eq!(unreadable.detail(), Some("file_unreadable"));
    assert_eq!(unreadable.source_span().unwrap().line(), 16);
}

#[cfg(unix)]
#[test]
fn symbolic_link_escape_is_rejected_after_canonicalization() {
    use std::os::unix::fs::symlink;

    let directory = TempDirectory::new("symlink-root");
    let outside = TempDirectory::new("symlink-outside");
    outside.write("root.pem", TEST_CA_PEM);
    symlink(
        outside.0.join("root.pem"),
        directory.0.join("certs/link.pem"),
    )
    .unwrap();

    let error = resolve(
        &directory,
        "    tls: 'true'\n    tls-root-ca: ../certs/link.pem\n",
        &SystemSchemaSourceService,
    )
    .unwrap_err();
    assert_eq!(error.code(), WorkspaceConfigErrorCode::InvalidTlsRootCa);
    assert_eq!(error.source_span().unwrap().line(), 16);
}
