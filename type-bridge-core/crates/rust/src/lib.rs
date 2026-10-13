//! Public Rust client library for TypeBridge.
//!
//! Provides top-level [`Database`], [`ConnectionOptions`], [`SchemaPackage`],
//! and owner-branded schema/model primitives.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate self as type_bridge;

pub mod __codegen;
pub mod aggregate;
mod canonical_codec;
mod connection;
#[allow(dead_code)]
mod entity_codec;
mod entity_manager;
pub mod error;
pub mod hooks;
pub mod migration;
pub mod model;
mod projected_batch;
mod projected_codec;
mod projected_filter;
mod query;
#[allow(dead_code)]
mod relation_codec;
mod relation_manager;
mod remote;
pub mod schema;
pub mod session;
mod transaction;
pub mod value;

pub use canonical_codec::{CanonicalCodecLimits, CanonicalCodecOptions};
pub use connection::{Connection, ConnectionPolicy};
pub use entity_manager::{EntityManager, EntitySubtypeManager};
pub use error::{
    Error, ErrorCategory, ErrorDetail, ErrorDiagnostic, ErrorPathSegment, ModelValidationPhase,
    QueryDiagnosticCategory, QueryDiagnosticPathKind, Result,
};
pub use hooks::{
    CrudOperation, HookContext, HookError, HookFuture, LifecycleHook, ModelKind, PreHookResult,
};
pub use migration::{
    MigrationApprovalBuilder, MigrationApprovalSet, MigrationCatalog, MigrationHistoryEntry,
    MigrationPlan, MigrationPreview, MigrationPreviewEntry,
};
pub use projected_filter::{
    ProjectedEntityFilter, ProjectedRelationFilter, ReadEntityManager, ReadRelationManager,
};
pub use query::{
    Binding, BoundField, BoundRole, Collected, Exact, FieldGroupedQuery, FunctionArgument,
    FunctionCall, FunctionInput, FunctionScalarArgument, GroupedQuery, NamedSelection, Order,
    OrderedOperand, Page, PageOptions, Predicate, Query, QueryOperand, QuerySession, RowsOptions,
    Selectable, SelectedRowSpec, SelectedShape, SelectedSlot, SelectionMode, SingularSelectedShape,
    Subtypes,
};
pub use relation_manager::{RelationManager, RelationSubtypeManager};
pub use remote::{ProxyConnectionPolicy, ProxyDatabase, ProxyExchange};
#[allow(deprecated)]
pub use remote::{
    RemoteConnectionOptions, RemoteDatabase, RemoteQueryLimits, RemoteQueryTransport,
};
pub use schema::{Schema, SchemaPackage, Unbound};
pub use session::{ConnectionOptions, Database, DatabaseCreateOutcome, DatabaseDeleteOutcome};
#[cfg(feature = "typedb")]
pub use session::{
    ManagedDatabaseDeleteOutcome, ManagedDatabaseDeletionPlan, ManagedDatabasePairState,
};
pub use transaction::{
    ReadTransaction, TransactionEntityManager, TransactionRelationManager, WriteTransaction,
};
pub use type_bridge_contract::limits::{QueryLimits, QueryLimitsError};
#[cfg(feature = "typedb")]
pub use type_bridge_orm::DirectConnectionPolicy;
pub use type_bridge_orm::{
    AnswerCancellation, AnswerTransport, DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES,
    DEFAULT_DIRECT_QUERY_BYTES, DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS,
    DEFAULT_DIRECT_QUERY_GRAPH_NODES, DEFAULT_DIRECT_QUERY_ITEMS,
    DEFAULT_DIRECT_QUERY_ROLE_PLAYERS, DEFAULT_DIRECT_QUERY_STATEMENTS,
    DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS, DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES,
    DEFAULT_PROXY_QUERY_BYTES, DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS,
    DEFAULT_PROXY_QUERY_GRAPH_NODES, DEFAULT_PROXY_QUERY_ITEMS, DEFAULT_PROXY_QUERY_ROLE_PLAYERS,
    DEFAULT_PROXY_QUERY_STATEMENTS, DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS, MAX_QUERY_STATEMENTS,
    MAX_QUERY_TIMEOUT_MILLISECONDS, ProjectedManagerComparison, QueryExecutionDeadline,
    QueryExecutionLimitOutOfRange, QueryExecutionLimitsError, QueryExecutionResourceLimits,
};
#[allow(deprecated)]
pub use type_bridge_orm::{
    DirectTls, MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES, MAX_QUERY_COLLECTION_MEMBERS,
    MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS, MAX_QUERY_ROLE_PLAYERS,
};
pub use type_bridge_orm::{ProxyEndpoint, TlsPolicy};
pub use type_bridge_orm_derive::SelectedRow;
#[cfg(feature = "typedb")]
pub use type_bridge_schema_migration::{
    MAX_MIGRATION_BACKFILL_OBSERVATIONS, MAX_MIGRATION_EXECUTION_GROUPS,
    MigrationBackfillObservation, MigrationCancellation, MigrationDriftFinding,
    MigrationExecutionControl, MigrationExecutionDirection, MigrationExecutionReport,
    MigrationExecutionReportPosition, MigrationExecutionResourceLimits, MigrationExecutionStatus,
    MigrationVerifyReport,
};
