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
/// Queries and generated CRUD managers select their execution target from
/// the connection policy and behave the same on both variants.
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
    /// Open an explicit raw context on the configured direct or proxy route.
    pub async fn raw_transaction(
        &self,
        kind: crate::raw::TransactionKind,
    ) -> std::result::Result<crate::RawTransaction, crate::RawQueryError> {
        match self {
            Self::Direct(database) => database.raw_transaction(kind).await,
            Self::Proxy(proxy) => proxy.raw_transaction(kind).await,
        }
    }

    /// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
    /// to TypeDB as written, and TypeDB's output comes back as it
    /// answered, in this SDK's native data types. TypeBridge adds no
    /// validation, rewriting, hydration, hooks, limits or extra
    /// queries. It runs in read, write and schema transactions and
    /// accepts any TypeQL, schema-breaking changes included. The caller
    /// owns its correctness; TypeBridge makes no promise about its
    /// effects. On a proxy connection it posts to the server's opt-in
    /// `/v3/raw` endpoint, which is off by default.
    pub async fn execute_query(
        &self,
        query: &str,
        kind: crate::raw::TransactionKind,
    ) -> std::result::Result<serde_json::Value, crate::RawQueryError> {
        self.raw_transaction(kind).await?.execute_one(query).await
    }

    /// Create an exact entity manager on the configured connection target.
    pub fn entities<M>(&self) -> crate::entity_manager::EntityManager<'_, S, M>
    where
        M: crate::__codegen::EntityModel<Schema = S>,
    {
        match self {
            Self::Direct(database) => database.entities(),
            Self::Proxy(proxy) => proxy.entities(),
        }
    }

    /// Create an exact relation manager on the configured connection target.
    pub fn relations<M>(&self) -> crate::relation_manager::RelationManager<'_, S, M>
    where
        M: crate::__codegen::RelationModel<Schema = S>,
    {
        match self {
            Self::Direct(database) => database.relations(),
            Self::Proxy(proxy) => proxy.relations(),
        }
    }

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
