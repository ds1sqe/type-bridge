use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_schema::{SchemaAuthorityError, SchemaAuthorityErrorCode};

use crate::abi::TypeBridgeDiagnostics;

pub(crate) fn stable(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static C ABI diagnostic code is valid"),
        message,
    )
}

pub(crate) fn authority_diagnostics(error: SchemaAuthorityError) -> Vec<Diagnostic> {
    if let Some(contract) = error.contract() {
        return vec![contract.clone()];
    }
    if let Some(schema) = error.schema() {
        return schema
            .iter()
            .map(|item| item.diagnostic().clone())
            .collect();
    }
    let (category, code) = match error.code() {
        SchemaAuthorityErrorCode::Contract | SchemaAuthorityErrorCode::Schema => (
            DiagnosticCategory::InvalidContract,
            "c_schema_authority_invalid",
        ),
        SchemaAuthorityErrorCode::UnsupportedVersion => (
            DiagnosticCategory::UnsupportedCapability,
            "c_schema_authority_version_unsupported",
        ),
        SchemaAuthorityErrorCode::UnsupportedCapability => (
            DiagnosticCategory::UnsupportedCapability,
            "c_schema_authority_capability_unsupported",
        ),
        SchemaAuthorityErrorCode::ResourceLimit => (
            DiagnosticCategory::ResourceLimit,
            "c_schema_authority_resource_limit",
        ),
        SchemaAuthorityErrorCode::IntegrityMismatch => (
            DiagnosticCategory::Integrity,
            "c_schema_authority_integrity_mismatch",
        ),
    };
    vec![stable(
        category,
        code,
        "generated schema authority was rejected",
    )]
}

pub(crate) fn diagnostics_handle(diagnostics: Vec<Diagnostic>) -> TypeBridgeDiagnostics {
    let encoded = to_canonical_json(&diagnostics).unwrap_or_else(|_| {
        br#"[{"category":"invalid_contract","code":"c_diagnostic_encoding_failed","details":{},"message":"structured C diagnostic encoding failed","path":[]}]"#.to_vec()
    });
    TypeBridgeDiagnostics { encoded }
}
