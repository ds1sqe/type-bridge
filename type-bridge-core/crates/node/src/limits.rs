//! Core limits exported to the TypeScript package, which holds no copies.
//!
//! Answer-policy defaults come from the orm and byte ceilings from the
//! contract, so a changed core value reaches TypeScript with the addon.

use napi_derive::napi;
use type_bridge_contract::diagnostic;
use type_bridge_contract::limits;

// Direct defaults: what an omitted `QueryExecutionResourceLimits` dimension
// takes and what a direct session runs at.

/// Direct-session default: execution duration of a query terminal, in milliseconds.
#[napi]
pub const DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS: u64 =
    type_bridge_orm::DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS;
/// Direct-session default: provider/result item count.
#[napi]
pub const DEFAULT_DIRECT_QUERY_ITEMS: u64 = type_bridge_orm::DEFAULT_DIRECT_QUERY_ITEMS;
/// Direct-session default: provider, reply, and projected payload bytes.
#[napi]
pub const DEFAULT_DIRECT_QUERY_BYTES: u64 = type_bridge_orm::DEFAULT_DIRECT_QUERY_BYTES;
/// Direct-session default: hydrated/projected graph-node count.
#[napi]
pub const DEFAULT_DIRECT_QUERY_GRAPH_NODES: u64 = type_bridge_orm::DEFAULT_DIRECT_QUERY_GRAPH_NODES;
/// Direct-session default: hydrated/projected attribute-value count.
#[napi]
pub const DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES: u64 =
    type_bridge_orm::DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES;
/// Direct-session default: collection members, including reduction cells.
#[napi]
pub const DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS: u64 =
    type_bridge_orm::DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS;
/// Direct-session default: relation-role-player references.
#[napi]
pub const DEFAULT_DIRECT_QUERY_ROLE_PLAYERS: u64 =
    type_bridge_orm::DEFAULT_DIRECT_QUERY_ROLE_PLAYERS;
/// Direct-session default: provider statements per terminal.
#[napi]
pub const DEFAULT_DIRECT_QUERY_STATEMENTS: u64 =
    type_bridge_orm::DEFAULT_DIRECT_QUERY_STATEMENTS as u64;

// Proxy (wire) defaults: what a proxy connection runs at when no answer
// limits are given.

/// Proxy (wire) default: execution duration of a query terminal, in milliseconds.
#[napi]
pub const DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS: u64 =
    type_bridge_orm::DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS;
/// Proxy (wire) default: provider/result item count.
#[napi]
pub const DEFAULT_PROXY_QUERY_ITEMS: u64 = type_bridge_orm::DEFAULT_PROXY_QUERY_ITEMS;
/// Proxy (wire) default: provider, reply, and projected payload bytes.
#[napi]
pub const DEFAULT_PROXY_QUERY_BYTES: u64 = type_bridge_orm::DEFAULT_PROXY_QUERY_BYTES;
/// Proxy (wire) default: hydrated/projected graph-node count.
#[napi]
pub const DEFAULT_PROXY_QUERY_GRAPH_NODES: u64 = type_bridge_orm::DEFAULT_PROXY_QUERY_GRAPH_NODES;
/// Proxy (wire) default: hydrated/projected attribute-value count.
#[napi]
pub const DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES: u64 =
    type_bridge_orm::DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES;
/// Proxy (wire) default: collection members, including reduction cells.
#[napi]
pub const DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS: u64 =
    type_bridge_orm::DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS;
/// Proxy (wire) default: relation-role-player references.
#[napi]
pub const DEFAULT_PROXY_QUERY_ROLE_PLAYERS: u64 = type_bridge_orm::DEFAULT_PROXY_QUERY_ROLE_PLAYERS;
/// Proxy (wire) default: provider statements per terminal.
#[napi]
pub const DEFAULT_PROXY_QUERY_STATEMENTS: u64 =
    type_bridge_orm::DEFAULT_PROXY_QUERY_STATEMENTS as u64;

/// Largest canonical artifact (declared schema, plan), in bytes.
#[napi]
pub const MAX_CANONICAL_BYTES: u32 = to_u32(limits::MAX_CANONICAL_BYTES);
/// Largest remote request or reply envelope (and advertisement), in bytes.
#[napi]
pub const MAX_REMOTE_ENVELOPE_BYTES: u32 = to_u32(limits::MAX_REMOTE_ENVELOPE_BYTES);
/// Longest diagnostic code, in bytes.
#[napi]
pub const MAX_DIAGNOSTIC_CODE_BYTES: u32 = to_u32(diagnostic::MAX_DIAGNOSTIC_CODE_BYTES);

/// JavaScript sees these ceilings as numbers; refuse to build if one stops fitting.
const fn to_u32(value: usize) -> u32 {
    assert!(
        value <= u32::MAX as usize,
        "limit does not fit a JavaScript number export"
    );
    value as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeQueryExecutionResources;

    #[test]
    fn omitted_answer_dimensions_take_the_direct_defaults() {
        use type_bridge_orm::{AnswerTransport, QueryExecutionResourceLimits};

        let resources =
            NodeQueryExecutionResources::new(None, None, None, None, None, None, None, None)
                .expect("the core defaults are valid")
                .inner();
        assert_eq!(resources, QueryExecutionResourceLimits::default());
        let direct = QueryExecutionResourceLimits::default_for(AnswerTransport::Direct);
        assert_eq!(
            direct.timeout_milliseconds,
            DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS
        );
        assert_eq!(direct.items, DEFAULT_DIRECT_QUERY_ITEMS);
        assert_eq!(direct.bytes, DEFAULT_DIRECT_QUERY_BYTES);
        assert_eq!(direct.graph_nodes, DEFAULT_DIRECT_QUERY_GRAPH_NODES);
        assert_eq!(
            direct.attribute_values,
            DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES
        );
        assert_eq!(
            direct.collection_members,
            DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS
        );
        assert_eq!(direct.role_players, DEFAULT_DIRECT_QUERY_ROLE_PLAYERS);
        assert_eq!(
            u64::from(direct.statements),
            DEFAULT_DIRECT_QUERY_STATEMENTS
        );
        let proxy = QueryExecutionResourceLimits::default_for(AnswerTransport::Proxy);
        assert_eq!(
            proxy.timeout_milliseconds,
            DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS
        );
        assert_eq!(proxy.items, DEFAULT_PROXY_QUERY_ITEMS);
        assert_eq!(proxy.bytes, DEFAULT_PROXY_QUERY_BYTES);
        assert_eq!(proxy.graph_nodes, DEFAULT_PROXY_QUERY_GRAPH_NODES);
        assert_eq!(proxy.attribute_values, DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES);
        assert_eq!(
            proxy.collection_members,
            DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS
        );
        assert_eq!(proxy.role_players, DEFAULT_PROXY_QUERY_ROLE_PLAYERS);
        assert_eq!(u64::from(proxy.statements), DEFAULT_PROXY_QUERY_STATEMENTS);
    }
}
