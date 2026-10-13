//! The 2.2 proxy names, kept until 3.0. Each builds its 2.3 replacement and
//! adds no behaviour.

use std::future::Future;
use std::pin::Pin;

use type_bridge_orm::{
    DEFAULT_PROXY_QUERY_STATEMENTS, DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS,
    QueryExecutionResourceLimits,
};

use super::{ProxyConnectionPolicy, ProxyDatabase, ProxyExchange};
use crate::Result;
use crate::schema::Unbound;

/// The 2.2 name of [`ProxyDatabase`].
#[deprecated(since = "2.3.0", note = "use ProxyDatabase; removed in 3.0")]
pub type RemoteDatabase<S = Unbound> = ProxyDatabase<S>;

/// The 2.2 name of [`ProxyExchange`]. Every implementation is a
/// [`ProxyExchange`].
#[deprecated(since = "2.3.0", note = "use ProxyExchange; removed in 3.0")]
pub trait RemoteQueryTransport: Send + Sync + 'static {
    /// Fetch the executor's exact signed capability advertisement.
    fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>>;

    /// Exchange one exact canonical request for one exact signed reply.
    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>>;
}

#[allow(deprecated)]
impl<T: RemoteQueryTransport> ProxyExchange for T {
    fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>> {
        RemoteQueryTransport::capabilities(self)
    }

    fn exchange<'a>(
        &'a self,
        request: &'a [u8],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        RemoteQueryTransport::exchange(self, request)
    }
}

/// The 2.2 constructors of [`ProxyConnectionPolicy`]. Both return a
/// [`ProxyConnectionPolicy`]; the 2.2 type name itself is not kept, since
/// `ProxyConnectionPolicy::new` now takes an endpoint.
#[deprecated(since = "2.3.0", note = "use ProxyConnectionPolicy; removed in 3.0")]
#[derive(Debug)]
pub enum RemoteConnectionOptions {}

#[allow(deprecated)]
impl RemoteConnectionOptions {
    /// `ProxyConnectionPolicy::with_exchange(transport).answer_limits(limits)
    /// .authority(scope, semantic_profile)`.
    #[deprecated(
        since = "2.3.0",
        note = "use ProxyConnectionPolicy::with_exchange(transport).answer_limits(limits)\
                .authority(scope, semantic_profile); removed in 3.0"
    )]
    #[must_use]
    // The 2.2 constructor name; it now returns the replacement type.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        scope: impl Into<String>,
        semantic_profile: impl Into<String>,
        limits: impl Into<QueryExecutionResourceLimits>,
        transport: impl ProxyExchange,
    ) -> ProxyConnectionPolicy {
        Self::generated(limits, transport).authority(scope, semantic_profile)
    }

    /// `ProxyConnectionPolicy::with_exchange(transport).answer_limits(limits)`.
    #[deprecated(
        since = "2.3.0",
        note = "use ProxyConnectionPolicy::with_exchange(transport).answer_limits(limits); \
                removed in 3.0"
    )]
    #[must_use]
    pub fn generated(
        limits: impl Into<QueryExecutionResourceLimits>,
        transport: impl ProxyExchange,
    ) -> ProxyConnectionPolicy {
        ProxyConnectionPolicy::with_exchange(transport).answer_limits(limits.into())
    }
}

/// The 2.2 answer budget: [`QueryExecutionResourceLimits`] built from the
/// 2.2 constructor arguments, with the proxy-default timeout and statements.
#[deprecated(since = "2.3.0", note = "use QueryExecutionResourceLimits; removed in 3.0")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteQueryLimits {
    resources: QueryExecutionResourceLimits,
}

#[allow(deprecated)]
impl RemoteQueryLimits {
    /// Construct the limits with the matching
    /// [`QueryExecutionResourceLimits`] fields.
    #[must_use]
    pub const fn new(
        max_items: u64,
        max_bytes: u64,
        max_collection_members: u64,
        max_graph_nodes: u64,
        max_attribute_values: u64,
        max_role_players: u64,
    ) -> Self {
        Self {
            resources: QueryExecutionResourceLimits {
                timeout_milliseconds: DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS,
                items: max_items,
                bytes: max_bytes,
                graph_nodes: max_graph_nodes,
                attribute_values: max_attribute_values,
                collection_members: max_collection_members,
                role_players: max_role_players,
                statements: DEFAULT_PROXY_QUERY_STATEMENTS,
            },
        }
    }

    /// Set `timeout_milliseconds`.
    #[must_use]
    pub const fn deadline_ms(mut self, deadline_ms: u64) -> Self {
        self.resources.timeout_milliseconds = deadline_ms;
        self
    }
}

#[allow(deprecated)]
impl From<RemoteQueryLimits> for QueryExecutionResourceLimits {
    fn from(limits: RemoteQueryLimits) -> Self {
        limits.resources
    }
}

#[allow(deprecated)]
impl From<QueryExecutionResourceLimits> for RemoteQueryLimits {
    fn from(resources: QueryExecutionResourceLimits) -> Self {
        Self { resources }
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;

    struct Offline;

    impl RemoteQueryTransport for Offline {
        fn capabilities(&self) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + '_>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn exchange<'a>(
            &'a self,
            _request: &'a [u8],
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn fields(
        policy: &ProxyConnectionPolicy,
    ) -> (
        Option<&str>,
        Option<&str>,
        QueryExecutionResourceLimits,
        QueryExecutionResourceLimits,
        bool,
    ) {
        (
            policy.scope.as_deref(),
            policy.semantic_profile.as_deref(),
            policy.connection_limits,
            policy.answer_limits,
            policy.advertisement.is_some(),
        )
    }

    #[test]
    fn remote_connection_options_build_the_matching_policy() {
        let limits = RemoteQueryLimits::new(100, 8 << 20, 1_000, 1_000, 1_000, 1_000)
            .deadline_ms(30_000);
        let resources = QueryExecutionResourceLimits::from(limits);
        assert_eq!(
            resources,
            QueryExecutionResourceLimits {
                timeout_milliseconds: 30_000,
                items: 100,
                bytes: 8 << 20,
                graph_nodes: 1_000,
                attribute_values: 1_000,
                collection_members: 1_000,
                role_players: 1_000,
                statements: DEFAULT_PROXY_QUERY_STATEMENTS,
            }
        );

        let generated = RemoteConnectionOptions::generated(limits, Offline);
        let expected = ProxyConnectionPolicy::with_exchange(Offline).answer_limits(resources);
        assert_eq!(fields(&generated), fields(&expected));
        assert!(matches!(generated.route, super::super::ProxyRoute::Exchange(_)));

        let scoped = RemoteConnectionOptions::new("scope", "profile", limits, Offline);
        let expected = ProxyConnectionPolicy::with_exchange(Offline)
            .answer_limits(resources)
            .authority("scope", "profile");
        assert_eq!(fields(&scoped), fields(&expected));
        assert_eq!(fields(&scoped).0, Some("scope"));
        assert_eq!(fields(&scoped).1, Some("profile"));
    }
}
