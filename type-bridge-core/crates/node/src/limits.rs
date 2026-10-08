//! Core limits exported to the TypeScript package, which holds no copies.
//!
//! Answer-policy defaults come from the orm and byte ceilings from the
//! contract, so a changed core value reaches TypeScript with the addon.

use napi_derive::napi;
use type_bridge_contract::diagnostic;
use type_bridge_contract::limits;

/// Default execution duration of a query terminal, in milliseconds.
#[napi]
pub const DEFAULT_QUERY_TIMEOUT_MILLISECONDS: u64 =
    type_bridge_orm::DEFAULT_QUERY_TIMEOUT_MILLISECONDS;
/// Default provider/result item count.
#[napi]
pub const DEFAULT_QUERY_ITEMS: u64 = type_bridge_orm::DEFAULT_QUERY_ITEMS;
/// Default provider, reply, and projected payload bytes.
#[napi]
pub const DEFAULT_QUERY_BYTES: u64 = type_bridge_orm::DEFAULT_QUERY_BYTES;
/// Default hydrated/projected graph-node count.
#[napi]
pub const DEFAULT_QUERY_GRAPH_NODES: u64 = type_bridge_orm::DEFAULT_QUERY_GRAPH_NODES;
/// Default hydrated/projected attribute-value count.
#[napi]
pub const DEFAULT_QUERY_ATTRIBUTE_VALUES: u64 = type_bridge_orm::DEFAULT_QUERY_ATTRIBUTE_VALUES;
/// Default collection members, including reduction cells.
#[napi]
pub const DEFAULT_QUERY_COLLECTION_MEMBERS: u64 = type_bridge_orm::DEFAULT_QUERY_COLLECTION_MEMBERS;
/// Default relation-role-player references.
#[napi]
pub const DEFAULT_QUERY_ROLE_PLAYERS: u64 = type_bridge_orm::DEFAULT_QUERY_ROLE_PLAYERS;
/// Default provider statements per terminal.
#[napi]
pub const DEFAULT_QUERY_STATEMENTS: u64 = type_bridge_orm::DEFAULT_QUERY_STATEMENTS as u64;

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
    fn omitted_answer_dimensions_take_the_exported_core_defaults() {
        let resources =
            NodeQueryExecutionResources::new(None, None, None, None, None, None, None, None)
                .expect("the core defaults are valid")
                .inner();
        assert_eq!(
            resources,
            type_bridge_orm::QueryExecutionResourceLimits::default()
        );
        assert_eq!(
            resources.timeout_milliseconds,
            DEFAULT_QUERY_TIMEOUT_MILLISECONDS
        );
        assert_eq!(resources.items, DEFAULT_QUERY_ITEMS);
        assert_eq!(resources.bytes, DEFAULT_QUERY_BYTES);
        assert_eq!(resources.graph_nodes, DEFAULT_QUERY_GRAPH_NODES);
        assert_eq!(resources.attribute_values, DEFAULT_QUERY_ATTRIBUTE_VALUES);
        assert_eq!(
            resources.collection_members,
            DEFAULT_QUERY_COLLECTION_MEMBERS
        );
        assert_eq!(resources.role_players, DEFAULT_QUERY_ROLE_PLAYERS);
        assert_eq!(u64::from(resources.statements), DEFAULT_QUERY_STATEMENTS);
    }
}
