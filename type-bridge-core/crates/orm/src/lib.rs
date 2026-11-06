//! Shared TypeDB execution engine for generated TypeBridge clients.
//!
//! Public application model construction lives in generated `type-bridge`
//! packages. This crate exposes connection, query, transaction, and verified
//! runtime-projection execution; handwritten model traits, derives, registries,
//! descriptors, managers, and schema authoring are not package-root APIs.

#![deny(missing_docs)]

#[doc(hidden)]
#[path = "attribute.rs"]
pub mod _attribute;
#[doc(hidden)]
#[path = "descriptor.rs"]
pub mod _descriptor;
#[doc(hidden)]
#[path = "dynamic.rs"]
pub mod _dynamic;
#[doc(hidden)]
#[path = "entity.rs"]
pub mod _entity;
#[doc(hidden)]
#[path = "field_ref.rs"]
pub mod _field_ref;
#[doc(hidden)]
#[path = "manager/mod.rs"]
pub mod _manager;
#[doc(hidden)]
#[path = "registry.rs"]
pub mod _registry;
#[doc(hidden)]
#[path = "relation.rs"]
pub mod _relation;
#[doc(hidden)]
#[path = "schema/mod.rs"]
pub mod _schema;
mod answer_meter;
#[doc(hidden)]
pub mod debug;
mod enum_spelling;
pub mod error;
mod execution_diagnostic;
/// Invocation-local phase diagnostics and tracing for engine execution.
pub mod execution_phases;
pub mod expr;
pub mod filter;
pub mod hooks;
#[cfg(test)]
#[path = "../tests/support/live_database.rs"]
mod live_database;
pub mod match_request;
pub mod migration_assertion;
#[cfg(test)]
mod plan_adapter_tests;
pub mod plan_authority;
mod plan_builders;
pub mod plan_explain;
mod plan_lowering;
mod plan_model_remote;
pub mod plan_scalar;
pub mod projected_batch;
mod projected_batch_executor;
pub mod projected_codec;
pub mod projected_crud;
mod projected_manager_filter;
pub mod projected_model;
mod projected_query;
pub mod provider_runtime;
pub mod query;
mod query_connection;
mod query_execution_limits;
mod query_plan_exec;
pub mod remote_signing;
pub mod runtime_projection;
pub mod session;
pub mod value;
mod write_result;

// Generated/runtime execution exports.
pub use _attribute::ValueType;
pub use _dynamic::{
    DynamicAggregate, DynamicAttributeMap, DynamicComparisonOp, DynamicEntityIdentity,
    DynamicEntityRow, DynamicExpr, DynamicRelationIdentity, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput, DynamicSort,
};
pub use enum_spelling::{EnumKind, canonical_enum_spelling};
pub use error::{ClassifiedCommitError, CommitFailureCertainty, OrmError, Result};
#[doc(hidden)]
pub use execution_diagnostic::{
    lower_classified_commit_error, lower_execution_error, lower_match_error,
    lower_remote_query_diagnostic, query_resource_closed_diagnostic,
};
pub use expr::{Agg, AggResult, Expr, GroupByResult, SortDir};
pub use filter::Filter;
pub use hooks::{
    CrudOperation, HookContext, HookError, HookRunner, LifecycleHook, PreHookResult, TypeKind,
};
pub use match_request::*;
pub use plan_model_remote::{
    ClaimedRemoteModelReply, PendingRemoteModelQuery, RemoteModelQueryError,
    prepare_remote_model_query, prepare_remote_model_query_with_budget,
};
#[doc(hidden)]
pub use projected_batch::{PreparedProjectedBatchInvocation, ProjectedBatchInvocationControl};
pub use projected_batch::{
    ProjectedBatch, ProjectedBatchOperation, ProjectedBatchResourceMeasure, ProjectedBatchRow,
};
pub use projected_batch_executor::{ProjectedBatchExecutor, ProjectedBatchResult};
pub use projected_codec::{
    ProjectedCodecError, ProjectedCodecValue, ProjectedStructValue, materialize_record,
    record_from_attribute, record_from_create, record_from_reference, record_from_snapshot,
    record_from_struct,
};
pub use projected_crud::ProjectedCrudExecutor;
#[doc(hidden)]
pub use projected_crud::{
    ProjectedCrudCompatibilityCause, ProjectedCrudCompatibilityFailure,
    ProjectedCrudCompatibilityStage,
};
#[doc(hidden)]
pub use projected_manager_filter::ProjectedManagerFilterInvocationControl;
pub use projected_manager_filter::{
    ProjectedManagerComparison, ProjectedManagerFilter, ProjectedManagerFilterExecutor,
    ProjectedManagerFilterResourceMeasure,
};
pub use projected_model::{
    MAX_PROJECTED_MODEL_BYTES, MAX_PROJECTED_MODEL_MEMBERS, ProjectedAttributeValue,
    ProjectedCreate, ProjectedCreateBudget, ProjectedReference, ProjectedReferenceOrigin,
    ProjectedResourceMeasure, ProjectedRolePlayer, ProjectedThing,
    validate_projected_create_member_count, validate_projected_hydrated_member_count,
};
#[doc(hidden)]
pub use projected_query::{
    ProjectedQueryMaterializationLimits, ProjectedQueryOrigin, ProjectedQueryResourceMeasure,
    ProjectedQueryResult, ProjectedQueryRow, ProjectedQuerySlot, ProjectedQuerySlotValue,
    ProjectedQueryValue, ProjectedReducedValue, ProjectedReductionGroup, ProjectedReductionRow,
    materialize_projected_query_result, materialize_projected_query_result_with_budget,
    materialize_projected_query_result_with_cancellation,
};
pub use provider_runtime::ProviderRuntimeOwner;
pub use query::{EntityQuery, GroupByEntityQuery, GroupByRelationQuery, RelationQuery};
pub use query_connection::{OwnedQueryTarget, ProxyQueryConnection, QueryTarget, QueryTargetError};
pub use query_execution_limits::{
    AnswerTransport, DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES, DEFAULT_DIRECT_QUERY_BYTES,
    DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS, DEFAULT_DIRECT_QUERY_GRAPH_NODES,
    DEFAULT_DIRECT_QUERY_ITEMS, DEFAULT_DIRECT_QUERY_ROLE_PLAYERS, DEFAULT_DIRECT_QUERY_STATEMENTS,
    DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS, DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES,
    DEFAULT_PROXY_QUERY_BYTES, DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS,
    DEFAULT_PROXY_QUERY_GRAPH_NODES, DEFAULT_PROXY_QUERY_ITEMS, DEFAULT_PROXY_QUERY_ROLE_PLAYERS,
    DEFAULT_PROXY_QUERY_STATEMENTS, DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS, MAX_QUERY_STATEMENTS,
    QueryExecutionDeadline, QueryExecutionLimitOutOfRange, QueryExecutionLimitsError,
    QueryExecutionResourceLimits, QueryTimeout,
};
pub use runtime_projection::InstalledRuntimeProjection;
#[doc(hidden)]
pub use session::TransactionContextState;
pub use session::backend::AnswerCancellation;
#[doc(hidden)]
pub use session::database::is_identity_safe_provider_address;
#[cfg(feature = "typedb")]
pub use session::embedded_driver_versions;
#[cfg(feature = "typedb")]
#[allow(deprecated)]
pub use session::{
    ConnectOptions, DirectConnectionPolicy, PreparedSecureConnectOptions, SecureConnectError,
    SecureConnectOptions, SecureResult, TlsMode,
};
#[allow(deprecated)]
pub use session::{
    Database, DatabaseConnectionAuthority, GivenColumnType, GivenRowsSpec, GivenValue,
    OwnedTransaction, ProxyEndpoint, ProxyHttpTransport, TlsPolicy, Transaction,
    TransactionContext, TxType, require_legacy_writer_open,
    require_legacy_writer_open_in_transaction,
};
#[cfg(feature = "typedb")]
#[allow(deprecated)]
pub use session::{
    database_exists, database_exists_prepared_secure, database_exists_secure,
    delete_database_prepared_secure, delete_database_secure, ensure_database_exists,
    ensure_database_exists_prepared_secure, ensure_database_exists_secure,
};
pub use type_bridge_contract::enum_spelling::UnknownEnumValue;
pub use type_bridge_contract::projection::{
    GeneratedManagerLookup, ManagerLookup, OwnerLookup, resolve_generated_manager_lookup,
};
/// Provider version type used by binding-owned recording backends.
#[doc(hidden)]
pub use type_bridge_core_lib::version::Version as _ProviderVersion;
pub use value::AttributeValue;
pub use write_result::{BatchWriteResult, WriteResult};

#[cfg(test)]
#[path = "../tests/support/native_batch.rs"]
mod native_batch_fixture;
#[cfg(test)]
mod native_batch_answer_item {
    pub use crate::session::backend::{AnswerItem, QueryResult};
}
