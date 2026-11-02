//! Captured TLS policy preparation for staged and raw runtime connections.

#[cfg(test)]
use crate::config::TypeDBSection;
use crate::config::{OutboundTlsMode, SecureTypeDBSection};
use crate::error::PipelineError;
use runtime::PreparedSecureConnectOptions;
use type_bridge_typedb_runtime as runtime;

fn secure_connect_options(
    config: &SecureTypeDBSection,
) -> Result<runtime::SecureConnectOptions, PipelineError> {
    let connection = &config.connection;
    let server_version = connection
        .server_version
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(|_| {
            PipelineError::Config(
                "typedb.server-version is invalid [typedb_server_version_invalid]".to_owned(),
            )
        })?;
    let tls_mode = match &config.tls_mode {
        OutboundTlsMode::Disabled => runtime::TlsMode::Disabled,
        OutboundTlsMode::NativeRoots => runtime::TlsMode::NativeRoots,
        OutboundTlsMode::CustomRootCa(path) => runtime::TlsMode::CustomRootCa(path.clone()),
    };

    Ok(runtime::SecureConnectOptions {
        http_port: connection.http_port,
        tls_mode,
        server_version,
    })
}

pub(crate) fn prepare_secure_connect_options(
    config: &SecureTypeDBSection,
) -> Result<PreparedSecureConnectOptions, PipelineError> {
    let options = secure_connect_options(config)?;
    match &config.custom_root_ca_snapshot {
        Some(snapshot) => {
            if !matches!(
                &config.tls_mode,
                OutboundTlsMode::CustomRootCa(path) if path == &snapshot.path
            ) {
                return Err(PipelineError::Connection(
                    "TLS configuration error [tls_custom_root_ca_identity_mismatch]: typedb.tls-root-ca changed after its relative material was captured; reload the configuration"
                        .to_owned(),
                ));
            }
            options
                .prepare_transport_from_captured_custom_root(snapshot.bytes.clone())
                .map_err(sanitize_prepared_connect_error)
        }
        None => options
            .prepare_transport_from_validated_physical_path()
            .map_err(sanitize_prepared_connect_error),
    }
}

pub(crate) fn sanitize_prepared_connect_error(error: runtime::SecureConnectError) -> PipelineError {
    use type_bridge_core_lib::version::VersionError;

    match error {
        runtime::SecureConnectError::TlsConfiguration(error) => PipelineError::Connection(
            format!(
                "TLS policy preparation failed [{}]; inspect the configured trust material",
                error.code()
            ),
        ),
        runtime::SecureConnectError::DriverTlsConfiguration { band } => {
            PipelineError::Connection(format!(
                "TLS policy lowering failed [tls_driver_lowering_failed] for TypeDB driver band {band}"
            ))
        }
        runtime::SecureConnectError::Runtime(runtime::RuntimeError::UnsupportedVersion(
            error @ (VersionError::Unsupported { .. }
            | VersionError::BandMismatch { .. }
            | VersionError::EmbeddedUnavailable { .. }
            | VersionError::FeatureUnsupported { .. }),
        )) => PipelineError::UnsupportedVersion(error),
        _ => PipelineError::Connection(
            "secure prepared connection failed [typedb_prepared_connect_failed]; inspect provider logs"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::path::PathBuf;

    use super::*;

    fn released_config() -> TypeDBSection {
        TypeDBSection {
            address: "localhost:1729".to_owned(),
            database: "db".to_owned(),
            username: "admin".to_owned(),
            password: "password".to_owned(),
            http_port: 8000,
            server_version: Some("3.12.1".to_owned()),
        }
    }

    #[test]
    fn secure_connect_path_preserves_the_typed_custom_root() {
        let root = PathBuf::from("root.pem");
        let config = SecureTypeDBSection::new(
            released_config(),
            OutboundTlsMode::CustomRootCa(root.clone()),
        );
        let options = secure_connect_options(&config).unwrap();
        assert!(matches!(
            options.tls_mode,
            runtime::TlsMode::CustomRootCa(path) if path == root
        ));
    }

    #[test]
    fn prepared_connect_errors_drop_all_untrusted_identity_and_provider_text() {
        const ADDRESS: &str = "admin:TB_ADDRESS_SECRET@provider.invalid:1729";
        const USERNAME: &str = "TB_USERNAME_SECRET";
        const PASSWORD: &str = "TB_PASSWORD_SECRET";
        const PROVIDER: &str = "TB_PROVIDER_SECRET";
        let raw = format!(
            "address={ADDRESS}; username={USERNAME}; password={PASSWORD}; provider={PROVIDER}"
        );
        let errors = [
            runtime::SecureConnectError::Runtime(runtime::RuntimeError::Connection(raw.clone())),
            runtime::SecureConnectError::Runtime(runtime::RuntimeError::UnsupportedVersion(
                type_bridge_core_lib::version::VersionError::Probe(raw.clone()),
            )),
            runtime::SecureConnectError::Runtime(runtime::RuntimeError::UnsupportedVersion(
                type_bridge_core_lib::version::VersionError::Parse(raw.clone()),
            )),
        ];

        for error in errors {
            let sanitized = sanitize_prepared_connect_error(error);
            let rendered = format!("{sanitized}\n{sanitized:?}");
            for secret in [ADDRESS, USERNAME, PASSWORD, PROVIDER] {
                assert!(!rendered.contains(secret), "{secret}: {rendered}");
            }
            assert!(
                sanitized.source().is_none(),
                "sanitized failures must not retain the raw provider error"
            );
        }
    }

    #[test]
    fn prepared_tls_failure_drops_the_configured_path_and_source() {
        const SENTINEL: &str = "TB_TLS_PATH_SECRET";
        let config = SecureTypeDBSection::new(
            released_config(),
            OutboundTlsMode::CustomRootCa(PathBuf::from(SENTINEL)),
        );
        let error = prepare_secure_connect_options(&config)
            .expect_err("a non-validated relative root path must reject");
        let rendered = format!("{error}\n{error:?}");
        assert!(!rendered.contains(SENTINEL), "{rendered}");
        assert!(rendered.contains("TLS policy preparation failed [tls_"));
        assert!(error.source().is_none());
    }

    #[test]
    fn prepared_connect_preserves_only_structured_version_diagnostics() {
        let version = type_bridge_core_lib::version::Version::new(3, 13, 0);
        let error = sanitize_prepared_connect_error(runtime::SecureConnectError::Runtime(
            runtime::RuntimeError::UnsupportedVersion(
                type_bridge_core_lib::version::VersionError::Unsupported {
                    component: "server",
                    found: version,
                },
            ),
        ));
        assert!(matches!(
            error,
            PipelineError::UnsupportedVersion(
                type_bridge_core_lib::version::VersionError::Unsupported {
                    component: "server",
                    found,
                }
            ) if found == version
        ));
    }

    #[test]
    fn secure_configured_version_parse_error_drops_the_source_value() {
        const SENTINEL: &str = "TB_VERSION_SECRET";
        let mut connection = released_config();
        connection.server_version = Some(SENTINEL.to_owned());
        let config = SecureTypeDBSection::new(connection, OutboundTlsMode::Disabled);
        let error = secure_connect_options(&config).expect_err("invalid version must reject");
        let rendered = format!("{error}\n{error:?}");
        assert!(!rendered.contains(SENTINEL), "{rendered}");
        assert!(error.source().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn prepared_server_path_rejects_a_post_validation_symlink_swap() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("server TLS directory");
        let outside = tempfile::tempdir().expect("outside TLS directory");
        let configured = directory.path().join("root.pem");
        let replacement = outside.path().join("replacement.pem");
        let pem = include_bytes!("../../tests/fixtures/certs/root.pem");
        std::fs::write(&configured, pem).expect("write validated server CA");
        std::fs::write(&replacement, pem).expect("write outside replacement CA");
        let physical = configured.canonicalize().expect("canonical server CA");
        std::fs::remove_file(&configured).expect("remove validated server CA");
        symlink(&replacement, &configured).expect("install outside replacement symlink");
        let config =
            SecureTypeDBSection::new(released_config(), OutboundTlsMode::CustomRootCa(physical));

        let error = prepare_secure_connect_options(&config)
            .expect_err("validated physical paths must not follow a replacement symlink");
        assert!(
            error.to_string().contains("tls_custom_root_ca_unreadable"),
            "{error}"
        );
    }

    #[test]
    fn prepared_server_uses_authority_captured_root_without_reopening_diagnostic_path() {
        let directory = tempfile::tempdir().expect("server TLS directory");
        let diagnostic_path = directory.path().join("already-renamed-root.pem");
        let mut config = SecureTypeDBSection::new(
            released_config(),
            OutboundTlsMode::CustomRootCa(diagnostic_path),
        );
        config.custom_root_ca_snapshot = Some(crate::config::CapturedConfiguredMaterial {
            path: match &config.tls_mode {
                OutboundTlsMode::CustomRootCa(path) => path.clone(),
                _ => unreachable!(),
            },
            bytes: include_bytes!("../../../typedb-runtime/tests/fixtures/root-ca.pem")
                .as_slice()
                .into(),
        });

        prepare_secure_connect_options(&config)
            .expect("captured root bytes must be lowered without reopening their old name");
    }

    #[test]
    fn prepared_server_rejects_mutated_policy_with_stale_captured_root() {
        let directory = tempfile::tempdir().expect("server TLS directory");
        let captured_path = directory.path().join("captured-root.pem");
        let mut config = SecureTypeDBSection::new(
            released_config(),
            OutboundTlsMode::CustomRootCa(captured_path.clone()),
        );
        config.custom_root_ca_snapshot = Some(crate::config::CapturedConfiguredMaterial {
            path: captured_path,
            bytes: include_bytes!("../../../typedb-runtime/tests/fixtures/root-ca.pem")
                .as_slice()
                .into(),
        });
        config.tls_mode = OutboundTlsMode::CustomRootCa(directory.path().join("mutated.pem"));

        let error = prepare_secure_connect_options(&config)
            .expect_err("a captured root must stay bound to its exact configured policy");
        assert!(
            error
                .to_string()
                .contains("tls_custom_root_ca_identity_mismatch"),
            "{error}"
        );
    }
}
