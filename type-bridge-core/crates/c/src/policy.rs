//! Shared C policy parsing for query, CRUD, batch, and connection entry points.

use std::mem::size_of;

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::{
    AnswerCancellation, MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES, MAX_QUERY_COLLECTION_MEMBERS,
    MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS, MAX_QUERY_ROLE_PLAYERS, MAX_QUERY_STATEMENTS,
    MAX_QUERY_TIMEOUT_MILLISECONDS, QueryExecutionDeadline, QueryExecutionResourceLimits,
};

use crate::abi::TypeBridgeByteView;
use crate::query::TypeBridgeQueryExecutionLimitsV1;

const LIMITS_VERSION: u32 = 1;

/// Frozen database configuration V2 descriptor version.
pub const DATABASE_CONFIG_V2_VERSION: u32 = 2;
/// Frozen maximum copied custom-root PEM input length.
pub const DATABASE_CUSTOM_ROOT_CA_BYTES_MAX: usize = 1_048_576;
/// Plaintext transport mode.
pub const TLS_DISABLED: u32 = 0;
/// Native-root TLS transport mode.
pub const TLS_NATIVE_ROOTS: u32 = 1;
/// Captured custom-root TLS transport mode.
pub const TLS_CUSTOM_ROOT_CA: u32 = 2;

/// Version-2 direct TypeDB database connection and answer policy.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TypeBridgeDatabaseConfigV2 {
    /// Exact `sizeof` of this structure.
    pub struct_size: u32,
    /// Database configuration layout version.
    pub version: u32,
    /// TypeDB gRPC address as bounded copied UTF-8.
    pub address: TypeBridgeByteView,
    /// TypeDB database name as bounded copied UTF-8.
    pub database: TypeBridgeByteView,
    /// Username as bounded copied UTF-8.
    pub username: TypeBridgeByteView,
    /// Password as bounded copied UTF-8. Empty is admitted.
    pub password: TypeBridgeByteView,
    /// Authoritative HTTP version-probe port.
    pub http_port: u32,
    /// One closed `TYPE_BRIDGE_TLS_*` transport mode.
    pub tls_mode: u32,
    /// Captured custom-root PEM bytes, canonical-empty outside custom-root mode.
    pub custom_root_ca_pem: TypeBridgeByteView,
    /// Tighten-only connection-time resource policy.
    pub connection_limits: TypeBridgeQueryExecutionLimitsV1,
    /// Immutable ceiling for every operation opened through this database.
    pub answer_limits: TypeBridgeQueryExecutionLimitsV1,
    /// Reserved zero words for compatible growth.
    pub reserved: [u64; 4],
}

/// One parsed resource ceiling and the absolute deadline captured from it.
pub(crate) struct ParsedLimits {
    pub(crate) resources: QueryExecutionResourceLimits,
    pub(crate) deadline: QueryExecutionDeadline,
}

fn invalid_limits() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(
        SdkDiagnosticCode::new("c_query_execution_limits_invalid")
            .expect("static C limits diagnostic code is canonical"),
        SdkDiagnosticMessage::new(
            "Typed-query limits must use the canonical version-1 descriptor layout",
        )
        .expect("static C limits diagnostic message is valid"),
    )
}

fn default_limits_descriptor() -> TypeBridgeQueryExecutionLimitsV1 {
    TypeBridgeQueryExecutionLimitsV1 {
        struct_size: size_of::<TypeBridgeQueryExecutionLimitsV1>() as u32,
        version: LIMITS_VERSION,
        timeout_milliseconds: MAX_QUERY_TIMEOUT_MILLISECONDS,
        items: MAX_QUERY_ITEMS,
        bytes: MAX_QUERY_BYTES,
        graph_nodes: MAX_QUERY_GRAPH_NODES,
        attribute_values: MAX_QUERY_ATTRIBUTE_VALUES,
        collection_members: MAX_QUERY_COLLECTION_MEMBERS,
        role_players: MAX_QUERY_ROLE_PLAYERS,
        statements: MAX_QUERY_STATEMENTS,
        reserved0: 0,
        reserved: [0; 4],
    }
}

/// Parse one optional frozen limits descriptor and clamp it to common ceilings.
pub(crate) fn parse_common_limits(
    value: Option<TypeBridgeQueryExecutionLimitsV1>,
) -> Result<QueryExecutionResourceLimits, SdkExecutionDiagnostic> {
    let value = value.unwrap_or_else(default_limits_descriptor);
    if value.struct_size as usize != size_of::<TypeBridgeQueryExecutionLimitsV1>()
        || value.version != LIMITS_VERSION
        || value.reserved0 != 0
        || value.reserved != [0; 4]
    {
        return Err(invalid_limits());
    }
    Ok(QueryExecutionResourceLimits::tightened(
        value.timeout_milliseconds,
        value.items,
        value.bytes,
        value.graph_nodes,
        value.attribute_values,
        value.collection_members,
        value.role_players,
        value.statements,
    ))
}

/// Parse one optional limits descriptor under an immutable enclosing ceiling.
pub(crate) fn parse_limits_constrained(
    value: Option<TypeBridgeQueryExecutionLimitsV1>,
    cancellation: &AnswerCancellation,
    ceiling: Option<QueryExecutionResourceLimits>,
) -> Result<ParsedLimits, SdkExecutionDiagnostic> {
    let mut resources = parse_common_limits(value)?;
    if let Some(ceiling) = ceiling {
        resources = resources.constrained_by(ceiling);
    }
    let deadline = QueryExecutionDeadline::for_limits(resources);
    deadline.check(cancellation)?;
    Ok(ParsedLimits {
        resources,
        deadline,
    })
}

#[cfg(test)]
mod tests {
    use std::mem::{offset_of, size_of};

    use super::*;

    #[test]
    fn database_config_v2_native_layout_matches_the_frozen_lp64_contract() {
        assert_eq!(size_of::<TypeBridgeDatabaseConfigV2>(), 336);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, struct_size), 0);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, version), 4);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, address), 8);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, database), 24);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, username), 40);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, password), 56);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, http_port), 72);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, tls_mode), 76);
        assert_eq!(
            offset_of!(TypeBridgeDatabaseConfigV2, custom_root_ca_pem),
            80
        );
        assert_eq!(
            offset_of!(TypeBridgeDatabaseConfigV2, connection_limits),
            96
        );
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, answer_limits), 200);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, reserved), 304);
    }
}
