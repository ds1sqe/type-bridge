//! Versioned native C boundary for generated TypeBridge schema packages.

#![deny(missing_docs)]

mod abi;
mod allocation;
mod canonical_archive;
mod crud_v2;
mod diagnostic;
mod entity_crud;
mod execution_diagnostic;
mod generated_preflight;
mod migration_abi;
mod migration_runtime;
mod policy;
mod projected_batch;
mod projected_create_builder;
mod projected_model;
mod projected_token;
mod projected_value;
mod query;
mod relation_crud;
mod runtime;
mod schema_package;
mod thing_crud;

pub use abi::{
    TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION, TypeBridgeByteView, TypeBridgeChunkedByteViewV1,
    TypeBridgeDiagnostics, TypeBridgeSchemaPackage, TypeBridgeSchemaPackageChunkedDescriptorV1,
    TypeBridgeSchemaPackageDescriptorV1, TypeBridgeStatus,
};
pub use canonical_archive::{
    TypeBridgeCanonicalArchive, TypeBridgeCanonicalArchiveBuilder, TypeBridgeCanonicalBytes,
    TypeBridgeProjectedStruct, TypeBridgeProjectedStructMember,
};
pub use execution_diagnostic::{
    TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticDetailKind,
    TypeBridgeExecutionDiagnosticDetailViewV1, TypeBridgeExecutionDiagnosticPathKind,
    TypeBridgeExecutionDiagnosticPathViewV1, TypeBridgeExecutionDiagnosticViewV1,
    TypeBridgeExecutionDiagnostics,
};
pub use generated_preflight::{
    GENERATED_CREATE_GRAPH_VERSION, GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX,
    GENERATED_CREATE_MEMBER_COUNT_MAX, GENERATED_CREATE_MEMBER_REFERENCE_SCALAR,
    GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE, GENERATED_CREATE_MEMBER_VALUE_SCALAR,
    GENERATED_CREATE_MEMBER_VALUE_SEQUENCE, GENERATED_INPUT_BYTES, GENERATED_INPUT_CANCELLATION,
    GENERATED_INPUT_CANONICAL_ARCHIVE, GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER,
    GENERATED_INPUT_CANONICAL_BYTES, GENERATED_INPUT_CREATE_ARGS_GRAPH, GENERATED_INPUT_DATABASE,
    GENERATED_INPUT_DIAGNOSTICS, GENERATED_INPUT_EXECUTION_DIAGNOSTICS,
    GENERATED_INPUT_PROJECTED_BATCH, GENERATED_INPUT_PROJECTED_BATCH_BUILDER,
    GENERATED_INPUT_PROJECTED_BATCH_RESULT, GENERATED_INPUT_PROJECTED_CREATE,
    GENERATED_INPUT_PROJECTED_REFERENCE, GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY,
    GENERATED_INPUT_PROJECTED_STRUCT, GENERATED_INPUT_PROJECTED_STRUCT_MEMBER,
    GENERATED_INPUT_PROJECTED_THING, GENERATED_INPUT_PROJECTED_TOKEN,
    GENERATED_INPUT_PROJECTED_VALUE, GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY,
    GENERATED_INPUT_QUERY, GENERATED_INPUT_QUERY_BINDING, GENERATED_INPUT_QUERY_FIELD,
    GENERATED_INPUT_QUERY_FUNCTION, GENERATED_INPUT_QUERY_FUNCTION_CALL,
    GENERATED_INPUT_QUERY_FUNCTION_VALUE, GENERATED_INPUT_QUERY_ORDER,
    GENERATED_INPUT_QUERY_PREDICATE, GENERATED_INPUT_QUERY_REMOTE_CLAIM,
    GENERATED_INPUT_QUERY_REMOTE_CONTEXT, GENERATED_INPUT_QUERY_REMOTE_PENDING,
    GENERATED_INPUT_QUERY_RESULT, GENERATED_INPUT_QUERY_ROLE, GENERATED_INPUT_QUERY_SELECTION,
    GENERATED_INPUT_QUERY_SESSION, GENERATED_INPUT_QUERY_TERMINAL,
    GENERATED_INPUT_READ_TRANSACTION, GENERATED_INPUT_RUNTIME, GENERATED_INPUT_SCHEMA_PACKAGE,
    GENERATED_INPUT_WRITE_TRANSACTION, TypeBridgeGeneratedCreateArgsGraphV1,
    TypeBridgeGeneratedCreateHandleChunkV1, TypeBridgeGeneratedCreateMemberV1,
    TypeBridgeGeneratedOpaqueInputV1, TypeBridgeGeneratedOutputRangeV1,
};
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
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeDatabaseConfigV1,
    TypeBridgeReadTransaction, TypeBridgeRuntime, TypeBridgeRuntimeConfigV1,
    TypeBridgeWriteTransaction,
};
