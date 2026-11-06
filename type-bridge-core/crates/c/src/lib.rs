//! Versioned native C boundary for generated TypeBridge schema packages.

// Database one-shot calls await the deeper staged::Transaction::commit future.
#![recursion_limit = "256"]
#![deny(missing_docs)]

mod abi;
mod allocation;
mod canonical_archive;
mod crud_v2;
mod diagnostic;
mod entity_crud;
mod execution_diagnostic;
mod generated_preflight;
mod iid;
mod manager_create;
mod manager_filter;
mod manager_read;
mod manager_returning;
mod manager_target;
mod manager_write;
mod migration_abi;
mod migration_user_steps;
pub use migration_user_steps::TypeBridgeMigrationCallback;
mod migration_runtime;
mod perf_stages;
mod plan_builder;
mod plan_results;
mod policy;
mod projected_batch;
mod projected_create_builder;
mod projected_model;
mod projected_token;
mod projected_value;
mod query;
mod raw;
mod runtime;
mod schema_package;
#[cfg(test)]
#[path = "../../../../tests/support/sdk_profile_matrix.rs"]
#[allow(dead_code)]
mod sdk_profile_matrix;
mod staged;
mod staged_answers;
mod staged_terminals;
mod thing_crud;

pub use abi::{
    TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION, TypeBridgeByteView, TypeBridgeChunkedByteViewV1,
    TypeBridgeDiagnostics, TypeBridgeSchemaPackage, TypeBridgeSchemaPackageChunkedDescriptorV1,
    TypeBridgeSchemaPackageDescriptorV1, TypeBridgeStatus,
};
pub use canonical_archive::{
    TypeBridgeCanonicalArchive, TypeBridgeCanonicalArchiveBuilder, TypeBridgeCanonicalBytes,
    TypeBridgeProjectedCodecOptionsV1, TypeBridgeProjectedStruct, TypeBridgeProjectedStructMember,
};
pub use execution_diagnostic::{
    TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticDetailKind,
    TypeBridgeExecutionDiagnosticDetailViewV1, TypeBridgeExecutionDiagnosticPathKind,
    TypeBridgeExecutionDiagnosticPathViewV1, TypeBridgeExecutionDiagnosticViewV1,
    TypeBridgeExecutionDiagnostics, TypeBridgeExecutionFailureV1, TypeBridgeExecutionPhaseViewV1,
    TypeBridgeExecutionTimelineViewV1,
};
pub use generated_preflight::{
    GENERATED_CREATE_GRAPH_VERSION, GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX,
    GENERATED_CREATE_MEMBER_COUNT_MAX, GENERATED_CREATE_MEMBER_REFERENCE_SCALAR,
    GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE, GENERATED_CREATE_MEMBER_VALUE_SCALAR,
    GENERATED_CREATE_MEMBER_VALUE_SEQUENCE, GENERATED_INPUT_BYTES, GENERATED_INPUT_CANCELLATION,
    GENERATED_INPUT_CANONICAL_ARCHIVE, GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER,
    GENERATED_INPUT_CANONICAL_BYTES, GENERATED_INPUT_CREATE_ARGS_GRAPH, GENERATED_INPUT_DATABASE,
    GENERATED_INPUT_DIAGNOSTICS, GENERATED_INPUT_EXECUTION_DIAGNOSTICS,
    GENERATED_INPUT_MANAGER_FILTER, GENERATED_INPUT_MANAGER_RESULT, GENERATED_INPUT_PLAN_BUILDER,
    GENERATED_INPUT_PLAN_OPTIONS, GENERATED_INPUT_PROJECTED_BATCH,
    GENERATED_INPUT_PROJECTED_BATCH_BUILDER, GENERATED_INPUT_PROJECTED_BATCH_RESULT,
    GENERATED_INPUT_PROJECTED_CODEC_OPTIONS, GENERATED_INPUT_PROJECTED_CREATE,
    GENERATED_INPUT_PROJECTED_REFERENCE, GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY,
    GENERATED_INPUT_PROJECTED_STRUCT, GENERATED_INPUT_PROJECTED_STRUCT_MEMBER,
    GENERATED_INPUT_PROJECTED_THING, GENERATED_INPUT_PROJECTED_TOKEN,
    GENERATED_INPUT_PROJECTED_VALUE, GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
    GENERATED_INPUT_PROXY_CONNECTION, GENERATED_INPUT_QUERY, GENERATED_INPUT_QUERY_BINDING,
    GENERATED_INPUT_QUERY_FIELD, GENERATED_INPUT_QUERY_FUNCTION,
    GENERATED_INPUT_QUERY_FUNCTION_CALL, GENERATED_INPUT_QUERY_FUNCTION_VALUE,
    GENERATED_INPUT_QUERY_ORDER, GENERATED_INPUT_QUERY_PREDICATE,
    GENERATED_INPUT_QUERY_REMOTE_CLAIM, GENERATED_INPUT_QUERY_REMOTE_CONTEXT,
    GENERATED_INPUT_QUERY_REMOTE_PENDING, GENERATED_INPUT_QUERY_RESULT, GENERATED_INPUT_QUERY_ROLE,
    GENERATED_INPUT_QUERY_SELECTION, GENERATED_INPUT_QUERY_SESSION, GENERATED_INPUT_QUERY_TERMINAL,
    GENERATED_INPUT_READ_TRANSACTION, GENERATED_INPUT_RUNTIME, GENERATED_INPUT_SCHEMA_PACKAGE,
    GENERATED_INPUT_WRITE_TRANSACTION, TypeBridgeGeneratedCreateArgsGraphV1,
    TypeBridgeGeneratedCreateHandleChunkV1, TypeBridgeGeneratedCreateMemberV1,
    TypeBridgeGeneratedOpaqueInputV1, TypeBridgeGeneratedOutputRangeV1,
};
pub use policy::TypeBridgeDatabaseConfig;
pub use projected_create_builder::TypeBridgeProjectedCreateBuilder;
pub use projected_model::{
    TypeBridgeProjectedCreate, TypeBridgeProjectedCreateDescriptorV1,
    TypeBridgeProjectedFieldInputV1, TypeBridgeProjectedReference,
    TypeBridgeProjectedReferenceDescriptorV1, TypeBridgeProjectedRoleInputV1,
    TypeBridgeProjectedThing, TypeBridgeProjectedThingDescriptorV1,
};
pub use projected_token::TypeBridgeProjectedTokenV1;
pub use projected_value::{TypeBridgeProjectedValue, TypeBridgeProjectedValueKind};
pub use query::{
    TypeBridgeProxyConnection, TypeBridgeProxyConnectionOptionsV1, TypeBridgeProxyHeaderV1,
    TypeBridgeQuery, TypeBridgeQueryBinding, TypeBridgeQueryDescriptorV1,
    TypeBridgeQueryExecutionLimitsV1, TypeBridgeQueryField, TypeBridgeQueryFieldReferenceV1,
    TypeBridgeQueryFunction, TypeBridgeQueryFunctionArgumentMemberV1,
    TypeBridgeQueryFunctionArgumentV1, TypeBridgeQueryFunctionArgumentsGraphV1,
    TypeBridgeQueryFunctionArgumentsHeaderV1, TypeBridgeQueryFunctionCall,
    TypeBridgeQueryFunctionValue, TypeBridgeQueryOrder, TypeBridgeQueryOrderDescriptorV1,
    TypeBridgeQueryPageMetadataV1, TypeBridgeQueryPredicate, TypeBridgeQueryReducedValueMetadataV1,
    TypeBridgeQueryReducerV1, TypeBridgeQueryRemoteClaim, TypeBridgeQueryRemoteContext,
    TypeBridgeQueryRemotePending, TypeBridgeQueryResult, TypeBridgeQueryRole,
    TypeBridgeQuerySelection, TypeBridgeQuerySelectionDescriptorV1, TypeBridgeQuerySession,
    TypeBridgeQueryShapeSlotV1, TypeBridgeQueryTerminal, TypeBridgeQueryTerminalDescriptorV1,
};
pub use runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction, TypeBridgeRuntime,
    TypeBridgeRuntimeConfigV1, TypeBridgeWriteTransaction,
};

pub use runtime::{TypeBridgeDebugListener, type_bridge_debug_listener_set};

pub use staged::{TypeBridgeStagedTransactionOptionsV1, type_bridge_plan_explain};

pub use plan_builder::idioms::{
    TypeBridgePlanBoolean, TypeBridgePlanCondition, TypeBridgePlanDate, TypeBridgePlanDateTime,
    TypeBridgePlanDateTimeTz, TypeBridgePlanDecimal, TypeBridgePlanDouble, TypeBridgePlanDuration,
    TypeBridgePlanLong, TypeBridgePlanString,
};

pub use plan_builder::bulk::{
    TypeBridgePlanBulkCell, TypeBridgePlanBulkColumn, TypeBridgePlanBulkHostColumn,
    TypeBridgePlanBulkLiteral,
};

pub use plan_builder::context::{TYPE_BRIDGE_PLAN_OPTIONS_VERSION, TypeBridgePlanOptionsV1};
pub use query::TypeBridgeQueryLimitOverrideV1;

pub use iid::TypeBridgeIid;
pub use manager_filter::TypeBridgeManagerFilter;
pub use manager_read::TypeBridgeManagerResult;
pub use raw::{TypeBridgeRawResult, TypeBridgeRawTransaction};
