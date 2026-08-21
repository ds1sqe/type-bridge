//! Binding-neutral generated-package direct connection policy.

use std::fmt;
use std::path::{Path, PathBuf};

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_core_lib::version::{DEFAULT_HTTP_PORT, TYPEDB_3_12_SEMANTIC_PROFILE_ID};

use super::backend::AnswerCancellation;
use super::real_driver::{
    PreparedSecureConnectOptions, SecureConnectError, SecureConnectOptions, TlsMode,
};
use crate::error::OrmError;
use crate::execution_diagnostic::lower_execution_error;
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::runtime_projection::InstalledRuntimeProjection;

const MAX_ENDPOINT_BYTES: usize = 4 * 1024;
const MAX_DATABASE_BYTES: usize = 256;
const MAX_USERNAME_BYTES: usize = 4 * 1024;
const MAX_PASSWORD_BYTES: usize = 64 * 1024;

#[derive(Clone)]
enum DirectTlsKind {
    Disabled,
    NativeRoots,
    CustomRoot(PathBuf),
}

/// Closed TLS policy for one generated-package direct connection.
///
/// The variants are intentionally private so contradictory trust input cannot
/// be represented. Custom-root paths are opened and snapshotted only when a
/// connection invocation begins, after package authority and noncredential
/// configuration have passed validation.
#[derive(Clone)]
pub struct DirectTls {
    kind: DirectTlsKind,
}

impl DirectTls {
    /// Select plaintext HTTP and gRPC.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            kind: DirectTlsKind::Disabled,
        }
    }

    /// Select TLS using the operating system's native trust roots.
    #[must_use]
    pub const fn native_roots() -> Self {
        Self {
            kind: DirectTlsKind::NativeRoots,
        }
    }

    /// Select TLS using one custom-root path snapshotted at connect time.
    pub fn custom_root(path: impl AsRef<Path>) -> Result<Self, SdkExecutionDiagnostic> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err(invalid_input(
                "direct_tls_custom_root_path_empty",
                "Custom-root TLS requires a nonempty trust path",
            ));
        }
        Ok(Self {
            kind: DirectTlsKind::CustomRoot(path.to_path_buf()),
        })
    }
}

impl Default for DirectTls {
    fn default() -> Self {
        Self::disabled()
    }
}

impl fmt::Debug for DirectTls {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectTls([REDACTED])")
    }
}

/// Immutable generated-package direct connection policy.
///
/// This value carries no caller-claimed server version. Each connection
/// invocation validates its installed projection authority, captures one
/// monotonic deadline, snapshots trust, and only then validates and copies
/// credentials for the authoritative provider probe and exact-version gate.
#[derive(Clone)]
pub struct DirectConnectionPolicy {
    endpoint: String,
    database: String,
    username: String,
    password: String,
    http_port: u16,
    tls: DirectTls,
    connection_limits: QueryExecutionResourceLimits,
    answer_limits: QueryExecutionResourceLimits,
}

impl DirectConnectionPolicy {
    /// Construct a reusable policy with plaintext transport and common limits.
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        database: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            database: database.into(),
            username: username.into(),
            password: password.into(),
            http_port: DEFAULT_HTTP_PORT,
            tls: DirectTls::disabled(),
            connection_limits: QueryExecutionResourceLimits::default(),
            answer_limits: QueryExecutionResourceLimits::default(),
        }
    }

    /// Set the authoritative HTTP version-probe port.
    #[must_use]
    pub const fn http_port(mut self, http_port: u16) -> Self {
        self.http_port = http_port;
        self
    }

    /// Set the closed TLS policy.
    #[must_use]
    pub fn tls(mut self, tls: DirectTls) -> Self {
        self.tls = tls;
        self
    }

    /// Tighten connection-time deadline and resource ceilings.
    #[must_use]
    pub const fn connection_limits(mut self, limits: QueryExecutionResourceLimits) -> Self {
        self.connection_limits = limits.effective();
        self
    }

    /// Tighten the immutable ceiling inherited by database operations.
    #[must_use]
    pub const fn answer_limits(mut self, limits: QueryExecutionResourceLimits) -> Self {
        self.answer_limits = limits.effective();
        self
    }
}

impl fmt::Debug for DirectConnectionPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectConnectionPolicy([REDACTED])")
    }
}

pub(super) struct PreparedDirectConnection {
    pub(super) endpoint: String,
    pub(super) database: String,
    pub(super) username: String,
    pub(super) password: String,
    pub(super) transport: PreparedSecureConnectOptions,
    pub(super) connection_limits: QueryExecutionResourceLimits,
    pub(super) answer_limits: QueryExecutionResourceLimits,
    pub(super) deadline: QueryExecutionDeadline,
}

pub(super) fn prepare_direct_connection(
    installed: &InstalledRuntimeProjection,
    policy: &DirectConnectionPolicy,
    cancellation: &AnswerCancellation,
) -> Result<PreparedDirectConnection, SdkExecutionDiagnostic> {
    // The sole absolute deadline is captured before package validation,
    // configuration inspection, trust I/O, credential copying, or provider I/O.
    let connection_limits = policy.connection_limits.effective();
    let answer_limits = policy.answer_limits.effective();
    let deadline = QueryExecutionDeadline::for_limits(connection_limits);

    require_exact_package_profile(installed)?;
    check_control(deadline, cancellation)?;

    validate_noncredential_text(
        &policy.endpoint,
        MAX_ENDPOINT_BYTES,
        "direct_connection_endpoint_limit_exceeded",
        "The provider endpoint exceeds its stable byte ceiling",
        "direct_connection_endpoint_empty",
        "The provider endpoint is empty",
    )?;
    if !super::database::is_identity_safe_provider_address(&policy.endpoint) {
        return Err(invalid_input(
            "direct_connection_endpoint_invalid",
            "The provider endpoint is not one canonical credential-free host and port",
        ));
    }
    if policy.endpoint.contains(',') {
        return Err(unsupported(
            "direct_connection_multi_endpoint_unsupported",
            "Generated direct connections support exactly one provider endpoint",
        ));
    }
    validate_noncredential_text(
        &policy.database,
        MAX_DATABASE_BYTES,
        "direct_connection_database_limit_exceeded",
        "The database name exceeds its stable byte ceiling",
        "direct_connection_database_empty",
        "The database name is empty",
    )?;
    if policy.http_port == 0 {
        return Err(invalid_input(
            "direct_connection_http_port_invalid",
            "The authoritative HTTP probe port is invalid",
        ));
    }

    let tls_mode = match &policy.tls.kind {
        DirectTlsKind::Disabled => TlsMode::Disabled,
        DirectTlsKind::NativeRoots => TlsMode::NativeRoots,
        DirectTlsKind::CustomRoot(path) => TlsMode::CustomRootCa(path.clone()),
    };
    let transport = SecureConnectOptions {
        http_port: policy.http_port,
        tls_mode,
        server_version: None,
    }
    .prepare_transport()
    .map_err(lower_tls_configuration)?;
    check_control(deadline, cancellation)?;

    validate_credential(
        &policy.username,
        MAX_USERNAME_BYTES,
        false,
        "direct_connection_username_limit_exceeded",
        "The username exceeds its stable byte ceiling",
        "direct_connection_username_empty",
        "The username is empty",
    )?;
    validate_credential(
        &policy.password,
        MAX_PASSWORD_BYTES,
        true,
        "direct_connection_password_limit_exceeded",
        "The password exceeds its stable byte ceiling",
        "direct_connection_password_empty",
        "The password is empty",
    )?;

    // Clone credentials only after the complete package, endpoint, database,
    // limits, TLS mode, and custom-root snapshot have passed preflight.
    let username = policy.username.clone();
    let password = policy.password.clone();
    check_control(deadline, cancellation)?;

    Ok(PreparedDirectConnection {
        endpoint: policy.endpoint.clone(),
        database: policy.database.clone(),
        username,
        password,
        transport,
        connection_limits,
        answer_limits,
        deadline,
    })
}

pub(super) fn lower_secure_connection(error: SecureConnectError) -> SdkExecutionDiagnostic {
    if let Some(code_value) = error.configuration_code() {
        let code = code(code_value);
        let message = message("The direct connection TLS policy is invalid");
        return if code_value == "tls_custom_root_ca_too_large" {
            SdkExecutionDiagnostic::resource_limit(code, message)
        } else {
            SdkExecutionDiagnostic::invalid_input(code, message)
        };
    }
    let error: OrmError = error.into_runtime_error().into();
    lower_execution_error(error, SdkProviderOperation::Connect)
}

fn require_exact_package_profile(
    installed: &InstalledRuntimeProjection,
) -> Result<(), SdkExecutionDiagnostic> {
    let profile = installed
        .projection()
        .semantic_fingerprint()
        .as_fingerprint()
        .semantic_profile()
        .map(|profile| profile.as_str());
    if profile == Some(TYPEDB_3_12_SEMANTIC_PROFILE_ID) {
        return Ok(());
    }
    Err(unsupported(
        "direct_connection_semantic_profile_unsupported",
        "The generated package semantic profile is unsupported for direct connection",
    ))
}

fn validate_noncredential_text(
    value: &str,
    maximum: usize,
    limit_code: &'static str,
    limit_message: &'static str,
    empty_code: &'static str,
    empty_message: &'static str,
) -> Result<(), SdkExecutionDiagnostic> {
    if value.len() > maximum {
        return Err(resource_limit(limit_code, limit_message));
    }
    if value.is_empty() {
        return Err(invalid_input(empty_code, empty_message));
    }
    if value.as_bytes().contains(&0) {
        return Err(invalid_input(
            "direct_connection_text_nul_invalid",
            "A direct connection configuration value contains a NUL byte",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_credential(
    value: &str,
    maximum: usize,
    allow_empty: bool,
    limit_code: &'static str,
    limit_message: &'static str,
    empty_code: &'static str,
    empty_message: &'static str,
) -> Result<(), SdkExecutionDiagnostic> {
    if value.len() > maximum {
        return Err(resource_limit(limit_code, limit_message));
    }
    if value.is_empty() && !allow_empty {
        return Err(invalid_input(empty_code, empty_message));
    }
    if value.as_bytes().contains(&0) {
        return Err(invalid_input(
            "direct_connection_credential_nul_invalid",
            "A direct connection credential contains a NUL byte",
        ));
    }
    Ok(())
}

fn check_control(
    deadline: QueryExecutionDeadline,
    cancellation: &AnswerCancellation,
) -> Result<(), SdkExecutionDiagnostic> {
    if cancellation.is_cancelled() {
        Err(SdkExecutionDiagnostic::data_operation_cancelled())
    } else if deadline.is_expired() {
        Err(SdkExecutionDiagnostic::data_operation_deadline_exceeded())
    } else {
        Ok(())
    }
}

fn lower_tls_configuration(error: SecureConnectError) -> SdkExecutionDiagnostic {
    lower_secure_connection(error)
}

fn invalid_input(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

fn unsupported(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::unsupported_capability(code(code_value), message(message_value))
}

fn resource_limit(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::resource_limit(code(code_value), message(message_value))
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static direct connection diagnostic code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static direct connection diagnostic message is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_connection_fails_before_provider_dispatch() {
        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let error = check_control(
            QueryExecutionDeadline::from_timeout_milliseconds(60_000),
            &cancellation,
        )
        .expect_err("pre-cancelled connection must fail");

        assert_eq!(error.code().as_str(), "provider_cancelled");
    }

    #[test]
    fn expired_connection_deadline_fails_before_provider_dispatch() {
        let error = check_control(
            QueryExecutionDeadline::from_timeout_milliseconds(0),
            &AnswerCancellation::default(),
        )
        .expect_err("zero-timeout connection must fail");

        assert_eq!(error.code().as_str(), "transaction_deadline_exceeded");
    }

    #[test]
    fn direct_policy_debug_redacts_all_connection_material() {
        let policy = DirectConnectionPolicy::new(
            "hostile-endpoint:1729",
            "hostile-database",
            "hostile-user",
            "hostile-password",
        )
        .tls(DirectTls::custom_root("/hostile/root.pem").unwrap());
        let diagnostic = format!("{policy:?}");

        assert_eq!(diagnostic, "DirectConnectionPolicy([REDACTED])");
    }
}
