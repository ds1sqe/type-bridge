//! The 2.2 proxy names keep compiling, and running, until 3.0.
#![allow(deprecated)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use type_bridge::{
    RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits, RemoteQueryTransport,
};

/// A 2.2 transport that serves a signed advertisement and refuses queries.
struct OfflineTransport {
    advertisement: Vec<u8>,
    capability_fetches: Arc<AtomicUsize>,
}

impl OfflineTransport {
    fn new() -> (Self, Arc<AtomicUsize>) {
        use type_bridge_contract::query_plan::query_plan_v2_capability_vocabulary;
        use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
        use type_bridge_contract::query_remote_v2::query_remote_v2_required_capabilities;
        use type_bridge_orm::query_v2_remote::RemoteReplySigningKey;

        let signer = RemoteReplySigningKey::from_secret_bytes([0x52; 32]);
        let mut capabilities = query_plan_v2_capability_vocabulary();
        for capability in query_remote_v2_required_capabilities(true) {
            capabilities.insert(capability);
        }
        let advertisement = RemoteCapabilities::new(
            capabilities,
            RemoteExecutorBinding::new("deprecated-aliases", "epoch-00000000001").unwrap(),
            signer.public_key(),
        )
        .encode()
        .unwrap();
        let capability_fetches = Arc::new(AtomicUsize::new(0));
        (
            Self {
                advertisement,
                capability_fetches: Arc::clone(&capability_fetches),
            },
            capability_fetches,
        )
    }
}

impl RemoteQueryTransport for OfflineTransport {
    fn capabilities(
        &self,
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + '_>> {
        self.capability_fetches.fetch_add(1, Ordering::SeqCst);
        let advertisement = self.advertisement.clone();
        Box::pin(async move { Ok(advertisement) })
    }

    fn exchange<'a>(
        &'a self,
        _request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = type_bridge::Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async {
            Err(type_bridge::Error::Other {
                message: "offline transport".into(),
                source: None,
            })
        })
    }
}

// The 2.2.4 Rust guide's remote example up to binding a schema; the rest
// needs a generated package and is compiled by the schema-codegen
// acceptance consumer.
#[tokio::test]
async fn guide_2_2_remote_example_connects() -> type_bridge::Result<()> {
    let (transport, capability_fetches) = OfflineTransport::new();

    let options = RemoteConnectionOptions::generated(
        RemoteQueryLimits::new(100, 8 << 20, 1_000, 1_000, 1_000, 1_000).deadline_ms(30_000),
        transport,
    );
    let remote: RemoteDatabase = RemoteDatabase::connect(options).await?;

    let _: &type_bridge::ProxyDatabase = &remote;
    assert_eq!(capability_fetches.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn remote_query_limits_convert_to_resource_limits() {
    let limits = RemoteQueryLimits::new(100, 8 << 20, 1_000, 2_000, 3_000, 4_000).deadline_ms(5);
    assert_eq!(
        type_bridge::QueryExecutionResourceLimits::from(limits),
        type_bridge::QueryExecutionResourceLimits {
            timeout_milliseconds: 5,
            items: 100,
            bytes: 8 << 20,
            collection_members: 1_000,
            graph_nodes: 2_000,
            attribute_values: 3_000,
            role_players: 4_000,
            statements: type_bridge::DEFAULT_PROXY_QUERY_STATEMENTS,
        }
    );
}

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
