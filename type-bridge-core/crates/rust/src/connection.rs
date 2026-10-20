//! One connection, direct or proxy, chosen by configuration.
//!
//! A program that opens a [`Connection`] and runs its queries through
//! [`Connection::query`] runs unchanged against TypeDB or a TypeBridge server;
//! only the [`ConnectionPolicy`] differs.

use type_bridge_orm::{AnswerCancellation, QueryExecutionResourceLimits};

use crate::Result;
use crate::query::QuerySession;
use crate::remote::{ProxyConnectionPolicy, ProxyDatabase};
use crate::schema::Schema;
use crate::session::Database;

/// Where a connection's queries run.
#[derive(Debug)]
pub enum ConnectionPolicy {
    /// Directly against TypeDB.
    #[cfg(feature = "typedb")]
    Direct(type_bridge_orm::DirectConnectionPolicy),
    /// Through a TypeBridge server.
    Proxy(ProxyConnectionPolicy),
}

#[cfg(feature = "typedb")]
impl From<type_bridge_orm::DirectConnectionPolicy> for ConnectionPolicy {
    fn from(policy: type_bridge_orm::DirectConnectionPolicy) -> Self {
        Self::Direct(policy)
    }
}

impl From<ProxyConnectionPolicy> for ConnectionPolicy {
    fn from(policy: ProxyConnectionPolicy) -> Self {
        Self::Proxy(policy)
    }
}

/// A schema-bound connection opened by [`crate::SchemaPackage::connect`].
///
/// Queries behave the same on both variants. Match on the variant only for
/// what one of them alone offers, such as CRUD managers on a direct
/// [`Database`].
pub enum Connection<S: Schema> {
    /// Directly against TypeDB.
    Direct(Database<S>),
    /// Through a TypeBridge server.
    Proxy(ProxyDatabase<S>),
}

impl<S: Schema> std::fmt::Debug for Connection<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Direct(_) => "Connection::Direct",
            Self::Proxy(_) => "Connection::Proxy",
        })
    }
}

impl<S: Schema> Connection<S> {
    /// Start one query session under this connection's answer-limit ceiling.
    pub fn query(&self) -> Result<QuerySession<'_, S>> {
        self.query_with_resources(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }

    /// Start one query session whose resources are `resources` constrained by
    /// this connection's answer-limit ceiling.
    pub fn query_with_resources(
        &self,
        resources: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Result<QuerySession<'_, S>> {
        match self {
            Self::Direct(database) => database.query_with_resources(resources, cancellation),
            Self::Proxy(proxy) => proxy.query_with_resources(resources, cancellation),
        }
    }
}
