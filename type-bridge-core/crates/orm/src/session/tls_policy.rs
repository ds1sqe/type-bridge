//! Closed TLS policy shared by direct and proxy connections.

use std::fmt;
use std::path::{Path, PathBuf};

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};

#[derive(Clone)]
pub(crate) enum TlsPolicyKind {
    Disabled,
    NativeRoots,
    CustomRoot(PathBuf),
}

/// Closed TLS policy for one generated-package direct or proxy connection.
///
/// The variants are intentionally private so contradictory trust input cannot
/// be represented. Custom-root paths are opened and snapshotted only when a
/// connection invocation begins, after package authority and noncredential
/// configuration have passed validation.
#[derive(Clone)]
pub struct TlsPolicy {
    pub(crate) kind: TlsPolicyKind,
}

impl TlsPolicy {
    /// Select plaintext transport.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            kind: TlsPolicyKind::Disabled,
        }
    }

    /// Select TLS using the operating system's native trust roots.
    #[must_use]
    pub const fn native_roots() -> Self {
        Self {
            kind: TlsPolicyKind::NativeRoots,
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
            kind: TlsPolicyKind::CustomRoot(path.to_path_buf()),
        })
    }
}

impl Default for TlsPolicy {
    fn default() -> Self {
        Self::disabled()
    }
}

impl fmt::Debug for TlsPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TlsPolicy([REDACTED])")
    }
}

fn invalid_input(code: &'static str, message: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new(code).expect("static TLS policy diagnostic code is canonical"),
        SdkDiagnosticMessage::new(message).expect("static TLS policy diagnostic message is valid"),
    )
}
