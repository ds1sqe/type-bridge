use std::fmt;

use jni::{Env, errors::ErrorPolicy, jni_str, strings::JNIString};
use type_bridge_contract::codec::to_canonical_json_with_limits;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::{CodecLimits, MAX_DIAGNOSTIC_BYTES};
use type_bridge_schema::{SchemaAuthorityError, SchemaAuthorityErrorCode};

#[derive(Debug)]
pub(crate) enum JvmError {
    Diagnostics(Vec<Diagnostic>),
    Jni(jni::errors::Error),
    Execution(Box<type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic>),
}

impl fmt::Display for JvmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostics(_) | Self::Execution(_) => {
                formatter.write_str("Kotlin/JVM package operation rejected")
            }
            Self::Jni(_) => formatter.write_str("Kotlin/JVM boundary operation failed"),
        }
    }
}
impl std::error::Error for JvmError {}
impl From<jni::errors::Error> for JvmError {
    fn from(error: jni::errors::Error) -> Self {
        Self::Jni(error)
    }
}
impl From<type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> for JvmError {
    fn from(error: type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic) -> Self {
        Self::Execution(Box::new(error))
    }
}
impl From<Diagnostic> for JvmError {
    fn from(error: Diagnostic) -> Self {
        Self::Diagnostics(vec![error])
    }
}
impl From<SchemaAuthorityError> for JvmError {
    fn from(error: SchemaAuthorityError) -> Self {
        if let Some(diagnostic) = error.contract() {
            return diagnostic.clone().into();
        }
        if let Some(diagnostics) = error.schema() {
            return Self::Diagnostics(diagnostics.iter().map(|d| d.diagnostic().clone()).collect());
        }
        let (category, code) = match error.code() {
            SchemaAuthorityErrorCode::Contract | SchemaAuthorityErrorCode::Schema => (
                DiagnosticCategory::InvalidContract,
                "jvm_schema_authority_invalid",
            ),
            SchemaAuthorityErrorCode::UnsupportedVersion => (
                DiagnosticCategory::UnsupportedCapability,
                "jvm_schema_authority_version_unsupported",
            ),
            SchemaAuthorityErrorCode::UnsupportedCapability => (
                DiagnosticCategory::UnsupportedCapability,
                "jvm_schema_authority_capability_unsupported",
            ),
            SchemaAuthorityErrorCode::ResourceLimit => (
                DiagnosticCategory::ResourceLimit,
                "jvm_schema_authority_resource_limit",
            ),
            SchemaAuthorityErrorCode::IntegrityMismatch => (
                DiagnosticCategory::Integrity,
                "jvm_schema_authority_integrity_mismatch",
            ),
        };
        diagnostic(category, code, "Generated schema authority was rejected").into()
    }
}

pub(crate) fn diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static JVM diagnostic code is valid"),
        message,
    )
}

pub(crate) fn invalid(code: &'static str, message: &'static str) -> JvmError {
    diagnostic(DiagnosticCategory::InvalidContract, code, message).into()
}

pub(crate) fn allocation_error() -> JvmError {
    diagnostic(
        DiagnosticCategory::ResourceLimit,
        "jvm_allocation_failed",
        "The JVM boundary could not allocate bounded native storage",
    )
    .into()
}

fn throw_error(env: &mut Env<'_>, error: JvmError) -> jni::errors::Result<()> {
    if env.exception_check() {
        return Ok(());
    }
    let diagnostics = match error {
        JvmError::Diagnostics(diagnostics) => diagnostics,
        JvmError::Execution(diagnostic) => {
            return match crate::execution_diagnostic::throw(env, &diagnostic) {
                Ok(()) | Err(JvmError::Jni(jni::errors::Error::JavaException)) => Ok(()),
                Err(error) => throw_error(env, error),
            };
        }

        JvmError::Jni(error) => {
            // Preserve pending Java exceptions, but keep implementation-specific
            // JNI errors out of the stable diagnostic contract.
            let _ = error;
            vec![diagnostic(
                DiagnosticCategory::InvalidContract,
                "jvm_boundary_failure",
                "A native JVM boundary operation failed",
            )]
        }
    };
    let limits = CodecLimits {
        max_bytes: MAX_DIAGNOSTIC_BYTES,
        ..CodecLimits::CANONICAL
    };
    let bytes = to_canonical_json_with_limits(&diagnostics, limits).unwrap_or_else(|_| {
        br#"[{"category":"resource_limit","code":"jvm_diagnostic_limit","details":{},"message":"Native diagnostics exceed their bounded encoding","path":[]}]"#.to_vec()
    });
    let payload = JNIString::from(String::from_utf8(bytes).expect("canonical JSON is UTF-8"));
    match env.throw_new(
        jni_str!("io/github/ds1sqe/typebridge/SchemaPackageException"),
        payload,
    ) {
        Err(jni::errors::Error::JavaException) => Ok(()),
        outcome => outcome,
    }
}

pub(crate) struct ThrowJvmError;
impl<T: Default> ErrorPolicy<T, JvmError> for ThrowJvmError {
    type Captures<'local: 'native, 'native> = ();

    fn on_error<'local: 'native, 'native>(
        env: &mut Env<'local>,
        _: &mut Self::Captures<'local, 'native>,
        error: JvmError,
    ) -> jni::errors::Result<T> {
        throw_error(env, error)?;
        Ok(T::default())
    }

    fn on_panic<'local: 'native, 'native>(
        env: &mut Env<'local>,
        _: &mut Self::Captures<'local, 'native>,
        payload: Box<dyn std::any::Any + Send + 'static>,
    ) -> jni::errors::Result<T> {
        if let Err(nested) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)))
        {
            std::mem::forget(nested);
        }
        throw_error(
            env,
            invalid(
                "jvm_internal_failure",
                "A native operation failed internally",
            ),
        )?;
        Ok(T::default())
    }
}
