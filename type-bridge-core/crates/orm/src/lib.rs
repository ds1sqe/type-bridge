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
pub mod expr;
pub mod filter;
pub mod hooks;
pub mod match_request;
pub mod migration_assertion;
pub mod provider_runtime;
pub mod query;
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
            MatchRequestAdaptation::NativeOnly => Err(failure(
                type_bridge_contract::diagnostic::DiagnosticCategory::UnsupportedCapability,
                "query_v2_adapter_test_native_only",
                "the live V2 parity fixture has no V2 spelling for this operation",
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
pub use expr::{Agg, AggResult, Expr, GroupByResult, SortDir};
pub use filter::Filter;
pub use hooks::{
    CrudOperation, HookContext, HookError, HookRunner, LifecycleHook, PreHookResult, TypeKind,
};
pub use match_request::*;
pub use provider_runtime::ProviderRuntimeOwner;
pub use query::{EntityQuery, GroupByEntityQuery, GroupByRelationQuery, RelationQuery};
pub use query_v2_model_remote::{
    ClaimedRemoteModelReplyV2, PendingRemoteModelQueryV2, RemoteModelQueryV2Error,
    prepare_remote_model_query_v2,
};
pub use runtime_projection::InstalledRuntimeProjection;
pub use session::backend::AnswerCancellation;
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
pub use value::AttributeValue;
