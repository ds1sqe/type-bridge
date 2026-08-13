use type_bridge_contract::projection::{
    ProjectedTokenIdentity, ProjectedTokenKind, TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};

use crate::abi::SchemaPackageState;

/// Version-1 generated-only projected-token descriptor.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeBridgeProjectedTokenV1 {
    /// Exact size of this descriptor.
    pub struct_size: u32,
    /// Generated projection-token layout version.
    pub version: u32,
    /// Frozen token kind: model `1`, field `2`, role `3`, or function `4`.
    pub kind: u32,
    /// Zero-based canonical ordinal within the token kind.
    pub ordinal: u32,
    /// Raw SHA-256 digest of the exact binding projection.
    pub projection_digest: [u8; 32],
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C projected-token code is valid")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C projected-token message is valid")
}

fn invalid_layout() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        code("c_projected_token_layout_invalid"),
        message("The generated projection token layout is invalid"),
    )
}

fn invalid_brand() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(
        code("c_projected_token_brand_mismatch"),
        message("The generated projection token belongs to a different schema package"),
    )
}

fn invalid_ordinal() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(
        code("c_projected_token_ordinal_invalid"),
        message("The generated projection token ordinal is unavailable"),
    )
}

pub(crate) unsafe fn resolve_token(
    package: &SchemaPackageState,
    token: *const TypeBridgeProjectedTokenV1,
    expected: ProjectedTokenKind,
) -> Result<ProjectedTokenIdentity, SdkExecutionDiagnostic> {
    if token.is_null() {
        return Err(invalid_layout());
    }
    // SAFETY: caller promises one readable token descriptor. Unaligned reads
    // admit generated constants placed in packed or otherwise hostile storage.
    let token = unsafe { token.read_unaligned() };
    if token.struct_size as usize != size_of::<TypeBridgeProjectedTokenV1>()
        || token.version != TYPE_BRIDGE_PROJECTED_TOKEN_VERSION
        || token.reserved != [0; 4]
        || ProjectedTokenKind::from_u32(token.kind) != Some(expected)
    {
        return Err(invalid_layout());
    }
    let digest = package
        ._projection
        .projection_fingerprint()
        .as_fingerprint()
        .digest()
        .bytes();
    if token.projection_digest != digest {
        return Err(invalid_brand());
    }
    package
        ._projection
        .projected_token_identity(expected, token.ordinal)
        .ok_or_else(invalid_ordinal)
}

pub(crate) unsafe fn resolve_model_token(
    package: &SchemaPackageState,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<type_bridge_contract::id::TypeId, SdkExecutionDiagnostic> {
    // SAFETY: token validation and snapshotting are delegated to the shared resolver.
    match unsafe { resolve_token(package, token, ProjectedTokenKind::Model) }? {
        ProjectedTokenIdentity::Model(id) => Ok(id),
        _ => Err(invalid_layout()),
    }
}

pub(crate) unsafe fn resolve_field_token(
    package: &SchemaPackageState,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<
    (
        type_bridge_contract::id::TypeId,
        type_bridge_contract::schema::OwnsFactId,
    ),
    SdkExecutionDiagnostic,
> {
    // SAFETY: token validation and snapshotting are delegated to the shared resolver.
    match unsafe { resolve_token(package, token, ProjectedTokenKind::Field) }? {
        ProjectedTokenIdentity::Field { owner, field } => Ok((owner, field)),
        _ => Err(invalid_layout()),
    }
}

pub(crate) unsafe fn resolve_role_token(
    package: &SchemaPackageState,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<
    (
        type_bridge_contract::id::TypeId,
        type_bridge_contract::id::RoleId,
    ),
    SdkExecutionDiagnostic,
> {
    // SAFETY: token validation and snapshotting are delegated to the shared resolver.
    match unsafe { resolve_token(package, token, ProjectedTokenKind::Role) }? {
        ProjectedTokenIdentity::Role { owner, role } => Ok((owner, role)),
        _ => Err(invalid_layout()),
    }
}

pub(crate) unsafe fn resolve_function_token(
    package: &SchemaPackageState,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<type_bridge_contract::id::FunctionId, SdkExecutionDiagnostic> {
    // SAFETY: token validation and snapshotting are delegated to the shared resolver.
    match unsafe { resolve_token(package, token, ProjectedTokenKind::Function) }? {
        ProjectedTokenIdentity::Function(id) => Ok(id),
        _ => Err(invalid_layout()),
    }
}
