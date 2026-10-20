//! Shared C policy parsing for query, CRUD, batch, and connection entry points.

use std::mem::size_of;

use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::{
    AnswerCancellation, AnswerTransport, QueryExecutionDeadline, QueryExecutionResourceLimits,
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

/// Parse one optional frozen limits descriptor.
///
/// A missing descriptor takes `transport`'s defaults: a direct database,
/// transaction, or call the direct defaults, a remote context the proxy
/// (wire) defaults. A dimension may tighten or raise its default. A value
/// above a dimension's supported maximum is refused, never clamped.
pub(crate) fn parse_common_limits(
    value: Option<TypeBridgeQueryExecutionLimitsV1>,
    transport: AnswerTransport,
) -> Result<QueryExecutionResourceLimits, SdkExecutionDiagnostic> {
    let Some(value) = value else {
        return Ok(QueryExecutionResourceLimits::default_for(transport));
    };
    if value.struct_size as usize != size_of::<TypeBridgeQueryExecutionLimitsV1>()
        || value.version != LIMITS_VERSION
        || value.reserved0 != 0
        || value.reserved != [0; 4]
    {
        return Err(invalid_limits());
    }
    QueryExecutionResourceLimits {
        timeout_milliseconds: value.timeout_milliseconds,
        items: value.items,
        bytes: value.bytes,
        graph_nodes: value.graph_nodes,
        attribute_values: value.attribute_values,
        collection_members: value.collection_members,
        role_players: value.role_players,
        statements: value.statements,
    }
    .validate()
    .map_err(|_| invalid_limits())
}

/// Parse one optional per-call limits descriptor and capture its absolute
/// deadline. A missing descriptor takes the direct defaults; a proxy target
/// constrains them by its connection ceiling at execution.
pub(crate) fn parse_limits(
    value: Option<TypeBridgeQueryExecutionLimitsV1>,
    cancellation: &AnswerCancellation,
) -> Result<ParsedLimits, SdkExecutionDiagnostic> {
    let resources = parse_common_limits(value, AnswerTransport::Direct)?;
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
    fn a_missing_descriptor_takes_its_transports_defaults() {
        let direct = parse_common_limits(None, AnswerTransport::Direct).expect("direct defaults");
        let proxy = parse_common_limits(None, AnswerTransport::Proxy).expect("proxy defaults");
        assert_eq!(direct, QueryExecutionResourceLimits::default());
        assert_eq!(direct.items, type_bridge_orm::DEFAULT_DIRECT_QUERY_ITEMS);
        assert_eq!(direct.bytes, type_bridge_orm::DEFAULT_DIRECT_QUERY_BYTES);
        assert_eq!(proxy.items, type_bridge_orm::DEFAULT_PROXY_QUERY_ITEMS);
        assert_eq!(proxy.bytes, type_bridge_orm::DEFAULT_PROXY_QUERY_BYTES);
        assert_ne!(direct, proxy);
        // A direct session's per-call limits default to the direct policy.
        let call = parse_limits(None, &AnswerCancellation::default()).expect("call defaults");
        assert_eq!(call.resources, direct);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn database_config_v2_native_layout_matches_the_frozen_lp64_or_llp64_contract() {
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

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn database_config_v2_native_layout_matches_the_frozen_ilp32_contract() {
        assert_eq!(size_of::<TypeBridgeDatabaseConfigV2>(), 296);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, struct_size), 0);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, version), 4);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, address), 8);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, database), 16);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, username), 24);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, password), 32);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, http_port), 40);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, tls_mode), 44);
        assert_eq!(
            offset_of!(TypeBridgeDatabaseConfigV2, custom_root_ca_pem),
            48
        );
        assert_eq!(
            offset_of!(TypeBridgeDatabaseConfigV2, connection_limits),
            56
        );
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, answer_limits), 160);
        assert_eq!(offset_of!(TypeBridgeDatabaseConfigV2, reserved), 264);
    }
}
