//! Other deprecated aliases retained until their scheduled removal.
#![allow(deprecated)]

#[test]
fn direct_tls_is_tls_policy() {
    let _: type_bridge::TlsPolicy = type_bridge::DirectTls::native_roots();
    let _: type_bridge_orm::TlsPolicy = type_bridge_orm::DirectTls::disabled();
    let _: type_bridge_orm::session::TlsPolicy = type_bridge_orm::session::DirectTls::disabled();
}

#[test]
fn max_query_constants_keep_their_2_2_values() {
    let pairs = [
        (
            type_bridge::MAX_QUERY_ITEMS,
            type_bridge::DEFAULT_PROXY_QUERY_ITEMS,
        ),
        (
            type_bridge::MAX_QUERY_BYTES,
            type_bridge::DEFAULT_PROXY_QUERY_BYTES,
        ),
        (
            type_bridge::MAX_QUERY_GRAPH_NODES,
            type_bridge::DEFAULT_PROXY_QUERY_GRAPH_NODES,
        ),
        (
            type_bridge::MAX_QUERY_ATTRIBUTE_VALUES,
            type_bridge::DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES,
        ),
        (
            type_bridge::MAX_QUERY_COLLECTION_MEMBERS,
            type_bridge::DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS,
        ),
        (
            type_bridge::MAX_QUERY_ROLE_PLAYERS,
            type_bridge::DEFAULT_PROXY_QUERY_ROLE_PLAYERS,
        ),
        (
            type_bridge_orm::MAX_QUERY_ITEMS,
            type_bridge_orm::DEFAULT_PROXY_QUERY_ITEMS,
        ),
        (
            type_bridge_orm::MAX_QUERY_BYTES,
            type_bridge_orm::DEFAULT_PROXY_QUERY_BYTES,
        ),
    ];
    for (alias, default) in pairs {
        assert_eq!(alias, default);
    }
    // The 2.2.4 values: the canonical collection length and the 32 MiB
    // remote envelope.
    assert_eq!(type_bridge::MAX_QUERY_ITEMS, 65_536);
    assert_eq!(type_bridge::MAX_QUERY_BYTES, 32 * 1024 * 1024);
}
