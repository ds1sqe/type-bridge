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
#[path = "codegen/mod.rs"]
pub mod _codegen;
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
pub mod error;
mod execution_diagnostic;
pub mod expr;
pub mod filter;
pub mod hooks;
pub mod match_request;
pub mod migration_assertion;
pub mod projected_batch;
mod projected_batch_executor;
pub mod projected_crud;
mod projected_manager_filter;
pub mod projected_model;
mod projected_query;
pub mod provider_runtime;
pub mod query;
mod query_execution_limits;
pub mod query_v2;
mod query_v2_adapter;
#[cfg(test)]
mod query_v2_adapter_tests;
pub mod query_v2_builder;
mod query_v2_compatibility;
mod query_v2_model;
mod query_v2_model_remote;
pub mod query_v2_prepared;
pub mod query_v2_remote;
pub mod runtime_projection;
pub mod session;
pub mod value;

/// Integration-test bridge for exercising the production-internal V1-to-V2
/// adapter from the live integration-test crate.
///
/// This public test seam exists only when the explicitly test-only
/// `integration-tests` feature is enabled. The adapter itself is compiled in
/// normal/release builds but remains crate-private.
#[cfg(feature = "integration-tests")]
#[doc(hidden)]
pub mod integration_test_support {
    use type_bridge_contract::diagnostic::Diagnostic;
    use type_bridge_contract::limits::StructuralLimits;
    use type_bridge_contract::query_plan::QueryOperation;
    use type_bridge_query::ValidatedQuery;

    use crate::_registry::DescriptorRegistry;
    use crate::match_request::ValidatedMatchRequest;
    use crate::query_v2::failure;
    use crate::query_v2_adapter::{
        MatchRequestAdaptation, MatchRequestAdapterAuthority, adapt_match_request,
    };

    /// Adapt a validated V1 request through the production registry authority.
    ///
    /// Deriving the authority here keeps the live parity gate on the same
    /// descriptor projection as public execution and proves that its V2 side
    /// cannot silently take the retained `LegacyRequired` fallback.
    pub fn adapt_match_request_for_live_test(
        validated: &ValidatedMatchRequest,
        registry: &DescriptorRegistry,
        limits: StructuralLimits,
    ) -> Result<(ValidatedQuery, QueryOperation), Diagnostic> {
        let authority = MatchRequestAdapterAuthority::from_registry(registry)?;
        match adapt_match_request(validated, registry, &authority.context(), limits)? {
            MatchRequestAdaptation::Adapted(adapted) => {
                Ok((adapted.validated().clone(), adapted.operation()))
            }
            MatchRequestAdaptation::LegacyRequired(_) => Err(failure(
                type_bridge_contract::diagnostic::DiagnosticCategory::ResourceLimit,
                "query_v2_adapter_test_resource_envelope",
                "the live V2 parity fixture cannot fit the canonical V2 artifact envelope",
            )),
        }
    }
}

// Generated/runtime execution exports.
pub use _attribute::ValueType;
pub use _dynamic::{
    DynamicAggregate, DynamicAttributeMap, DynamicComparisonOp, DynamicEntityIdentity,
    DynamicEntityRow, DynamicExpr, DynamicRelationIdentity, DynamicRelationRow, DynamicRolePlayer,
    DynamicRolePlayerInput, DynamicSort,
};
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
#[doc(hidden)]
pub use projected_batch::{PreparedProjectedBatchInvocation, ProjectedBatchInvocationControl};
pub use projected_batch::{
    ProjectedBatch, ProjectedBatchOperation, ProjectedBatchResourceMeasure, ProjectedBatchRow,
};
pub use projected_batch_executor::{ProjectedBatchExecutor, ProjectedBatchResult};
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
};
#[doc(hidden)]
pub use projected_query::{
    MAX_PROJECTED_QUERY_ATTRIBUTE_VALUES, MAX_PROJECTED_QUERY_BYTES, MAX_PROJECTED_QUERY_CELLS,
    MAX_PROJECTED_QUERY_ROWS, MAX_PROJECTED_QUERY_THINGS, ProjectedQueryMaterializationLimits,
    ProjectedQueryOrigin, ProjectedQueryResourceMeasure, ProjectedQueryResult, ProjectedQueryRow,
    ProjectedQuerySlot, ProjectedQuerySlotValue, ProjectedQueryValue, ProjectedReducedValue,
    ProjectedReductionGroup, ProjectedReductionRow, materialize_projected_query_result,
    materialize_projected_query_result_with_budget,
    materialize_projected_query_result_with_cancellation,
};
pub use provider_runtime::ProviderRuntimeOwner;
pub use query::{EntityQuery, GroupByEntityQuery, GroupByRelationQuery, RelationQuery};
pub use query_execution_limits::{
    MAX_QUERY_ATTRIBUTE_VALUES, MAX_QUERY_BYTES, MAX_QUERY_COLLECTION_MEMBERS,
    MAX_QUERY_GRAPH_NODES, MAX_QUERY_ITEMS, MAX_QUERY_ROLE_PLAYERS, MAX_QUERY_STATEMENTS,
    MAX_QUERY_TIMEOUT_MILLISECONDS, QueryExecutionDeadline, QueryExecutionResourceLimits,
};
pub use query_v2_model_remote::{
    ClaimedRemoteModelReplyV2, PendingRemoteModelQueryV2, RemoteModelQueryV2Error,
    prepare_remote_model_query_v2, prepare_remote_model_query_v2_with_budget,
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
pub use session::{
    ConnectOptions, PreparedSecureConnectOptions, SecureConnectError, SecureConnectOptions,
    SecureResult, TlsMode,
};
pub use session::{
    Database, DatabaseConnectionAuthority, GivenRowsSpec, GivenValue, Transaction,
    TransactionContext, TxType, require_legacy_writer_open,
    require_legacy_writer_open_in_transaction,
};
#[cfg(feature = "typedb")]
pub use session::{
    database_exists, database_exists_prepared_secure, database_exists_secure,
    delete_database_prepared_secure, delete_database_secure, ensure_database_exists,
    ensure_database_exists_prepared_secure, ensure_database_exists_secure,
};
/// Provider version type used by binding-owned recording backends.
#[doc(hidden)]
pub use type_bridge_core_lib::version::Version as _ProviderVersion;
pub use value::AttributeValue;
