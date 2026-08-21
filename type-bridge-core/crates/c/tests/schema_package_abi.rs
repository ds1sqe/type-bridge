use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::mem::{align_of, offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use type_bridge_c::{
    TypeBridgeByteView, TypeBridgeChunkedByteViewV1, TypeBridgeDiagnostics,
    TypeBridgeExecutionDiagnosticCategory, TypeBridgeExecutionDiagnosticDetailKind,
    TypeBridgeExecutionDiagnosticDetailViewV1, TypeBridgeExecutionDiagnosticPathKind,
    TypeBridgeExecutionDiagnosticPathViewV1, TypeBridgeExecutionDiagnosticViewV1,
    TypeBridgeExecutionDiagnostics, TypeBridgeGeneratedCreateArgsGraphV1,
    TypeBridgeGeneratedCreateHandleChunkV1, TypeBridgeGeneratedCreateMemberV1,
    TypeBridgeGeneratedOpaqueInputV1, TypeBridgeGeneratedOutputRangeV1,
    TypeBridgeProjectedCreateDescriptorV1, TypeBridgeProjectedFieldInputV1,
    TypeBridgeProjectedReferenceDescriptorV1, TypeBridgeProjectedRoleInputV1,
    TypeBridgeProjectedThingDescriptorV1, TypeBridgeProjectedTokenV1, TypeBridgeQueryDescriptorV1,
    TypeBridgeQueryExecutionLimitsV1, TypeBridgeQueryFieldReferenceV1,
    TypeBridgeQueryFunctionArgumentMemberV1, TypeBridgeQueryFunctionArgumentV1,
    TypeBridgeQueryFunctionArgumentsGraphV1, TypeBridgeQueryFunctionArgumentsHeaderV1,
    TypeBridgeQueryOrderDescriptorV1, TypeBridgeQueryPageMetadataV1,
    TypeBridgeQueryReducedValueMetadataV1, TypeBridgeQueryReducerV1,
    TypeBridgeQuerySelectionDescriptorV1, TypeBridgeQueryShapeSlotV1,
    TypeBridgeQueryTerminalDescriptorV1, TypeBridgeSchemaPackage,
    TypeBridgeSchemaPackageChunkedDescriptorV1, TypeBridgeSchemaPackageDescriptorV1,
    TypeBridgeStatus,
};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::limits::MAX_CANONICAL_BYTES;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::projection::{
    BindingTarget, CSymbolPrefix, CodeResourceDigest, EmissionPlan, ProjectedTokenIdentity,
    ProjectionConfig, RuntimeProjection,
};
use type_bridge_contract::projection_wire::decode_runtime_projection_verified;
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet, build_schema_authority,
    decode_schema_authority, normalize_documents, project, resolve,
    schema_authority_capability_vocabulary,
};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};

const SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  aliases: { value: string }
  identifier: { value: string }
  secondary-identifier: { value: string }
  measure-long: { value: integer }
  measure-double: { value: double }
  enabled: { value: boolean }
  born-on: { value: date }
  observed-at: { value: datetime }
  zoned-at: { value: datetime-tz }
  balance: { value: decimal }
  elapsed: { value: duration }
entities:
  person:
    owns:
      aliases: { card: { min: 0, max: 3 } }
      identifier: { key: true }
      measure-long: {}
      measure-double: {}
      enabled: {}
      born-on: {}
      observed-at: {}
      zoned-at: {}
      balance: {}
      elapsed: {}
relations:
  membership:
    relates:
      member: { card: 1 }
plays:
  person:
    membership: [member]
"#;

const RELATION_SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  employee-id: { value: string }
  person-id: { value: string }
  robot-id: { value: integer }
entities:
  employee:
    owns:
      employee-id: { key: true }
  person:
    owns:
      person-id: { key: true }
  robot:
    owns:
      robot-id: { key: true }
relations:
  container:
    relates:
      item: { card: { min: 0, max: 3 } }
  event: {}
  membership:
    relates:
      member: { card: 1 }
plays:
  event:
    container: [item]
  person:
    membership: [member]
  robot:
    membership: [member]
"#;

const ORDERED_PHASE2_SOURCE: &str = r#"format: typebridge.schema/v2
attributes:
  aliases: { value: string }
entities:
  actor:
    abstract: true
    owns:
      aliases:
        card: { min: 0, max: 3 }
        ordered: true
        distinct: true
  person: { sub: actor }
relations:
  base-activity:
    relates:
      participant:
        abstract: true
        card: { min: 0, max: 3 }
        ordered: true
        distinct: true
  plain-activity: { sub: base-activity }
plays:
  person:
    base-activity:
      participant: { card: { min: 0, max: 3 } }
"#;
const ABI_1_3_EXPORTED_SYMBOLS: [&str; 180] = [
    "type_bridge_c_abi_major",
    "type_bridge_c_abi_minor",
    "type_bridge_cancellation_close",
    "type_bridge_cancellation_is_requested",
    "type_bridge_cancellation_open",
    "type_bridge_cancellation_request",
    "type_bridge_database_close",
    "type_bridge_database_entity_count",
    "type_bridge_database_entity_delete_by_iid",
    "type_bridge_database_entity_get_by_iid",
    "type_bridge_database_entity_insert",
    "type_bridge_database_entity_put",
    "type_bridge_database_entity_update",
    "type_bridge_database_relation_count",
    "type_bridge_database_relation_delete_by_iid",
    "type_bridge_database_relation_get_by_iid",
    "type_bridge_database_relation_insert",
    "type_bridge_database_relation_put",
    "type_bridge_database_relation_update",
    "type_bridge_database_open_v1",
    "type_bridge_database_query_execute_v1",
    "type_bridge_database_server_version",
    "type_bridge_diagnostics_close",
    "type_bridge_diagnostics_json",
    "type_bridge_execution_diagnostics_close",
    "type_bridge_execution_diagnostics_count",
    "type_bridge_execution_diagnostics_detail_get_v1",
    "type_bridge_execution_diagnostics_detail_list_count",
    "type_bridge_execution_diagnostics_detail_list_get",
    "type_bridge_execution_diagnostics_detail_signed",
    "type_bridge_execution_diagnostics_get_v1",
    "type_bridge_execution_diagnostics_path_get_v1",
    "type_bridge_generated_opaque_alias_preflight_v1",
    "type_bridge_projected_collection_limit_diagnostics",
    "type_bridge_projected_create_builder_add_v1",
    "type_bridge_projected_create_builder_close",
    "type_bridge_projected_create_builder_finish",
    "type_bridge_projected_create_builder_open_v1",
    "type_bridge_projected_create_close",
    "type_bridge_projected_create_field_count",
    "type_bridge_projected_create_field_value_at",
    "type_bridge_projected_create_open_v1",
    "type_bridge_projected_create_role_count",
    "type_bridge_projected_create_role_reference_at",
    "type_bridge_projected_reference_clone",
    "type_bridge_projected_reference_close",
    "type_bridge_projected_reference_iid",
    "type_bridge_projected_reference_key",
    "type_bridge_projected_reference_model_ordinal",
    "type_bridge_projected_reference_open_v1",
    "type_bridge_projected_reference_validate_model",
    "type_bridge_projected_reference_validate_role",
    "type_bridge_projected_thing_clone",
    "type_bridge_projected_thing_close",
    "type_bridge_projected_thing_field_count",
    "type_bridge_projected_thing_field_value_at",
    "type_bridge_projected_thing_iid",
    "type_bridge_projected_thing_model_ordinal",
    "type_bridge_projected_thing_open_v1",
    "type_bridge_projected_thing_reference",
    "type_bridge_projected_thing_role_count",
    "type_bridge_projected_thing_role_reference_at",
    "type_bridge_projected_thing_scalar_field_value",
    "type_bridge_projected_thing_scalar_role_reference",
    "type_bridge_projected_thing_validate_model",
    "type_bridge_projected_value_boolean",
    "type_bridge_projected_value_boolean_open",
    "type_bridge_projected_value_close",
    "type_bridge_projected_value_date_open",
    "type_bridge_projected_value_datetime_open",
    "type_bridge_projected_value_datetime_tz_open",
    "type_bridge_projected_value_decimal_open",
    "type_bridge_projected_value_double_bits",
    "type_bridge_projected_value_double_open",
    "type_bridge_projected_value_duration_open",
    "type_bridge_projected_value_kind",
    "type_bridge_projected_value_long",
    "type_bridge_projected_value_long_open",
    "type_bridge_projected_value_string_open",
    "type_bridge_projected_value_text",
    "type_bridge_projected_value_validate_model",
    "type_bridge_query_add_hidden",
    "type_bridge_query_allow_cross_join",
    "type_bridge_query_binding_close",
    "type_bridge_query_binding_iid_in_v1",
    "type_bridge_query_binding_iid_v1",
    "type_bridge_query_binding_open_v1",
    "type_bridge_query_close",
    "type_bridge_query_field_close",
    "type_bridge_query_field_compare_field",
    "type_bridge_query_field_compare_function",
    "type_bridge_query_field_compare_value",
    "type_bridge_query_field_open",
    "type_bridge_query_field_presence",
    "type_bridge_query_function_call_close",
    "type_bridge_query_function_call_compare_call",
    "type_bridge_query_function_call_compare_field",
    "type_bridge_query_function_call_compare_value",
    "type_bridge_query_function_call_open_v1",
    "type_bridge_query_function_close",
    "type_bridge_query_function_open",
    "type_bridge_query_function_value_close",
    "type_bridge_query_function_value_open",
    "type_bridge_query_open_v1",
    "type_bridge_query_order_close",
    "type_bridge_query_order_open_v1",
    "type_bridge_query_predicate_close",
    "type_bridge_query_predicate_combine",
    "type_bridge_query_remote_claim_close",
    "type_bridge_query_remote_claim_decode_v1",
    "type_bridge_query_remote_pending_response_snapshot_limit",
    "type_bridge_query_remote_context_close",
    "type_bridge_query_remote_context_open_v1",
    "type_bridge_query_remote_pending_claim",
    "type_bridge_query_remote_pending_close",
    "type_bridge_query_remote_pending_request_bytes",
    "type_bridge_query_remote_prepare_v1",
    "type_bridge_query_result_close",
    "type_bridge_query_result_count",
    "type_bridge_query_result_exists",
    "type_bridge_query_result_kind",
    "type_bridge_query_result_page_metadata_v1",
    "type_bridge_query_result_reduction_group_field_at",
    "type_bridge_query_result_reduction_group_field_count",
    "type_bridge_query_result_reduction_group_kind",
    "type_bridge_query_result_reduction_group_thing",
    "type_bridge_query_result_reduction_row_count",
    "type_bridge_query_result_reduction_value_count",
    "type_bridge_query_result_reduction_value_double_bits",
    "type_bridge_query_result_reduction_value_long",
    "type_bridge_query_result_reduction_value_metadata_v1",
    "type_bridge_query_result_row_count",
    "type_bridge_query_result_row_slot_count",
    "type_bridge_query_result_row_slot_thing_at",
    "type_bridge_query_role_close",
    "type_bridge_query_role_connects",
    "type_bridge_query_role_open",
    "type_bridge_query_selection_close",
    "type_bridge_query_selection_open_v1",
    "type_bridge_query_session_close",
    "type_bridge_query_session_open",
    "type_bridge_query_session_reachable",
    "type_bridge_query_terminal_close",
    "type_bridge_query_terminal_open_v1",
    "type_bridge_query_where",
    "type_bridge_read_transaction_close",
    "type_bridge_read_transaction_entity_count",
    "type_bridge_read_transaction_entity_get_by_iid",
    "type_bridge_read_transaction_relation_count",
    "type_bridge_read_transaction_relation_get_by_iid",
    "type_bridge_read_transaction_open",
    "type_bridge_read_transaction_query_execute_v1",
    "type_bridge_runtime_close",
    "type_bridge_runtime_open_v1",
    "type_bridge_runtime_version",
    "type_bridge_schema_package_authority_json",
    "type_bridge_schema_package_binding_fingerprint_json",
    "type_bridge_schema_package_close",
    "type_bridge_schema_package_managed_scope",
    "type_bridge_schema_package_open_chunked_v1",
    "type_bridge_schema_package_open_v1",
    "type_bridge_schema_package_projection_json",
    "type_bridge_schema_package_semantic_fingerprint_json",
    "type_bridge_schema_package_semantic_profile",
    "type_bridge_write_transaction_close",
    "type_bridge_write_transaction_commit",
    "type_bridge_write_transaction_entity_count",
    "type_bridge_write_transaction_entity_delete_by_iid",
    "type_bridge_write_transaction_entity_get_by_iid",
    "type_bridge_write_transaction_entity_insert",
    "type_bridge_write_transaction_entity_put",
    "type_bridge_write_transaction_entity_update",
    "type_bridge_write_transaction_relation_count",
    "type_bridge_write_transaction_relation_delete_by_iid",
    "type_bridge_write_transaction_relation_get_by_iid",
    "type_bridge_write_transaction_relation_insert",
    "type_bridge_write_transaction_relation_put",
    "type_bridge_write_transaction_relation_update",
    "type_bridge_write_transaction_open",
    "type_bridge_write_transaction_rollback",
];

const ABI_1_4_ADDED_EXPORTED_SYMBOLS: [&str; 45] = [
    "type_bridge_database_config_validate_v2",
    "type_bridge_database_entity_count_v2",
    "type_bridge_database_entity_delete_by_iid_v2",
    "type_bridge_database_entity_get_by_iid_v2",
    "type_bridge_database_entity_insert_v2",
    "type_bridge_database_entity_put_v2",
    "type_bridge_database_entity_update_v2",
    "type_bridge_database_open_v2",
    "type_bridge_database_projected_batch_execute_v1",
    "type_bridge_database_relation_count_v2",
    "type_bridge_database_relation_delete_by_iid_v2",
    "type_bridge_database_relation_get_by_iid_v2",
    "type_bridge_database_relation_insert_v2",
    "type_bridge_database_relation_put_v2",
    "type_bridge_database_relation_update_v2",
    "type_bridge_projected_batch_builder_add_v1",
    "type_bridge_projected_batch_builder_close",
    "type_bridge_projected_batch_builder_finish",
    "type_bridge_projected_batch_builder_open_v1",
    "type_bridge_projected_batch_close",
    "type_bridge_projected_batch_result_close",
    "type_bridge_projected_batch_result_count",
    "type_bridge_projected_batch_result_thing_at",
    "type_bridge_read_transaction_entity_count_v2",
    "type_bridge_read_transaction_entity_get_by_iid_v2",
    "type_bridge_read_transaction_open_v2",
    "type_bridge_read_transaction_relation_count_v2",
    "type_bridge_read_transaction_relation_get_by_iid_v2",
    "type_bridge_schema_package_open_chunked_v2",
    "type_bridge_schema_package_open_v2",
    "type_bridge_write_transaction_commit_v2",
    "type_bridge_write_transaction_entity_count_v2",
    "type_bridge_write_transaction_entity_delete_by_iid_v2",
    "type_bridge_write_transaction_entity_get_by_iid_v2",
    "type_bridge_write_transaction_entity_insert_v2",
    "type_bridge_write_transaction_entity_put_v2",
    "type_bridge_write_transaction_entity_update_v2",
    "type_bridge_write_transaction_open_v2",
    "type_bridge_write_transaction_projected_batch_execute_v1",
    "type_bridge_write_transaction_relation_count_v2",
    "type_bridge_write_transaction_relation_delete_by_iid_v2",
    "type_bridge_write_transaction_relation_get_by_iid_v2",
    "type_bridge_write_transaction_relation_insert_v2",
    "type_bridge_write_transaction_relation_put_v2",
    "type_bridge_write_transaction_relation_update_v2",
];

// ABI 1.3 is append-only over this exact ABI 1.2 surface. Keep this ledger
// independent of the current export list so a removal or rename cannot be
// hidden by merely updating the latest-version inventory.
const ABI_1_2_EXPORTED_SYMBOLS: [&str; 109] = [
    "type_bridge_c_abi_major",
    "type_bridge_c_abi_minor",
    "type_bridge_cancellation_close",
    "type_bridge_cancellation_is_requested",
    "type_bridge_cancellation_open",
    "type_bridge_cancellation_request",
    "type_bridge_database_close",
    "type_bridge_database_entity_count",
    "type_bridge_database_entity_delete_by_iid",
    "type_bridge_database_entity_get_by_iid",
    "type_bridge_database_entity_insert",
    "type_bridge_database_entity_put",
    "type_bridge_database_entity_update",
    "type_bridge_database_relation_count",
    "type_bridge_database_relation_delete_by_iid",
    "type_bridge_database_relation_get_by_iid",
    "type_bridge_database_relation_insert",
    "type_bridge_database_relation_put",
    "type_bridge_database_relation_update",
    "type_bridge_database_open_v1",
    "type_bridge_database_server_version",
    "type_bridge_diagnostics_close",
    "type_bridge_diagnostics_json",
    "type_bridge_execution_diagnostics_close",
    "type_bridge_execution_diagnostics_count",
    "type_bridge_execution_diagnostics_detail_get_v1",
    "type_bridge_execution_diagnostics_get_v1",
    "type_bridge_execution_diagnostics_path_get_v1",
    "type_bridge_generated_opaque_alias_preflight_v1",
    "type_bridge_projected_collection_limit_diagnostics",
    "type_bridge_projected_create_builder_add_v1",
    "type_bridge_projected_create_builder_close",
    "type_bridge_projected_create_builder_finish",
    "type_bridge_projected_create_builder_open_v1",
    "type_bridge_projected_create_close",
    "type_bridge_projected_create_field_count",
    "type_bridge_projected_create_field_value_at",
    "type_bridge_projected_create_open_v1",
    "type_bridge_projected_create_role_count",
    "type_bridge_projected_create_role_reference_at",
    "type_bridge_projected_reference_clone",
    "type_bridge_projected_reference_close",
    "type_bridge_projected_reference_iid",
    "type_bridge_projected_reference_key",
    "type_bridge_projected_reference_model_ordinal",
    "type_bridge_projected_reference_open_v1",
    "type_bridge_projected_reference_validate_model",
    "type_bridge_projected_reference_validate_role",
    "type_bridge_projected_thing_close",
    "type_bridge_projected_thing_field_count",
    "type_bridge_projected_thing_field_value_at",
    "type_bridge_projected_thing_iid",
    "type_bridge_projected_thing_open_v1",
    "type_bridge_projected_thing_reference",
    "type_bridge_projected_thing_role_count",
    "type_bridge_projected_thing_role_reference_at",
    "type_bridge_projected_thing_scalar_field_value",
    "type_bridge_projected_thing_scalar_role_reference",
    "type_bridge_projected_thing_validate_model",
    "type_bridge_projected_value_boolean",
    "type_bridge_projected_value_boolean_open",
    "type_bridge_projected_value_close",
    "type_bridge_projected_value_date_open",
    "type_bridge_projected_value_datetime_open",
    "type_bridge_projected_value_datetime_tz_open",
    "type_bridge_projected_value_decimal_open",
    "type_bridge_projected_value_double_bits",
    "type_bridge_projected_value_double_open",
    "type_bridge_projected_value_duration_open",
    "type_bridge_projected_value_kind",
    "type_bridge_projected_value_long",
    "type_bridge_projected_value_long_open",
    "type_bridge_projected_value_string_open",
    "type_bridge_projected_value_text",
    "type_bridge_projected_value_validate_model",
    "type_bridge_read_transaction_close",
    "type_bridge_read_transaction_entity_count",
    "type_bridge_read_transaction_entity_get_by_iid",
    "type_bridge_read_transaction_relation_count",
    "type_bridge_read_transaction_relation_get_by_iid",
    "type_bridge_read_transaction_open",
    "type_bridge_runtime_close",
    "type_bridge_runtime_open_v1",
    "type_bridge_runtime_version",
    "type_bridge_schema_package_authority_json",
    "type_bridge_schema_package_binding_fingerprint_json",
    "type_bridge_schema_package_close",
    "type_bridge_schema_package_managed_scope",
    "type_bridge_schema_package_open_chunked_v1",
    "type_bridge_schema_package_open_v1",
    "type_bridge_schema_package_projection_json",
    "type_bridge_schema_package_semantic_fingerprint_json",
    "type_bridge_schema_package_semantic_profile",
    "type_bridge_write_transaction_close",
    "type_bridge_write_transaction_commit",
    "type_bridge_write_transaction_entity_count",
    "type_bridge_write_transaction_entity_delete_by_iid",
    "type_bridge_write_transaction_entity_get_by_iid",
    "type_bridge_write_transaction_entity_insert",
    "type_bridge_write_transaction_entity_put",
    "type_bridge_write_transaction_entity_update",
    "type_bridge_write_transaction_relation_count",
    "type_bridge_write_transaction_relation_delete_by_iid",
    "type_bridge_write_transaction_relation_get_by_iid",
    "type_bridge_write_transaction_relation_insert",
    "type_bridge_write_transaction_relation_put",
    "type_bridge_write_transaction_relation_update",
    "type_bridge_write_transaction_open",
    "type_bridge_write_transaction_rollback",
];

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn type_bridge_c_abi_major() -> u32;
    fn type_bridge_c_abi_minor() -> u32;
    fn type_bridge_runtime_version(out_version: *mut TypeBridgeByteView) -> TypeBridgeStatus;
    fn type_bridge_schema_package_open_v1(
        descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut TypeBridgeDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_open_chunked_v1(
        descriptor: *const TypeBridgeSchemaPackageChunkedDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut TypeBridgeDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_open_v2(
        descriptor: *const TypeBridgeSchemaPackageDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_open_chunked_v2(
        descriptor: *const TypeBridgeSchemaPackageChunkedDescriptorV1,
        out_package: *mut *mut TypeBridgeSchemaPackage,
        out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_close(
        package: *mut *mut TypeBridgeSchemaPackage,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_authority_json(
        package: *const TypeBridgeSchemaPackage,
        out_value: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_projection_json(
        package: *const TypeBridgeSchemaPackage,
        out_value: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_semantic_fingerprint_json(
        package: *const TypeBridgeSchemaPackage,
        out_value: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_binding_fingerprint_json(
        package: *const TypeBridgeSchemaPackage,
        out_value: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_managed_scope(
        package: *const TypeBridgeSchemaPackage,
        out_value: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_schema_package_semantic_profile(
        package: *const TypeBridgeSchemaPackage,
        out_value: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_diagnostics_json(
        diagnostics: *const TypeBridgeDiagnostics,
        out_json: *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus;
    fn type_bridge_diagnostics_close(
        diagnostics: *mut *mut TypeBridgeDiagnostics,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_count(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        out_count: *mut usize,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        index: usize,
        out_diagnostic: *mut TypeBridgeExecutionDiagnosticViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_path_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        diagnostic_index: usize,
        path_index: usize,
        out_path: *mut TypeBridgeExecutionDiagnosticPathViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_detail_get_v1(
        diagnostics: *const TypeBridgeExecutionDiagnostics,
        diagnostic_index: usize,
        detail_index: usize,
        out_detail: *mut TypeBridgeExecutionDiagnosticDetailViewV1,
    ) -> TypeBridgeStatus;
    fn type_bridge_execution_diagnostics_close(
        diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    ) -> TypeBridgeStatus;
}

#[derive(Clone, Debug)]
struct EmittedDescriptorBytes {
    authority: Vec<u8>,
    declared: Vec<u8>,
    projection: Vec<u8>,
    semantic: Vec<u8>,
    binding: Vec<u8>,
    scope: Vec<u8>,
    profile: Vec<u8>,
}

#[derive(Debug)]
struct EmittedFixture {
    package: GeneratedPackage,
    bytes: EmittedDescriptorBytes,
}

#[derive(Debug)]
struct RelationEmittedFixture {
    package: GeneratedPackage,
    membership_model_ordinal: u32,
    membership_role_ordinal: u32,
    container_model_ordinal: u32,
    container_role_ordinal: u32,
}

#[derive(Debug)]
struct OrderedPhase2EmittedFixture {
    package: GeneratedPackage,
    foreign_package: GeneratedPackage,
    person_model_ordinal: u32,
    person_aliases_field_ordinal: u32,
    plain_activity_model_ordinal: u32,
    plain_activity_participant_role_ordinal: u32,
}

impl EmittedDescriptorBytes {
    fn descriptor(&self) -> TypeBridgeSchemaPackageDescriptorV1 {
        TypeBridgeSchemaPackageDescriptorV1 {
            struct_size: size_of::<TypeBridgeSchemaPackageDescriptorV1>()
                .try_into()
                .expect("C descriptor size fits u32"),
            abi_major: 1,
            abi_minor: 1,
            schema_authority_json: view(&self.authority),
            declared_schema_json: view(&self.declared),
            runtime_projection_json: view(&self.projection),
            semantic_fingerprint_json: view(&self.semantic),
            binding_fingerprint_json: view(&self.binding),
            managed_scope: view(&self.scope),
            semantic_profile: view(&self.profile),
            reserved: [0; 4],
        }
    }

    fn chunked_parts(&self, chunk_len: usize) -> ChunkedDescriptorParts {
        fn chunks(bytes: &[u8], chunk_len: usize) -> Vec<TypeBridgeByteView> {
            bytes.chunks(chunk_len).map(view).collect()
        }
        ChunkedDescriptorParts {
            authority: chunks(&self.authority, chunk_len),
            declared: chunks(&self.declared, chunk_len),
            projection: chunks(&self.projection, chunk_len),
            semantic: chunks(&self.semantic, chunk_len),
            binding: chunks(&self.binding, chunk_len),
            scope: chunks(&self.scope, chunk_len),
            profile: chunks(&self.profile, chunk_len),
        }
    }
}

struct ChunkedDescriptorParts {
    authority: Vec<TypeBridgeByteView>,
    declared: Vec<TypeBridgeByteView>,
    projection: Vec<TypeBridgeByteView>,
    semantic: Vec<TypeBridgeByteView>,
    binding: Vec<TypeBridgeByteView>,
    scope: Vec<TypeBridgeByteView>,
    profile: Vec<TypeBridgeByteView>,
}

impl ChunkedDescriptorParts {
    fn descriptor(&self) -> TypeBridgeSchemaPackageChunkedDescriptorV1 {
        fn scatter(chunks: &[TypeBridgeByteView]) -> TypeBridgeChunkedByteViewV1 {
            TypeBridgeChunkedByteViewV1 {
                struct_size: size_of::<TypeBridgeChunkedByteViewV1>() as u32,
                version: 1,
                chunks: chunks.as_ptr(),
                chunk_count: chunks.len(),
                total_length: chunks.iter().map(|chunk| chunk.length).sum(),
                reserved: [0; 4],
            }
        }
        TypeBridgeSchemaPackageChunkedDescriptorV1 {
            struct_size: size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>() as u32,
            abi_major: 1,
            abi_minor: 2,
            reserved0: 0,
            schema_authority_json: scatter(&self.authority),
            declared_schema_json: scatter(&self.declared),
            runtime_projection_json: scatter(&self.projection),
            semantic_fingerprint_json: scatter(&self.semantic),
            binding_fingerprint_json: scatter(&self.binding),
            managed_scope: scatter(&self.scope),
            semantic_profile: scatter(&self.profile),
            reserved: [0; 4],
        }
    }
}

fn view(bytes: &[u8]) -> TypeBridgeByteView {
    TypeBridgeByteView {
        data: bytes.as_ptr(),
        length: bytes.len(),
    }
}

fn emitted_array(source: &str, name: &str) -> Vec<u8> {
    fn parse(source: &str, symbol: &str) -> Option<Vec<u8>> {
        let marker = format!("static const uint8_t {symbol}[] = {{\n");
        let body = source.split_once(&marker)?.1.split_once("};\n")?.0;
        Some(
            body.lines()
                .flat_map(|line| line.split(','))
                .filter_map(|item| {
                    let item = item.trim();
                    if item.is_empty() {
                        return None;
                    }
                    let hexadecimal = item
                        .strip_prefix("0x")
                        .and_then(|item| item.strip_suffix('u'))
                        .unwrap_or_else(|| panic!("invalid generated byte literal {item:?}"));
                    Some(
                        u8::from_str_radix(hexadecimal, 16)
                            .expect("generated byte literal is valid"),
                    )
                })
                .collect(),
        )
    }
    if let Some(bytes) = parse(source, name) {
        return bytes;
    }
    let mut bytes = Vec::new();
    for index in 0.. {
        let Some(chunk) = parse(source, &format!("{name}_chunk_{index}")) else {
            break;
        };
        bytes.extend(chunk);
    }
    assert!(!bytes.is_empty(), "generated source omitted {name} chunks");
    bytes
}

fn emitted_fixture() -> EmittedFixture {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-schema-package-abi.yaml").expect("fixture document ID is valid"),
        SOURCE,
    )])
    .expect("C ABI fixture parses");
    let declared = normalize_documents(&documents).expect("C ABI fixture normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("C ABI fixture resolves");
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("c-schema-package-abi").expect("scope is valid"),
        profile,
        available,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("C ABI fixture authority builds");
    let emitter = CEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("fixture").expect("prefix is valid")),
        &emitter.generator_handlers(),
        &emitter.code_resources().expect("C emitter resources hash"),
    )
    .expect("C ABI fixture projects");
    let package = emitter
        .emit(&projection, &authority)
        .expect("C ABI fixture emits");
    let source = std::str::from_utf8(
        package
            .get("src/models.c")
            .expect("generated C source exists"),
    )
    .expect("generated C source is UTF-8");

    EmittedFixture {
        bytes: EmittedDescriptorBytes {
            authority: emitted_array(source, "fixture_schema_authority_json"),
            declared: emitted_array(source, "fixture_declared_schema_json"),
            projection: emitted_array(source, "fixture_runtime_projection_json"),
            semantic: emitted_array(source, "fixture_semantic_fingerprint_json"),
            binding: emitted_array(source, "fixture_binding_fingerprint_json"),
            scope: emitted_array(source, "fixture_managed_scope"),
            profile: emitted_array(source, "fixture_semantic_profile"),
        },
        package,
    }
}

fn emitted_relation_fixture() -> RelationEmittedFixture {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-relation-schema-package-abi.yaml")
            .expect("relation fixture document ID is valid"),
        RELATION_SOURCE,
    )])
    .expect("relation C ABI fixture parses");
    let declared = normalize_documents(&documents).expect("relation C ABI fixture normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("relation C ABI fixture resolves");
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("c-relation-schema-package-abi").expect("scope is valid"),
        profile,
        available,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("relation C ABI fixture authority builds");
    let emitter = CEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::C,
        &ProjectionConfig::c(CSymbolPrefix::new("relation").expect("prefix is valid")),
        &emitter.generator_handlers(),
        &emitter.code_resources().expect("C emitter resources hash"),
    )
    .expect("relation C ABI fixture projects");

    let token_ordinals = |model_label: &str, role_label: &str| {
        let model = projection
            .models()
            .values()
            .find(|model| model.id().label().as_str() == model_label)
            .unwrap_or_else(|| panic!("relation fixture omitted {model_label}"));
        let role = model
            .query_tokens()
            .roles()
            .values()
            .find(|role| role.role().label().as_str() == role_label)
            .unwrap_or_else(|| panic!("relation fixture omitted {model_label}:{role_label}"));
        let model_ordinal = projection
            .projected_token_ordinal(&ProjectedTokenIdentity::Model(model.id().clone()))
            .expect("relation model has a projected token ordinal");
        let role_ordinal = projection
            .projected_token_ordinal(&ProjectedTokenIdentity::Role {
                owner: model.id().clone(),
                role: role.role().clone(),
            })
            .expect("relation role has a projected token ordinal");
        (model_ordinal, role_ordinal)
    };
    let (membership_model_ordinal, membership_role_ordinal) =
        token_ordinals("membership", "member");
    let (container_model_ordinal, container_role_ordinal) = token_ordinals("container", "item");
    let package = emitter
        .emit(&projection, &authority)
        .expect("relation C ABI fixture emits");

    RelationEmittedFixture {
        package,
        membership_model_ordinal,
        membership_role_ordinal,
        container_model_ordinal,
        container_role_ordinal,
    }
}

fn emitted_ordered_phase2_fixture() -> OrderedPhase2EmittedFixture {
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("c-ordered-phase2-schema-package-abi.yaml")
            .expect("ordered Phase-2 fixture document ID is valid"),
        ORDERED_PHASE2_SOURCE,
    )])
    .expect("ordered Phase-2 C ABI fixture parses");
    let declared =
        normalize_documents(&documents).expect("ordered Phase-2 C ABI fixture normalizes");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile is valid");
    let resolved = resolve(&declared, &profile).expect("ordered Phase-2 C ABI fixture resolves");
    let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|id| CapabilityId::new(*id).expect("built-in capability ID is valid"))
        .collect();
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("c-ordered-phase2-schema-package-abi").expect("scope is valid"),
        profile,
        available,
    );
    let authority = build_schema_authority(&declared, declared.required_capabilities(), &context)
        .expect("ordered Phase-2 C ABI fixture authority builds");
    let emitter = CEmitter::new();
    let handlers = emitter.generator_handlers_for(&resolved);
    let resources = emitter
        .code_resources_for(&resolved)
        .expect("ordered Phase-2 C emitter resources hash");
    let emit = |prefix: &str| {
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).expect("prefix is valid")),
            &handlers,
            &resources,
        )
        .expect("ordered Phase-2 C ABI fixture projects");
        let package = emitter
            .emit(&projection, &authority)
            .expect("ordered Phase-2 C ABI fixture emits");
        (projection, package)
    };
    let (projection, package) = emit("orderedphase");
    let (foreign_projection, foreign_package) = emit("orderedforeign");

    let token_ordinals = |projection: &RuntimeProjection| {
        let person = projection
            .models()
            .values()
            .find(|model| model.id().label().as_str() == "person")
            .expect("ordered Phase-2 fixture projects person");
        let aliases = person
            .create()
            .fields()
            .iter()
            .find(|field| field.token().attribute().label().as_str() == "aliases")
            .expect("person create facet includes inherited aliases");
        let plain_activity = projection
            .models()
            .values()
            .find(|model| model.id().label().as_str() == "plain-activity")
            .expect("ordered Phase-2 fixture projects plain-activity");
        let participant = plain_activity
            .create()
            .roles()
            .values()
            .find(|role| role.role().label().as_str() == "participant")
            .expect("plain-activity create facet includes inherited participant");
        (
            projection
                .projected_token_ordinal(&ProjectedTokenIdentity::Model(person.id().clone()))
                .expect("person has a generated model token"),
            projection
                .projected_token_ordinal(&ProjectedTokenIdentity::Field {
                    owner: person.id().clone(),
                    field: aliases.token().clone(),
                })
                .expect("person aliases has a generated field token"),
            projection
                .projected_token_ordinal(&ProjectedTokenIdentity::Model(
                    plain_activity.id().clone(),
                ))
                .expect("plain-activity has a generated model token"),
            projection
                .projected_token_ordinal(&ProjectedTokenIdentity::Role {
                    owner: plain_activity.id().clone(),
                    role: participant.role().clone(),
                })
                .expect("plain-activity participant has a generated role token"),
        )
    };
    let ordinals = token_ordinals(&projection);
    assert_eq!(
        token_ordinals(&foreign_projection),
        ordinals,
        "symbol-prefix changes must not reorder canonical generated tokens",
    );

    OrderedPhase2EmittedFixture {
        package,
        foreign_package,
        person_model_ordinal: ordinals.0,
        person_aliases_field_ordinal: ordinals.1,
        plain_activity_model_ordinal: ordinals.2,
        plain_activity_participant_role_ordinal: ordinals.3,
    }
}

static TEMP_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = TEMP_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "typebridge-c-abi-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("unique C ABI test directory is created");
        Self(directory)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("C ABI test directory is removed");
    }
}

fn write_package(package: &GeneratedPackage, root: &Path) {
    for (relative, contents) in package.files() {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("generated path has a parent"))
            .expect("generated parent directory is created");
        fs::write(path, contents).expect("generated C package file is written");
    }
}

fn command_exists(program: &str) -> bool {
    Command::new(program).arg("--version").output().is_ok()
}

fn assert_cmake_cache_path(
    build_directory: &Path,
    variable: &str,
    expected: &Path,
    staged_prefix: &Path,
) {
    let cache = fs::read_to_string(build_directory.join("CMakeCache.txt"))
        .expect("configured CMake cache is readable UTF-8");
    let marker = format!("{variable}:");
    let value = cache
        .lines()
        .find(|line| line.starts_with(&marker))
        .and_then(|line| line.split_once('='))
        .map(|(_, value)| value)
        .unwrap_or_else(|| panic!("CMake cache omitted {variable}"));
    let actual = fs::canonicalize(value)
        .unwrap_or_else(|error| panic!("cached {variable} path {value:?} is invalid: {error}"));
    let expected = fs::canonicalize(expected).expect("expected staged CMake package exists");
    let staged_prefix =
        fs::canonicalize(staged_prefix).expect("staged CMake package prefix exists");
    assert_eq!(actual, expected, "CMake resolved an unexpected {variable}");
    assert!(
        actual.starts_with(&staged_prefix),
        "CMake resolved {variable} outside the staged prefix: {}",
        actual.display()
    );
}

fn shared_consumer_required() -> bool {
    match std::env::var("TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER") {
        Err(std::env::VarError::NotPresent) => false,
        Ok(value) if value == "1" => true,
        Ok(value) => panic!(
            "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER must be unset or exactly `1`, got {value:?}"
        ),
        Err(error) => panic!("TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER is not Unicode: {error}"),
    }
}

fn native_library() -> Option<PathBuf> {
    let executable = std::env::current_exe().expect("current test executable path is available");
    let dependency_directory = executable
        .parent()
        .expect("test executable has a parent directory");
    let profile_directory = dependency_directory
        .parent()
        .expect("Cargo dependency directory has a profile parent");
    let filename = format!(
        "{}type_bridge_c{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    [
        dependency_directory.join(&filename),
        profile_directory.join(filename),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn native_library_or_skip(test_name: &str) -> Option<PathBuf> {
    if !shared_consumer_required() {
        return None;
    }

    Some(native_library().unwrap_or_else(|| {
        panic!(
            "{test_name} requires the TypeBridge C shared library; run `cargo build -p type-bridge-c --lib` before the test"
        )
    }))
}

#[cfg(windows)]
fn native_import_library(native_library: &Path) -> PathBuf {
    let directory = native_library
        .parent()
        .expect("native library has a parent directory");
    [
        directory.join("type_bridge_c.dll.lib"),
        directory.join("type_bridge_c.lib"),
        directory
            .parent()
            .expect("Cargo dependency directory has a profile parent")
            .join("type_bridge_c.dll.lib"),
        directory
            .parent()
            .expect("Cargo dependency directory has a profile parent")
            .join("type_bridge_c.lib"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .unwrap_or_else(|| {
        panic!(
            "Cargo did not build an import library beside {}",
            native_library.display()
        )
    })
}

fn rust_abi_layout() -> Vec<usize> {
    let mut layout = vec![
        1,
        4,
        TypeBridgeStatus::Ok as usize,
        TypeBridgeStatus::InvalidArgument as usize,
        TypeBridgeStatus::SchemaPackageRejected as usize,
        TypeBridgeStatus::Unsupported as usize,
        TypeBridgeStatus::ResourceLimit as usize,
        TypeBridgeStatus::ExecutionFailed as usize,
        TypeBridgeStatus::Cancelled as usize,
        TypeBridgeStatus::CommitOutcomeUnknown as usize,
        TypeBridgeStatus::InUse as usize,
        TypeBridgeStatus::Panic as usize,
        1,  // generated alias-preflight descriptor version
        1,  // bytes
        2,  // projected token
        3,  // schema package
        4,  // runtime
        5,  // database
        6,  // read transaction
        7,  // write transaction
        8,  // cancellation
        9,  // projected value
        10, // projected reference
        11, // projected create
        12, // projected thing
        13, // projected-value pointer array
        14, // projected-reference pointer array
        15, // schema diagnostics
        16, // execution diagnostics
        17, // complete generated create-args graph
        18, // query session
        19, // query binding
        20, // query field
        21, // query role
        22, // query predicate
        23, // query order
        24, // query selection
        25, // query
        26, // query terminal
        27, // query result
        28, // query remote context
        29, // query remote pending request
        30, // query remote claim
        31, // query function
        32, // query function value
        33, // query function call
        1,  // generated create graph version
        1020,
        65_535,
        1, // scalar value member
        2, // sequence value member
        3, // scalar reference member
        4, // sequence reference member
        1, // projected token version
        1, // model token
        2, // field token
        3, // role token
        4, // function token
        1, // query descriptor version
        1, // query execution-limits version
        16,
        16,
        256,
        256,
        65_535,
        128,
        30_000,
        65_536,
        33_554_432,
        65_536,
        65_536,
        65_536,
        65_536,
        3,
        33_554_432,
        1,  // exact match
        2,  // subtype-inclusive match
        1,  // equal
        2,  // not equal
        3,  // less than
        4,  // less than or equal
        5,  // greater than
        6,  // greater than or equal
        7,  // contains
        8,  // starts with
        9,  // ends with
        10, // regex
        1,  // predicate and
        2,  // predicate or
        3,  // predicate not
        1,  // ascending
        2,  // descending
        1,  // missing reject
        2,  // missing first
        3,  // missing last
        1,  // select one
        2,  // select collect
        1,  // positional shape
        2,  // named shape
        1,  // rows terminal
        2,  // page terminal
        3,  // count terminal
        4,  // exists terminal
        5,  // ungrouped reduction terminal
        6,  // single-field reduction terminal
        7,  // field-tuple reduction terminal
        8,  // first terminal
        1,  // exactly-one row cardinality
        2,  // bounded-many row cardinality
        1,  // count reducer
        2,  // sum reducer
        3,  // min reducer
        4,  // max reducer
        5,  // mean reducer
        6,  // median reducer
        7,  // standard-deviation reducer
        1,  // rows result
        2,  // page result
        3,  // count result
        4,  // ungrouped reduction result
        5,  // single-field reduction result
        6,  // field-tuple reduction result
        7,  // exists result
        0,  // no reduction group
        1,  // thing reduction group
        2,  // field reduction group
        3,  // field-tuple reduction group
        1,  // reduced count
        2,  // reduced long
        3,  // reduced double
        1,  // function binding argument
        2,  // function scalar-value argument
        3,  // function prior-call argument
        TypeBridgeExecutionDiagnosticCategory::InvalidInput as usize,
        TypeBridgeExecutionDiagnosticCategory::UnsupportedCapability as usize,
        TypeBridgeExecutionDiagnosticCategory::ResourceLimit as usize,
        TypeBridgeExecutionDiagnosticCategory::Integrity as usize,
        TypeBridgeExecutionDiagnosticCategory::Provider as usize,
        TypeBridgeExecutionDiagnosticCategory::Transaction as usize,
        TypeBridgeExecutionDiagnosticCategory::Cancelled as usize,
        TypeBridgeExecutionDiagnosticCategory::Internal as usize,
        TypeBridgeExecutionDiagnosticPathKind::Argument as usize,
        TypeBridgeExecutionDiagnosticPathKind::Index as usize,
        TypeBridgeExecutionDiagnosticPathKind::Type as usize,
        TypeBridgeExecutionDiagnosticPathKind::Field as usize,
        TypeBridgeExecutionDiagnosticPathKind::Role as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryRequest as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryPlan as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryOperation as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryPredicate as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryOutput as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryProviderEvidence as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryResult as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryBinding as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryField as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryRole as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryRoleEdge as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryOutputSlot as usize,
        TypeBridgeExecutionDiagnosticPathKind::QueryOutputName as usize,
        TypeBridgeExecutionDiagnosticPathKind::ContractField as usize,
        TypeBridgeExecutionDiagnosticPathKind::ContractIdentity as usize,
        TypeBridgeExecutionDiagnosticPathKind::Unknown as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Boolean as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Count as usize,
        TypeBridgeExecutionDiagnosticDetailKind::ByteCount as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Capability as usize,
        TypeBridgeExecutionDiagnosticDetailKind::ValueType as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Type as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Field as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Role as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Fingerprint as usize,
        TypeBridgeExecutionDiagnosticDetailKind::ProviderOperation as usize,
        TypeBridgeExecutionDiagnosticDetailKind::CommitOutcome as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Text as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Signed as usize,
        TypeBridgeExecutionDiagnosticDetailKind::TextList as usize,
        TypeBridgeExecutionDiagnosticDetailKind::Unknown as usize,
        size_of::<TypeBridgeByteView>(),
        align_of::<TypeBridgeByteView>(),
        offset_of!(TypeBridgeByteView, data),
        offset_of!(TypeBridgeByteView, length),
        size_of::<TypeBridgeSchemaPackageDescriptorV1>(),
        align_of::<TypeBridgeSchemaPackageDescriptorV1>(),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, struct_size),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, abi_major),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, abi_minor),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, schema_authority_json),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, declared_schema_json),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, runtime_projection_json),
        offset_of!(
            TypeBridgeSchemaPackageDescriptorV1,
            semantic_fingerprint_json
        ),
        offset_of!(
            TypeBridgeSchemaPackageDescriptorV1,
            binding_fingerprint_json
        ),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, managed_scope),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, semantic_profile),
        offset_of!(TypeBridgeSchemaPackageDescriptorV1, reserved),
        1, // chunked byte-view version
        size_of::<TypeBridgeChunkedByteViewV1>(),
        align_of::<TypeBridgeChunkedByteViewV1>(),
        offset_of!(TypeBridgeChunkedByteViewV1, struct_size),
        offset_of!(TypeBridgeChunkedByteViewV1, version),
        offset_of!(TypeBridgeChunkedByteViewV1, chunks),
        offset_of!(TypeBridgeChunkedByteViewV1, chunk_count),
        offset_of!(TypeBridgeChunkedByteViewV1, total_length),
        offset_of!(TypeBridgeChunkedByteViewV1, reserved),
        size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        align_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, struct_size),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, abi_major),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, abi_minor),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, reserved0),
        offset_of!(
            TypeBridgeSchemaPackageChunkedDescriptorV1,
            schema_authority_json
        ),
        offset_of!(
            TypeBridgeSchemaPackageChunkedDescriptorV1,
            declared_schema_json
        ),
        offset_of!(
            TypeBridgeSchemaPackageChunkedDescriptorV1,
            runtime_projection_json
        ),
        offset_of!(
            TypeBridgeSchemaPackageChunkedDescriptorV1,
            semantic_fingerprint_json
        ),
        offset_of!(
            TypeBridgeSchemaPackageChunkedDescriptorV1,
            binding_fingerprint_json
        ),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, managed_scope),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, semantic_profile),
        offset_of!(TypeBridgeSchemaPackageChunkedDescriptorV1, reserved),
    ];
    layout.extend([
        size_of::<TypeBridgeProjectedTokenV1>(),
        align_of::<TypeBridgeProjectedTokenV1>(),
        offset_of!(TypeBridgeProjectedTokenV1, struct_size),
        offset_of!(TypeBridgeProjectedTokenV1, version),
        offset_of!(TypeBridgeProjectedTokenV1, kind),
        offset_of!(TypeBridgeProjectedTokenV1, ordinal),
        offset_of!(TypeBridgeProjectedTokenV1, projection_digest),
        offset_of!(TypeBridgeProjectedTokenV1, reserved),
        size_of::<TypeBridgeGeneratedOpaqueInputV1>(),
        align_of::<TypeBridgeGeneratedOpaqueInputV1>(),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, struct_size),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, version),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, kind),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, reserved0),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, pointer),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, count),
        offset_of!(TypeBridgeGeneratedOpaqueInputV1, reserved),
        size_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
        align_of::<TypeBridgeGeneratedCreateHandleChunkV1>(),
        offset_of!(TypeBridgeGeneratedCreateHandleChunkV1, struct_size),
        offset_of!(TypeBridgeGeneratedCreateHandleChunkV1, version),
        offset_of!(TypeBridgeGeneratedCreateHandleChunkV1, values),
        offset_of!(TypeBridgeGeneratedCreateHandleChunkV1, count),
        offset_of!(TypeBridgeGeneratedCreateHandleChunkV1, next),
        offset_of!(TypeBridgeGeneratedCreateHandleChunkV1, reserved),
        size_of::<TypeBridgeGeneratedCreateMemberV1>(),
        align_of::<TypeBridgeGeneratedCreateMemberV1>(),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, struct_size),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, version),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, kind),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, reserved0),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, args_offset),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, token),
        offset_of!(TypeBridgeGeneratedCreateMemberV1, reserved),
        size_of::<TypeBridgeGeneratedCreateArgsGraphV1>(),
        align_of::<TypeBridgeGeneratedCreateArgsGraphV1>(),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, struct_size),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, version),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, args),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, args_size),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, members),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, member_count),
        offset_of!(TypeBridgeGeneratedCreateArgsGraphV1, reserved),
        size_of::<TypeBridgeGeneratedOutputRangeV1>(),
        align_of::<TypeBridgeGeneratedOutputRangeV1>(),
        offset_of!(TypeBridgeGeneratedOutputRangeV1, struct_size),
        offset_of!(TypeBridgeGeneratedOutputRangeV1, version),
        offset_of!(TypeBridgeGeneratedOutputRangeV1, pointer),
        offset_of!(TypeBridgeGeneratedOutputRangeV1, length),
        offset_of!(TypeBridgeGeneratedOutputRangeV1, reserved),
        size_of::<TypeBridgeProjectedFieldInputV1>(),
        align_of::<TypeBridgeProjectedFieldInputV1>(),
        offset_of!(TypeBridgeProjectedFieldInputV1, struct_size),
        offset_of!(TypeBridgeProjectedFieldInputV1, version),
        offset_of!(TypeBridgeProjectedFieldInputV1, field),
        offset_of!(TypeBridgeProjectedFieldInputV1, values),
        offset_of!(TypeBridgeProjectedFieldInputV1, value_count),
        offset_of!(TypeBridgeProjectedFieldInputV1, reserved),
        size_of::<TypeBridgeProjectedRoleInputV1>(),
        align_of::<TypeBridgeProjectedRoleInputV1>(),
        offset_of!(TypeBridgeProjectedRoleInputV1, struct_size),
        offset_of!(TypeBridgeProjectedRoleInputV1, version),
        offset_of!(TypeBridgeProjectedRoleInputV1, role),
        offset_of!(TypeBridgeProjectedRoleInputV1, references),
        offset_of!(TypeBridgeProjectedRoleInputV1, reference_count),
        offset_of!(TypeBridgeProjectedRoleInputV1, reserved),
        size_of::<TypeBridgeProjectedReferenceDescriptorV1>(),
        align_of::<TypeBridgeProjectedReferenceDescriptorV1>(),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, struct_size),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, version),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, model),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, iid),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, keys),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, key_count),
        offset_of!(TypeBridgeProjectedReferenceDescriptorV1, reserved),
        size_of::<TypeBridgeProjectedCreateDescriptorV1>(),
        align_of::<TypeBridgeProjectedCreateDescriptorV1>(),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, struct_size),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, version),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, model),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, fields),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, field_count),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, roles),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, role_count),
        offset_of!(TypeBridgeProjectedCreateDescriptorV1, reserved),
        size_of::<TypeBridgeProjectedThingDescriptorV1>(),
        align_of::<TypeBridgeProjectedThingDescriptorV1>(),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, struct_size),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, version),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, model),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, iid),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, fields),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, field_count),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, roles),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, role_count),
        offset_of!(TypeBridgeProjectedThingDescriptorV1, reserved),
        size_of::<TypeBridgeExecutionDiagnosticViewV1>(),
        align_of::<TypeBridgeExecutionDiagnosticViewV1>(),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, struct_size),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, version),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, category),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, reserved0),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, code),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, message),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, path_count),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, detail_count),
        offset_of!(TypeBridgeExecutionDiagnosticViewV1, reserved),
        size_of::<TypeBridgeExecutionDiagnosticPathViewV1>(),
        align_of::<TypeBridgeExecutionDiagnosticPathViewV1>(),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, struct_size),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, kind),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, index),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, primary),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, secondary),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, tertiary),
        offset_of!(TypeBridgeExecutionDiagnosticPathViewV1, reserved),
        size_of::<TypeBridgeExecutionDiagnosticDetailViewV1>(),
        align_of::<TypeBridgeExecutionDiagnosticDetailViewV1>(),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, struct_size),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, kind),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, boolean_value),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, reserved0),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, unsigned_value),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, key),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, primary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, secondary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, tertiary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, quaternary),
        offset_of!(TypeBridgeExecutionDiagnosticDetailViewV1, reserved),
        size_of::<TypeBridgeQueryFunctionArgumentV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, struct_size),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, version),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, kind),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, reserved0),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, binding),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, value),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, call),
        offset_of!(TypeBridgeQueryFunctionArgumentV1, reserved),
        size_of::<TypeBridgeQueryFunctionArgumentMemberV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentMemberV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentMemberV1, struct_size),
        offset_of!(TypeBridgeQueryFunctionArgumentMemberV1, version),
        offset_of!(TypeBridgeQueryFunctionArgumentMemberV1, args_offset),
        offset_of!(TypeBridgeQueryFunctionArgumentMemberV1, reserved),
        size_of::<TypeBridgeQueryFunctionArgumentsHeaderV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentsHeaderV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentsHeaderV1, struct_size),
        offset_of!(TypeBridgeQueryFunctionArgumentsHeaderV1, version),
        offset_of!(TypeBridgeQueryFunctionArgumentsHeaderV1, reserved),
        size_of::<TypeBridgeQueryFunctionArgumentsGraphV1>(),
        align_of::<TypeBridgeQueryFunctionArgumentsGraphV1>(),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, struct_size),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, version),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, args),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, args_size),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, members),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, member_count),
        offset_of!(TypeBridgeQueryFunctionArgumentsGraphV1, reserved),
        size_of::<TypeBridgeQueryOrderDescriptorV1>(),
        align_of::<TypeBridgeQueryOrderDescriptorV1>(),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, struct_size),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, version),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, field),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, expected_field),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, direction),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, missing),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, reserved0),
        offset_of!(TypeBridgeQueryOrderDescriptorV1, reserved),
        size_of::<TypeBridgeQuerySelectionDescriptorV1>(),
        align_of::<TypeBridgeQuerySelectionDescriptorV1>(),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, struct_size),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, version),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, binding),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, expected_model),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, expected_mode),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, kind),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, distinct),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, reserved0),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, orders),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, order_count),
        offset_of!(TypeBridgeQuerySelectionDescriptorV1, reserved),
        size_of::<TypeBridgeQueryShapeSlotV1>(),
        align_of::<TypeBridgeQueryShapeSlotV1>(),
        offset_of!(TypeBridgeQueryShapeSlotV1, struct_size),
        offset_of!(TypeBridgeQueryShapeSlotV1, version),
        offset_of!(TypeBridgeQueryShapeSlotV1, selection),
        offset_of!(TypeBridgeQueryShapeSlotV1, expected_model),
        offset_of!(TypeBridgeQueryShapeSlotV1, expected_mode),
        offset_of!(TypeBridgeQueryShapeSlotV1, expected_kind),
        offset_of!(TypeBridgeQueryShapeSlotV1, reserved0),
        offset_of!(TypeBridgeQueryShapeSlotV1, name),
        offset_of!(TypeBridgeQueryShapeSlotV1, reserved),
        size_of::<TypeBridgeQueryDescriptorV1>(),
        align_of::<TypeBridgeQueryDescriptorV1>(),
        offset_of!(TypeBridgeQueryDescriptorV1, struct_size),
        offset_of!(TypeBridgeQueryDescriptorV1, version),
        offset_of!(TypeBridgeQueryDescriptorV1, shape_kind),
        offset_of!(TypeBridgeQueryDescriptorV1, reserved0),
        offset_of!(TypeBridgeQueryDescriptorV1, slots),
        offset_of!(TypeBridgeQueryDescriptorV1, slot_count),
        offset_of!(TypeBridgeQueryDescriptorV1, reserved),
        size_of::<TypeBridgeQueryReducerV1>(),
        align_of::<TypeBridgeQueryReducerV1>(),
        offset_of!(TypeBridgeQueryReducerV1, struct_size),
        offset_of!(TypeBridgeQueryReducerV1, version),
        offset_of!(TypeBridgeQueryReducerV1, kind),
        offset_of!(TypeBridgeQueryReducerV1, reserved0),
        offset_of!(TypeBridgeQueryReducerV1, input),
        offset_of!(TypeBridgeQueryReducerV1, expected_field),
        offset_of!(TypeBridgeQueryReducerV1, reserved),
        size_of::<TypeBridgeQueryFieldReferenceV1>(),
        align_of::<TypeBridgeQueryFieldReferenceV1>(),
        offset_of!(TypeBridgeQueryFieldReferenceV1, struct_size),
        offset_of!(TypeBridgeQueryFieldReferenceV1, version),
        offset_of!(TypeBridgeQueryFieldReferenceV1, field),
        offset_of!(TypeBridgeQueryFieldReferenceV1, expected_field),
        offset_of!(TypeBridgeQueryFieldReferenceV1, reserved),
        size_of::<TypeBridgeQueryTerminalDescriptorV1>(),
        align_of::<TypeBridgeQueryTerminalDescriptorV1>(),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, struct_size),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, version),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, kind),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, cardinality),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, root),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, expected_root_model),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, expected_root_mode),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, reserved1),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, orders),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, order_count),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, offset),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, limit),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, include_total),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, reserved0),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, group_binding),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, expected_group_model),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, expected_group_mode),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, reserved2),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, group_fields),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, group_field_count),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, reducers),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, reducer_count),
        offset_of!(TypeBridgeQueryTerminalDescriptorV1, reserved),
        size_of::<TypeBridgeQueryExecutionLimitsV1>(),
        align_of::<TypeBridgeQueryExecutionLimitsV1>(),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, struct_size),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, version),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, timeout_milliseconds),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, items),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, bytes),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, graph_nodes),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, attribute_values),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, collection_members),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, role_players),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, statements),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, reserved0),
        offset_of!(TypeBridgeQueryExecutionLimitsV1, reserved),
        size_of::<TypeBridgeQueryPageMetadataV1>(),
        align_of::<TypeBridgeQueryPageMetadataV1>(),
        offset_of!(TypeBridgeQueryPageMetadataV1, struct_size),
        offset_of!(TypeBridgeQueryPageMetadataV1, version),
        offset_of!(TypeBridgeQueryPageMetadataV1, offset),
        offset_of!(TypeBridgeQueryPageMetadataV1, limit),
        offset_of!(TypeBridgeQueryPageMetadataV1, has_total),
        offset_of!(TypeBridgeQueryPageMetadataV1, reserved0),
        offset_of!(TypeBridgeQueryPageMetadataV1, total),
        offset_of!(TypeBridgeQueryPageMetadataV1, reserved),
        size_of::<TypeBridgeQueryReducedValueMetadataV1>(),
        align_of::<TypeBridgeQueryReducedValueMetadataV1>(),
        offset_of!(TypeBridgeQueryReducedValueMetadataV1, struct_size),
        offset_of!(TypeBridgeQueryReducedValueMetadataV1, version),
        offset_of!(TypeBridgeQueryReducedValueMetadataV1, kind),
        offset_of!(TypeBridgeQueryReducedValueMetadataV1, present),
        offset_of!(TypeBridgeQueryReducedValueMetadataV1, reserved0),
        offset_of!(TypeBridgeQueryReducedValueMetadataV1, reserved),
    ]);
    layout
}

fn parse_layout(output: &[u8], compiler: &str) -> Vec<usize> {
    let output = std::str::from_utf8(output)
        .unwrap_or_else(|error| panic!("{compiler} layout probe emitted non-UTF-8: {error}"));
    output
        .split_whitespace()
        .map(|value| {
            value.parse::<usize>().unwrap_or_else(|error| {
                panic!("invalid {compiler} layout value {value:?}: {error}")
            })
        })
        .collect()
}

fn expected_exported_symbols() -> BTreeSet<String> {
    ABI_1_3_EXPORTED_SYMBOLS
        .into_iter()
        .chain(ABI_1_4_ADDED_EXPORTED_SYMBOLS)
        .map(str::to_owned)
        .collect()
}

#[test]
fn abi_1_4_preserves_the_exact_frozen_abi_1_2_and_abi_1_3_export_subsets() {
    let current = expected_exported_symbols();
    let abi_1_3 = ABI_1_3_EXPORTED_SYMBOLS
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let abi_1_2 = ABI_1_2_EXPORTED_SYMBOLS
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let additions = ABI_1_4_ADDED_EXPORTED_SYMBOLS
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    assert_eq!(abi_1_2.len(), 109, "ABI 1.2 export ledger has duplicates");
    assert_eq!(abi_1_3.len(), 180, "ABI 1.3 export ledger has duplicates");
    assert_eq!(
        additions.len(),
        45,
        "ABI 1.4 addition ledger has duplicates"
    );
    assert_eq!(current.len(), 225, "ABI 1.4 export ledger has duplicates");

    let missing = abi_1_2.difference(&abi_1_3).cloned().collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "ABI 1.3 removed or renamed ABI 1.2 exports: {missing:?}",
    );
    assert_eq!(
        abi_1_3.difference(&abi_1_2).count(),
        71,
        "ABI 1.3 must remain exactly 71 additive exports over ABI 1.2",
    );
    let missing = abi_1_3.difference(&current).cloned().collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "ABI 1.4 removed or renamed ABI 1.3 exports: {missing:?}",
    );
    assert!(
        additions.is_disjoint(&abi_1_3),
        "ABI 1.4 additions must not rename predecessor exports",
    );
    assert_eq!(
        current
            .difference(&abi_1_3)
            .cloned()
            .collect::<BTreeSet<_>>(),
        additions,
        "ABI 1.4 must remain exactly the frozen 45-function addition",
    );
}

#[cfg(windows)]
fn windows_dumpbin_exports(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 4
                || fields[0].parse::<u32>().is_err()
                || !fields[1].bytes().all(|byte| byte.is_ascii_hexdigit())
                || !fields[2].bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return None;
            }
            let mut name = fields[3].trim_start_matches('_');
            if let Some((base, suffix)) = name.rsplit_once('@')
                && suffix.bytes().all(|byte| byte.is_ascii_digit())
            {
                name = base;
            }
            Some(name.to_owned())
        })
        .collect()
}

unsafe fn copied_view(
    package: *const TypeBridgeSchemaPackage,
    accessor: unsafe extern "C" fn(
        *const TypeBridgeSchemaPackage,
        *mut TypeBridgeByteView,
    ) -> TypeBridgeStatus,
) -> Vec<u8> {
    let mut output = TypeBridgeByteView {
        data: NonNull::<u8>::dangling().as_ptr(),
        length: usize::MAX,
    };
    // SAFETY: `package` is live for the call and `output` is writable.
    assert_eq!(
        unsafe { accessor(package, &mut output) },
        TypeBridgeStatus::Ok
    );
    assert!(!output.data.is_null());
    // SAFETY: a successful accessor returns bytes owned by the live package.
    unsafe { std::slice::from_raw_parts(output.data, output.length) }.to_vec()
}

unsafe fn diagnostic_text(diagnostics: *const TypeBridgeDiagnostics) -> String {
    let mut output = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: `diagnostics` is live for the call and `output` is writable.
    assert_eq!(
        unsafe { type_bridge_diagnostics_json(diagnostics, &mut output) },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: the successful accessor returns live UTF-8 diagnostic JSON.
    String::from_utf8(unsafe { std::slice::from_raw_parts(output.data, output.length) }.to_vec())
        .expect("diagnostic JSON is UTF-8")
}

unsafe fn execution_view_text(view: TypeBridgeByteView) -> String {
    if view.length == 0 {
        assert!(view.data.is_null());
        return String::new();
    }
    assert!(!view.data.is_null());
    // SAFETY: the view borrows a live execution-diagnostic handle for this assertion.
    String::from_utf8(unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec())
        .expect("execution-diagnostic text is UTF-8")
}

unsafe fn one_execution_diagnostic(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
) -> TypeBridgeExecutionDiagnosticViewV1 {
    let mut count = usize::MAX;
    // SAFETY: the diagnostics handle remains live and `count` is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_count(diagnostics, &mut count) },
        TypeBridgeStatus::Ok,
    );
    assert_eq!(count, 1);
    let mut view = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticViewV1>::uninit();
    // SAFETY: the diagnostics handle remains live and the output storage is writable.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_get_v1(diagnostics, 0, view.as_mut_ptr()) },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: a successful accessor initialized the complete view.
    unsafe { view.assume_init() }
}

unsafe fn close_execution_diagnostics(diagnostics: &mut *mut TypeBridgeExecutionDiagnostics) {
    // SAFETY: this slot exclusively owns the execution-diagnostic handle.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_close(diagnostics) },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());
    // SAFETY: repeated close of the now-null slot is explicitly idempotent.
    assert_eq!(
        unsafe { type_bridge_execution_diagnostics_close(diagnostics) },
        TypeBridgeStatus::Ok,
    );
}

unsafe fn assert_missing_semantic_fingerprint_diagnostic(
    diagnostics: *const TypeBridgeExecutionDiagnostics,
) {
    // SAFETY: the caller retains the live diagnostic handle for every borrowed view below.
    let view = unsafe { one_execution_diagnostic(diagnostics) };
    assert_eq!(
        view.category,
        TypeBridgeExecutionDiagnosticCategory::Integrity
    );
    // SAFETY: the diagnostic handle remains live while this text is copied.
    assert_eq!(
        unsafe { execution_view_text(view.code) },
        "projection_evidence_mismatch"
    );
    assert_eq!(view.path_count, 3);
    assert_eq!(view.detail_count, 3);

    for (index, kind, expected_index, expected_primary) in [
        (
            0,
            TypeBridgeExecutionDiagnosticPathKind::Argument,
            0,
            "projection_evidence",
        ),
        (1, TypeBridgeExecutionDiagnosticPathKind::Index, 0, ""),
        (
            2,
            TypeBridgeExecutionDiagnosticPathKind::ContractIdentity,
            0,
            "semantic_schema_fingerprint",
        ),
    ] {
        let mut path = std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticPathViewV1>::uninit();
        // SAFETY: the diagnostics handle remains live and the output storage is writable.
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_path_get_v1(
                    diagnostics,
                    0,
                    index,
                    path.as_mut_ptr(),
                )
            },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: a successful accessor initialized the complete path view.
        let path = unsafe { path.assume_init() };
        assert_eq!(path.kind, kind);
        assert_eq!(path.index, expected_index);
        // SAFETY: the diagnostic handle remains live while this text is copied.
        assert_eq!(
            unsafe { execution_view_text(path.primary) },
            expected_primary
        );
    }

    for (index, key, kind, boolean_value, unsigned_value) in [
        (
            0,
            "actual_occurrence_count",
            TypeBridgeExecutionDiagnosticDetailKind::Count,
            0,
            0,
        ),
        (
            1,
            "expected_occurrence_count",
            TypeBridgeExecutionDiagnosticDetailKind::Count,
            0,
            1,
        ),
        (
            2,
            "foreign_package",
            TypeBridgeExecutionDiagnosticDetailKind::Boolean,
            0,
            0,
        ),
    ] {
        let mut detail =
            std::mem::MaybeUninit::<TypeBridgeExecutionDiagnosticDetailViewV1>::uninit();
        // SAFETY: the diagnostics handle remains live and the output storage is writable.
        assert_eq!(
            unsafe {
                type_bridge_execution_diagnostics_detail_get_v1(
                    diagnostics,
                    0,
                    index,
                    detail.as_mut_ptr(),
                )
            },
            TypeBridgeStatus::Ok,
        );
        // SAFETY: a successful accessor initialized the complete detail view.
        let detail = unsafe { detail.assume_init() };
        assert_eq!(detail.kind, kind);
        assert_eq!(detail.boolean_value, boolean_value);
        assert_eq!(detail.unsigned_value, unsigned_value);
        // SAFETY: the diagnostic handle remains live while this text is copied.
        assert_eq!(unsafe { execution_view_text(detail.key) }, key);
    }
}

fn assert_v2_flat_rejected(
    descriptor: &TypeBridgeSchemaPackageDescriptorV1,
    status: TypeBridgeStatus,
    category: TypeBridgeExecutionDiagnosticCategory,
    code: &str,
    path_count: usize,
    detail_count: usize,
) {
    let mut package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    let mut diagnostics = NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
    // SAFETY: the descriptor and both output slots remain live for the complete call.
    assert_eq!(
        unsafe { type_bridge_schema_package_open_v2(descriptor, &mut package, &mut diagnostics) },
        status,
    );
    assert!(package.is_null());
    assert!(!diagnostics.is_null());
    // SAFETY: the failed call returned one live typed-diagnostic handle.
    let view = unsafe { one_execution_diagnostic(diagnostics) };
    assert_eq!(view.category, category);
    assert_eq!(view.path_count, path_count);
    assert_eq!(view.detail_count, detail_count);
    // SAFETY: the diagnostic handle remains live while its borrowed code is copied.
    assert_eq!(unsafe { execution_view_text(view.code) }, code);
    // SAFETY: this slot uniquely owns the returned diagnostic handle.
    unsafe { close_execution_diagnostics(&mut diagnostics) };
}

fn assert_v2_chunked_rejected(
    descriptor: &TypeBridgeSchemaPackageChunkedDescriptorV1,
    status: TypeBridgeStatus,
    category: TypeBridgeExecutionDiagnosticCategory,
    code: &str,
    path_count: usize,
    detail_count: usize,
) {
    let mut package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    let mut diagnostics = NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
    // SAFETY: the descriptor graph and both output slots remain live for the complete call.
    assert_eq!(
        unsafe {
            type_bridge_schema_package_open_chunked_v2(descriptor, &mut package, &mut diagnostics)
        },
        status,
    );
    assert!(package.is_null());
    assert!(!diagnostics.is_null());
    // SAFETY: the failed call returned one live typed-diagnostic handle.
    let view = unsafe { one_execution_diagnostic(diagnostics) };
    assert_eq!(view.category, category);
    assert_eq!(view.path_count, path_count);
    assert_eq!(view.detail_count, detail_count);
    // SAFETY: the diagnostic handle remains live while its borrowed code is copied.
    assert_eq!(unsafe { execution_view_text(view.code) }, code);
    // SAFETY: this slot uniquely owns the returned diagnostic handle.
    unsafe { close_execution_diagnostics(&mut diagnostics) };
}

fn assert_rejected(
    descriptor: &TypeBridgeSchemaPackageDescriptorV1,
    status: TypeBridgeStatus,
    code: &str,
) {
    let mut package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    let mut diagnostics = NonNull::<TypeBridgeDiagnostics>::dangling().as_ptr();
    // SAFETY: the descriptor and both output slots are readable/writable for the call.
    let actual =
        unsafe { type_bridge_schema_package_open_v1(descriptor, &mut package, &mut diagnostics) };
    assert_eq!(actual, status);
    assert!(
        package.is_null(),
        "failed opens must clear the package slot"
    );
    assert!(
        !diagnostics.is_null(),
        "ordinary rejection must return structured diagnostics"
    );
    // SAFETY: the failed open returned a live diagnostic handle.
    let json = unsafe { diagnostic_text(diagnostics) };
    assert!(
        json.contains(&format!("\"code\":\"{code}\"")),
        "diagnostics did not contain {code}: {json}",
    );
    // SAFETY: ownership of the returned handle is released through its exact close family.
    assert_eq!(
        unsafe { type_bridge_diagnostics_close(&mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
    assert!(diagnostics.is_null());
    // SAFETY: repeated close of the now-null slot is explicitly idempotent.
    assert_eq!(
        unsafe { type_bridge_diagnostics_close(&mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
}

#[test]
fn public_c_header_matches_the_rust_abi_layout_and_constants() {
    // The ordinary Windows workspace test lane does not enter a Visual Studio
    // developer environment. The dedicated C ABI lane opts in after doing so.
    #[cfg(windows)]
    if !shared_consumer_required() {
        return;
    }

    let stage = TempDirectory::new();
    let source = stage.path().join("abi-layout.c");
    let executable = stage.path().join(if cfg!(windows) {
        "abi-layout.exe"
    } else {
        "abi-layout"
    });
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <stdio.h>

#include <typebridge/type_bridge.h>

#if defined(__cplusplus)
#define TYPE_BRIDGE_ALIGNOF(type) alignof(type)
#elif defined(_MSC_VER)
#define TYPE_BRIDGE_ALIGNOF(type) __alignof(type)
#else
#define TYPE_BRIDGE_ALIGNOF(type) _Alignof(type)
#endif

#define PRINT_VALUE(value) \
  do { printf("%zu\n", (size_t)(value)); } while (0)

int main(void) {
  PRINT_VALUE(TYPE_BRIDGE_C_ABI_MAJOR);
  PRINT_VALUE(TYPE_BRIDGE_C_ABI_MINOR);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_OK);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_SCHEMA_PACKAGE_REJECTED);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_UNSUPPORTED);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_CANCELLED);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_COMMIT_OUTCOME_UNKNOWN);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_IN_USE);
  PRINT_VALUE(TYPE_BRIDGE_STATUS_PANIC);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_ALIAS_PREFLIGHT_VERSION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_BYTES);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_SCHEMA_PACKAGE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_RUNTIME);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_DATABASE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_READ_TRANSACTION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_WRITE_TRANSACTION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_VALUE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_REFERENCE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_THING);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_DIAGNOSTICS);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_EXECUTION_DIAGNOSTICS);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_CREATE_ARGS_GRAPH);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_SESSION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_BINDING);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_PREDICATE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_ORDER);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_SELECTION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_TERMINAL);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_RESULT);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_CONTEXT);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_PENDING);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_CLAIM);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_VALUE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_CALL);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_GRAPH_VERSION);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_MEMBER_COUNT_MAX);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_MEMBER_VALUE_SCALAR);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_MEMBER_VALUE_SEQUENCE);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_MEMBER_REFERENCE_SCALAR);
  PRINT_VALUE(TYPE_BRIDGE_GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE);
  PRINT_VALUE(TYPE_BRIDGE_PROJECTED_TOKEN_VERSION);
  PRINT_VALUE(TYPE_BRIDGE_PROJECTED_TOKEN_MODEL);
  PRINT_VALUE(TYPE_BRIDGE_PROJECTED_TOKEN_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_PROJECTED_TOKEN_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_PROJECTED_TOKEN_FUNCTION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_VERSION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SELECTED_SLOT_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_ORDER_TERM_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_BOOLEAN_TERM_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_OUTPUT_NAME_BYTES_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_TIMEOUT_MILLISECONDS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_ITEMS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_BYTES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_GRAPH_NODES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_ATTRIBUTE_VALUES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_COLLECTION_MEMBERS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_ROLE_PLAYERS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_DEFAULT_STATEMENTS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REMOTE_ENVELOPE_BYTES_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_MATCH_EXACT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_MATCH_SUBTYPES);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_EQUAL);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_NOT_EQUAL);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN_OR_EQUAL);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_CONTAINS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_STARTS_WITH);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_ENDS_WITH);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_COMPARE_REGEX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_PREDICATE_AND);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_PREDICATE_OR);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_PREDICATE_NOT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SORT_ASCENDING);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SORT_DESCENDING);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_MISSING_REJECT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_MISSING_FIRST);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_MISSING_LAST);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SELECTION_ONE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SELECTION_COLLECT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SHAPE_POSITIONAL);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_SHAPE_NAMED);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_ROWS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_PAGE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_EXISTS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_REDUCE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELDS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_TERMINAL_FIRST);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_ROWS_EXACTLY_ONE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_SUM);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_MIN);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_MAX);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_MEAN);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_MEDIAN);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCER_STD);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_ROWS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_PAGE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_REDUCTION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_FIELD_REDUCTION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_FIELD_TUPLE_REDUCTION);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_RESULT_EXISTS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCTION_GROUP_NONE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCTION_GROUP_THING);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCTION_GROUP_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCTION_GROUP_FIELDS);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCED_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCED_LONG);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_REDUCED_DOUBLE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_BINDING);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_VALUE);
  PRINT_VALUE(TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_CALL);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_UNSUPPORTED_CAPABILITY);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PROVIDER);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_TRANSACTION);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_CANCELLED);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTERNAL);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_ARGUMENT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_INDEX);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_TYPE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_REQUEST);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PLAN);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OPERATION);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PREDICATE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PROVIDER_EVIDENCE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_RESULT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_BINDING);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_ROLE_EDGE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT_SLOT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT_NAME);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_IDENTITY);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_UNKNOWN);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_BOOLEAN);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_BYTE_COUNT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_CAPABILITY);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_VALUE_TYPE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TYPE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_FIELD);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_ROLE);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_FINGERPRINT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_PROVIDER_OPERATION);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COMMIT_OUTCOME);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_SIGNED);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT_LIST);
  PRINT_VALUE(TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_UNKNOWN);
  PRINT_VALUE(sizeof(type_bridge_byte_view_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_byte_view_t));
  PRINT_VALUE(offsetof(type_bridge_byte_view_t, data));
  PRINT_VALUE(offsetof(type_bridge_byte_view_t, length));
  PRINT_VALUE(sizeof(type_bridge_schema_package_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_schema_package_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t, abi_major));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t, abi_minor));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t,
                       schema_authority_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t,
                       declared_schema_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t,
                       runtime_projection_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t,
                       semantic_fingerprint_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t,
                       binding_fingerprint_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t, managed_scope));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t,
                       semantic_profile));
  PRINT_VALUE(offsetof(type_bridge_schema_package_descriptor_v1_t, reserved));
  PRINT_VALUE(TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION);
  PRINT_VALUE(sizeof(type_bridge_chunked_byte_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_chunked_byte_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_chunked_byte_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_chunked_byte_view_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_chunked_byte_view_v1_t, chunks));
  PRINT_VALUE(offsetof(type_bridge_chunked_byte_view_v1_t, chunk_count));
  PRINT_VALUE(offsetof(type_bridge_chunked_byte_view_v1_t, total_length));
  PRINT_VALUE(offsetof(type_bridge_chunked_byte_view_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_schema_package_chunked_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_schema_package_chunked_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       struct_size));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       abi_major));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       abi_minor));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       reserved0));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       schema_authority_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       declared_schema_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       runtime_projection_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       semantic_fingerprint_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       binding_fingerprint_json));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       managed_scope));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       semantic_profile));
  PRINT_VALUE(offsetof(type_bridge_schema_package_chunked_descriptor_v1_t,
                       reserved));
  PRINT_VALUE(sizeof(type_bridge_projected_token_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_projected_token_v1_t));
  PRINT_VALUE(offsetof(type_bridge_projected_token_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_projected_token_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_projected_token_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_projected_token_v1_t, ordinal));
  PRINT_VALUE(offsetof(type_bridge_projected_token_v1_t, projection_digest));
  PRINT_VALUE(offsetof(type_bridge_projected_token_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_generated_opaque_input_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_generated_opaque_input_v1_t));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, pointer));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, count));
  PRINT_VALUE(offsetof(type_bridge_generated_opaque_input_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_generated_create_handle_chunk_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_generated_create_handle_chunk_v1_t));
  PRINT_VALUE(offsetof(type_bridge_generated_create_handle_chunk_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_generated_create_handle_chunk_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_generated_create_handle_chunk_v1_t, values));
  PRINT_VALUE(offsetof(type_bridge_generated_create_handle_chunk_v1_t, count));
  PRINT_VALUE(offsetof(type_bridge_generated_create_handle_chunk_v1_t, next));
  PRINT_VALUE(offsetof(type_bridge_generated_create_handle_chunk_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_generated_create_member_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_generated_create_member_v1_t));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, args_offset));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, token));
  PRINT_VALUE(offsetof(type_bridge_generated_create_member_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_generated_create_args_graph_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_generated_create_args_graph_v1_t));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, args));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, args_size));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, members));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, member_count));
  PRINT_VALUE(offsetof(type_bridge_generated_create_args_graph_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_generated_output_range_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_generated_output_range_v1_t));
  PRINT_VALUE(offsetof(type_bridge_generated_output_range_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_generated_output_range_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_generated_output_range_v1_t, pointer));
  PRINT_VALUE(offsetof(type_bridge_generated_output_range_v1_t, length));
  PRINT_VALUE(offsetof(type_bridge_generated_output_range_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_projected_field_input_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_projected_field_input_v1_t));
  PRINT_VALUE(offsetof(type_bridge_projected_field_input_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_projected_field_input_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_projected_field_input_v1_t, field));
  PRINT_VALUE(offsetof(type_bridge_projected_field_input_v1_t, values));
  PRINT_VALUE(offsetof(type_bridge_projected_field_input_v1_t, value_count));
  PRINT_VALUE(offsetof(type_bridge_projected_field_input_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_projected_role_input_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_projected_role_input_v1_t));
  PRINT_VALUE(offsetof(type_bridge_projected_role_input_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_projected_role_input_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_projected_role_input_v1_t, role));
  PRINT_VALUE(offsetof(type_bridge_projected_role_input_v1_t, references));
  PRINT_VALUE(offsetof(type_bridge_projected_role_input_v1_t, reference_count));
  PRINT_VALUE(offsetof(type_bridge_projected_role_input_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_projected_reference_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_projected_reference_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, model));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, iid));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, keys));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, key_count));
  PRINT_VALUE(offsetof(type_bridge_projected_reference_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_projected_create_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_projected_create_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, model));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, fields));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, field_count));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, roles));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, role_count));
  PRINT_VALUE(offsetof(type_bridge_projected_create_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_projected_thing_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_projected_thing_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, model));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, iid));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, fields));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, field_count));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, roles));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, role_count));
  PRINT_VALUE(offsetof(type_bridge_projected_thing_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_execution_diagnostic_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_execution_diagnostic_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, category));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, code));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, message));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, path_count));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, detail_count));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_view_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_execution_diagnostic_path_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_execution_diagnostic_path_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, index));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, primary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, secondary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, tertiary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_path_view_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_execution_diagnostic_detail_view_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_execution_diagnostic_detail_view_v1_t));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, boolean_value));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, unsigned_value));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, key));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, primary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, secondary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, tertiary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, quaternary));
  PRINT_VALUE(offsetof(type_bridge_execution_diagnostic_detail_view_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_function_argument_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_argument_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, binding));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, value));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, call));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_function_argument_member_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_argument_member_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_member_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_member_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_member_v1_t, args_offset));
  PRINT_VALUE(offsetof(type_bridge_query_function_argument_member_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_function_arguments_header_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_arguments_header_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_header_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_header_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_header_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_function_arguments_graph_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_function_arguments_graph_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, args));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, args_size));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, members));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, member_count));
  PRINT_VALUE(offsetof(type_bridge_query_function_arguments_graph_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_order_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_order_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, field));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, expected_field));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, direction));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, missing));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_order_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_selection_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_selection_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, binding));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, expected_model));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, expected_mode));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, distinct));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, orders));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, order_count));
  PRINT_VALUE(offsetof(type_bridge_query_selection_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_shape_slot_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_shape_slot_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, selection));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, expected_model));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, expected_mode));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, expected_kind));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, name));
  PRINT_VALUE(offsetof(type_bridge_query_shape_slot_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, shape_kind));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, slots));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, slot_count));
  PRINT_VALUE(offsetof(type_bridge_query_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_reducer_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_reducer_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, input));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, expected_field));
  PRINT_VALUE(offsetof(type_bridge_query_reducer_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_field_reference_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_field_reference_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_field_reference_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_field_reference_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_field_reference_v1_t, field));
  PRINT_VALUE(offsetof(type_bridge_query_field_reference_v1_t, expected_field));
  PRINT_VALUE(offsetof(type_bridge_query_field_reference_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_terminal_descriptor_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_terminal_descriptor_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, cardinality));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, root));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, expected_root_model));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, expected_root_mode));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, reserved1));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, orders));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, order_count));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, offset));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, limit));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, include_total));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, group_binding));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, expected_group_model));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, expected_group_mode));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, reserved2));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, group_fields));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, group_field_count));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, reducers));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, reducer_count));
  PRINT_VALUE(offsetof(type_bridge_query_terminal_descriptor_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_execution_limits_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_execution_limits_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, timeout_milliseconds));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, items));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, bytes));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, graph_nodes));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, attribute_values));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, collection_members));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, role_players));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, statements));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_execution_limits_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_page_metadata_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_page_metadata_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, offset));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, limit));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, has_total));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, total));
  PRINT_VALUE(offsetof(type_bridge_query_page_metadata_v1_t, reserved));
  PRINT_VALUE(sizeof(type_bridge_query_reduced_value_metadata_v1_t));
  PRINT_VALUE(TYPE_BRIDGE_ALIGNOF(type_bridge_query_reduced_value_metadata_v1_t));
  PRINT_VALUE(offsetof(type_bridge_query_reduced_value_metadata_v1_t, struct_size));
  PRINT_VALUE(offsetof(type_bridge_query_reduced_value_metadata_v1_t, version));
  PRINT_VALUE(offsetof(type_bridge_query_reduced_value_metadata_v1_t, kind));
  PRINT_VALUE(offsetof(type_bridge_query_reduced_value_metadata_v1_t, present));
  PRINT_VALUE(offsetof(type_bridge_query_reduced_value_metadata_v1_t, reserved0));
  PRINT_VALUE(offsetof(type_bridge_query_reduced_value_metadata_v1_t, reserved));
  return 0;
}
"#,
    )
    .expect("C ABI layout probe is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let mut invocations = 0;

    #[cfg(not(windows))]
    for (compiler, standard, language, label) in [
        ("gcc", "c11", "c", "gcc C11"),
        ("clang", "c11", "c", "clang C11"),
        ("g++", "c++17", "c++", "g++ C++17"),
        ("clang++", "c++17", "c++", "clang++ C++17"),
    ] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .arg(format!("-std={standard}"))
            .args(["-Wall", "-Wextra", "-Werror", "-pedantic-errors"])
            .args(["-x", language])
            .arg("-I")
            .arg(&runtime_include)
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{label} failed to compile the ABI layout probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {label} layout probe: {error}"));
        assert!(
            output.status.success(),
            "{label} ABI layout probe failed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        assert_eq!(parse_layout(&output.stdout, label), rust_abi_layout());
    }

    #[cfg(windows)]
    for (compiler, standard, language, label) in [
        ("cl", "/std:c11", "/TC", "cl C11"),
        ("cl", "/std:c++17", "/TP", "cl C++17"),
        ("clang-cl", "/std:c11", "/TC", "clang-cl C11"),
        ("clang-cl", "/std:c++17", "/TP", "clang-cl C++17"),
    ] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let output = Command::new(compiler)
            .args(["/nologo", standard, "/W4", "/WX", language])
            .arg(format!("/I{}", runtime_include.display()))
            .arg(&source)
            .arg(format!("/Fe{}", executable.display()))
            .current_dir(stage.path())
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{label} failed to compile the ABI layout probe:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {label} layout probe: {error}"));
        assert!(
            output.status.success(),
            "{label} ABI layout probe failed: {}",
            String::from_utf8_lossy(&output.stderr),
        );
        assert_eq!(parse_layout(&output.stdout, label), rust_abi_layout());
    }

    assert!(invocations > 0, "no supported C compiler was available");
}

#[test]
fn shared_library_exports_only_the_frozen_c_abi() {
    let Some(library) = native_library_or_skip("shared_library_exports_only_the_frozen_c_abi")
    else {
        return;
    };

    #[cfg(target_os = "linux")]
    let actual = {
        let output = Command::new("nm")
            .args(["-D", "--defined-only", "--format=posix"])
            .arg(&library)
            .output()
            .expect("nm is required for the Linux C ABI export audit");
        assert!(
            output.status.success(),
            "nm failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout)
            .expect("nm output is UTF-8")
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
    };

    #[cfg(target_os = "macos")]
    let actual = {
        let output = Command::new("nm")
            .args(["-gjU"])
            .arg(&library)
            .output()
            .expect("nm is required for the macOS C ABI export audit");
        assert!(
            output.status.success(),
            "nm failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout)
            .expect("nm output is UTF-8")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| line.strip_prefix('_').unwrap_or(line).to_owned())
            .collect::<BTreeSet<_>>()
    };

    #[cfg(windows)]
    let actual = {
        let output = Command::new("dumpbin")
            .args(["/nologo", "/exports"])
            .arg(&library)
            .output()
            .expect("dumpbin is required for the Windows C ABI export audit");
        assert!(
            output.status.success(),
            "dumpbin failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        windows_dumpbin_exports(&String::from_utf8(output.stdout).expect("dumpbin output is UTF-8"))
    };

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let actual = expected_exported_symbols();

    assert_eq!(actual, expected_exported_symbols());
}

#[test]
fn shared_library_has_only_platform_dependencies_and_no_embedded_search_path() {
    let Some(library) = native_library_or_skip(
        "shared_library_has_only_platform_dependencies_and_no_embedded_search_path",
    ) else {
        return;
    };

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("readelf")
            .args(["-d"])
            .arg(&library)
            .output()
            .expect("readelf is required for the Linux shared-library audit");
        assert!(
            output.status.success(),
            "readelf failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        let report = String::from_utf8(output.stdout).expect("readelf output is UTF-8");
        assert!(
            !report.contains("(RPATH)") && !report.contains("(RUNPATH)"),
            "TypeBridge C shared library embeds a loader search path:\n{report}"
        );
        let dependencies = report
            .lines()
            .filter(|line| line.contains("(NEEDED)"))
            .map(|line| {
                line.split_once('[')
                    .and_then(|(_, tail)| tail.split_once(']'))
                    .map(|(name, _)| name)
                    .unwrap_or_else(|| panic!("invalid readelf NEEDED line: {line}"))
            })
            .collect::<BTreeSet<_>>();
        assert!(
            dependencies.iter().all(|name| {
                matches!(*name, "libgcc_s.so.1" | "libc.so.6" | "libm.so.6")
                    || name.starts_with("ld-linux-")
                    || name.starts_with("libc.musl-")
                    || name.starts_with("ld-musl-")
            }),
            "TypeBridge C shared library gained a non-platform dependency: {dependencies:?}"
        );
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("otool")
            .args(["-L"])
            .arg(&library)
            .output()
            .expect("otool is required for the macOS shared-library audit");
        assert!(
            output.status.success(),
            "otool -L failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        let report = String::from_utf8(output.stdout).expect("otool output is UTF-8");
        for dependency in report.lines().skip(1).filter_map(|line| {
            let path = line.split_whitespace().next()?;
            (!path.is_empty()).then_some(path)
        }) {
            assert!(
                dependency.starts_with("/usr/lib/")
                    || dependency.starts_with("/System/Library/Frameworks/"),
                "TypeBridge C shared library gained a non-platform dependency: {dependency}"
            );
        }
        let output = Command::new("otool")
            .args(["-l"])
            .arg(&library)
            .output()
            .expect("otool is required for the macOS RPATH audit");
        assert!(output.status.success(), "otool -l failed");
        let report = String::from_utf8(output.stdout).expect("otool output is UTF-8");
        assert!(
            !report.lines().any(|line| line.trim() == "cmd LC_RPATH"),
            "TypeBridge C shared library embeds LC_RPATH"
        );
    }

    #[cfg(windows)]
    {
        let output = Command::new("dumpbin")
            .args(["/nologo", "/dependents"])
            .arg(&library)
            .output()
            .expect("dumpbin is required for the Windows shared-library audit");
        assert!(
            output.status.success(),
            "dumpbin failed for {}: {}",
            library.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        let system_libraries = [
            "ADVAPI32.DLL",
            "BCRYPT.DLL",
            "CRYPT32.DLL",
            "KERNEL32.DLL",
            "MSVCRT.DLL",
            "NTDLL.DLL",
            "OLE32.DLL",
            "SHELL32.DLL",
            "USERENV.DLL",
            "VCRUNTIME140.DLL",
            "WS2_32.DLL",
            "UCRTBASE.DLL",
        ];
        let report = String::from_utf8(output.stdout).expect("dumpbin output is UTF-8");
        let dependencies = report
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.chars().any(char::is_whitespace)
                    && line.to_ascii_lowercase().ends_with(".dll")
            })
            .map(str::to_ascii_uppercase)
            .collect::<BTreeSet<_>>();
        assert!(
            dependencies.iter().all(|name| {
                system_libraries.contains(&name.as_str()) || name.starts_with("API-MS-WIN-")
            }),
            "TypeBridge C shared library gained a non-platform dependency: {dependencies:?}"
        );
    }
}

#[test]
fn clean_staged_cmake_consumer_finds_links_and_runs_generated_package() {
    let Some(library) = native_library_or_skip(
        "clean_staged_cmake_consumer_finds_links_and_runs_generated_package",
    ) else {
        return;
    };
    assert!(
        command_exists("cmake"),
        "cmake is required for the staged TypeBridge C consumer audit"
    );

    let fixture = emitted_fixture();
    let stage = TempDirectory::new();
    let runtime_build = stage.path().join("runtime-build");
    let install = stage.path().join("install");
    let generated_source = stage.path().join("generated-source");
    let generated_build = stage.path().join("generated-build");
    let rejected_consumer_source = stage.path().join("rejected-consumer-source");
    let rejected_consumer_build = stage.path().join("rejected-consumer-build");
    let consumer_source = stage.path().join("consumer-source");
    let consumer_build = stage.path().join("consumer-build");
    fs::create_dir_all(&rejected_consumer_source)
        .expect("rejected consumer source directory is created");
    fs::create_dir_all(&consumer_source).expect("consumer source directory is created");
    fs::create_dir_all(&generated_source).expect("generated package directory is created");
    write_package(&fixture.package, &generated_source);

    let runtime_source = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut configure_runtime = Command::new("cmake");
    configure_runtime
        .args(["-S"])
        .arg(runtime_source)
        .args(["-B"])
        .arg(&runtime_build)
        .arg(format!("-DTYPE_BRIDGE_C_LIBRARY={}", library.display()));
    #[cfg(windows)]
    configure_runtime.arg(format!(
        "-DTYPE_BRIDGE_C_IMPORT_LIBRARY={}",
        native_import_library(&library).display()
    ));
    let output = configure_runtime
        .output()
        .expect("runtime package CMake configure launches");
    assert!(
        output.status.success(),
        "runtime package CMake configure failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let mut install_runtime = Command::new("cmake");
    install_runtime
        .args(["--install"])
        .arg(&runtime_build)
        .args(["--prefix"])
        .arg(&install);
    #[cfg(windows)]
    install_runtime.args(["--config", "Debug"]);
    let output = install_runtime
        .output()
        .expect("runtime package CMake install launches");
    assert!(
        output.status.success(),
        "runtime package CMake install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let runtime_package_directory = install.join("lib/cmake/TypeBridge");
    let installed_config = runtime_package_directory.join("TypeBridgeConfig.cmake");
    let installed_pkg_config = install.join("lib/pkgconfig/type-bridge.pc");
    assert!(installed_config.is_file());
    assert!(installed_pkg_config.is_file());
    assert!(install.join("include/typebridge/type_bridge.h").is_file());
    for metadata in [&installed_config, &installed_pkg_config] {
        let contents = fs::read_to_string(metadata).expect("installed metadata is UTF-8");
        assert!(
            !contents.contains(runtime_source.to_string_lossy().as_ref())
                && !contents.contains(library.to_string_lossy().as_ref()),
            "installed package metadata retained a source/build path: {}",
            metadata.display(),
        );
    }

    fs::write(
        rejected_consumer_source.join("CMakeLists.txt"),
        r#"cmake_minimum_required(VERSION 3.20)
project(type_bridge_old_abi_consumer LANGUAGES C)
find_package(TypeBridge 1.2.0 EXACT CONFIG REQUIRED)
"#,
    )
    .expect("old-ABI rejection consumer is written");
    let output = Command::new("cmake")
        .args(["-S"])
        .arg(&rejected_consumer_source)
        .args(["-B"])
        .arg(&rejected_consumer_build)
        .arg(format!(
            "-DTypeBridge_DIR={}",
            runtime_package_directory.display()
        ))
        .output()
        .expect("old-ABI rejection configure launches");
    assert!(
        !output.status.success(),
        "the installed ABI 1.4 package incorrectly satisfied an exact ABI 1.2 request"
    );

    let output = Command::new("cmake")
        .args(["-S"])
        .arg(&generated_source)
        .args(["-B"])
        .arg(&generated_build)
        .arg(format!(
            "-DTypeBridge_DIR={}",
            runtime_package_directory.display()
        ))
        .arg(format!("-DCMAKE_INSTALL_PREFIX={}", install.display()))
        .output()
        .expect("generated schema package CMake configure launches");
    assert!(
        output.status.success(),
        "generated schema package CMake configure failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_cmake_cache_path(
        &generated_build,
        "TypeBridge_DIR",
        &runtime_package_directory,
        &install,
    );
    let mut build_generated = Command::new("cmake");
    build_generated.args(["--build"]).arg(&generated_build);
    #[cfg(windows)]
    build_generated.args(["--config", "Debug"]);
    let output = build_generated
        .output()
        .expect("generated schema package CMake build launches");
    assert!(
        output.status.success(),
        "generated schema package CMake build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let mut install_generated = Command::new("cmake");
    install_generated
        .args(["--install"])
        .arg(&generated_build)
        .args(["--prefix"])
        .arg(&install);
    #[cfg(windows)]
    install_generated.args(["--config", "Debug"]);
    let output = install_generated
        .output()
        .expect("generated schema package CMake install launches");
    assert!(
        output.status.success(),
        "generated schema package CMake install failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let generated_package_directory = install.join("lib/cmake/fixture");
    let installed_generated_config = generated_package_directory.join("fixtureConfig.cmake");
    let installed_generated_pkg_config = install.join("lib/pkgconfig/fixture.pc");
    assert!(
        installed_generated_config.is_file(),
        "generated schema package did not install its CMake config"
    );
    assert!(
        installed_generated_pkg_config.is_file(),
        "generated schema package did not install its pkg-config metadata"
    );
    for metadata in [&installed_generated_config, &installed_generated_pkg_config] {
        let contents = fs::read_to_string(metadata).expect("generated metadata is UTF-8");
        assert!(
            !contents.contains(generated_source.to_string_lossy().as_ref())
                && !contents.contains(generated_build.to_string_lossy().as_ref())
                && !contents.contains(install.to_string_lossy().as_ref()),
            "generated installed metadata retained a source/build/install path: {}",
            metadata.display(),
        );
    }

    fs::write(
        consumer_source.join("CMakeLists.txt"),
        r#"cmake_minimum_required(VERSION 3.20)
project(type_bridge_clean_consumer LANGUAGES C CXX)

find_package(TypeBridge 1.4.0 EXACT CONFIG REQUIRED)
find_package(fixture 1.0.0 EXACT CONFIG REQUIRED)

add_executable(type_bridge_clean_consumer main.c)
add_library(type_bridge_header_compat OBJECT header_compat.cpp)
set_target_properties(
  type_bridge_clean_consumer
  PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES C_EXTENSIONS NO
)
set_target_properties(
  type_bridge_header_compat
  PROPERTIES CXX_STANDARD 17 CXX_STANDARD_REQUIRED YES CXX_EXTENSIONS NO
)
target_link_libraries(type_bridge_clean_consumer PRIVATE fixture::schema)
target_link_libraries(type_bridge_header_compat PRIVATE fixture::schema)
target_compile_definitions(
  type_bridge_clean_consumer
  PRIVATE
    TYPE_BRIDGE_EXPECTED_RUNTIME_VERSION=\"${TYPE_BRIDGE_EXPECTED_RUNTIME_VERSION}\"
)
if(MSVC)
  target_compile_options(type_bridge_clean_consumer PRIVATE /W4 /WX)
  target_compile_options(type_bridge_header_compat PRIVATE /W4 /WX)
else()
  target_compile_options(
    type_bridge_clean_consumer PRIVATE -Wall -Wextra -Werror -pedantic-errors
  )
  target_compile_options(
    type_bridge_header_compat PRIVATE -Wall -Wextra -Werror -pedantic-errors
  )
endif()
"#,
    )
    .expect("clean consumer CMake project is written");
    fs::write(
        consumer_source.join("header_compat.cpp"),
        r#"#include <type_traits>

#include <fixture/models.h>

static_assert(std::is_standard_layout_v<type_bridge_byte_view_t>);
static_assert(std::is_standard_layout_v<type_bridge_schema_package_descriptor_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_projected_token_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_generated_opaque_input_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_generated_create_handle_chunk_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_generated_create_member_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_generated_create_args_graph_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_generated_output_range_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_projected_field_input_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_projected_role_input_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_projected_reference_descriptor_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_projected_create_descriptor_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_projected_thing_descriptor_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_execution_diagnostic_view_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_execution_diagnostic_path_view_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_execution_diagnostic_detail_view_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_runtime_config_v1_t>);
static_assert(std::is_standard_layout_v<type_bridge_database_config_v1_t>);

void type_bridge_header_compatibility_probe() {
  auto abi_major = &type_bridge_c_abi_major;
  auto open = &fixture_schema_package_open;
  auto runtime_open = &type_bridge_runtime_open_v1;
  auto database_open = &type_bridge_database_open_v1;
  auto read_open = &type_bridge_read_transaction_open;
  auto write_open = &type_bridge_write_transaction_open;
  auto generated_preflight = &type_bridge_generated_opaque_alias_preflight_v1;
  (void)abi_major;
  (void)open;
  (void)runtime_open;
  (void)database_open;
  (void)read_open;
  (void)write_open;
  (void)generated_preflight;
}
"#,
    )
    .expect("clean C++ header-compatibility source is written");
    fs::write(
        consumer_source.join("main.c"),
        r#"#include <stddef.h>
#include <string.h>

#include <fixture/models.h>

int main(void) {
  type_bridge_byte_view_t view = {0};
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  type_bridge_execution_diagnostics_t *execution_diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_cancellation_t *cancellation = NULL;
  type_bridge_runtime_config_v1_t runtime_config = {
      sizeof(type_bridge_runtime_config_v1_t),
      TYPE_BRIDGE_RUNTIME_CONFIG_VERSION,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN,
      0u,
      {0u, 0u, 0u, 0u}};

  if (type_bridge_c_abi_major() != TYPE_BRIDGE_C_ABI_MAJOR ||
      type_bridge_c_abi_minor() != TYPE_BRIDGE_C_ABI_MINOR ||
      type_bridge_runtime_version(&view) != TYPE_BRIDGE_STATUS_OK ||
      view.data == NULL ||
      view.length != sizeof(TYPE_BRIDGE_EXPECTED_RUNTIME_VERSION) - 1u ||
      memcmp(view.data, TYPE_BRIDGE_EXPECTED_RUNTIME_VERSION, view.length) != 0) {
    return 1;
  }
  if (type_bridge_runtime_open_v1(
          &runtime_config, &runtime, &execution_diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      runtime == NULL || execution_diagnostics != NULL) {
    return 9;
  }
  if (type_bridge_cancellation_open(&cancellation) != TYPE_BRIDGE_STATUS_OK ||
      cancellation == NULL ||
      type_bridge_cancellation_request(cancellation) != TYPE_BRIDGE_STATUS_OK) {
    return 10;
  }
  {
    uint8_t requested = 0u;
    if (type_bridge_cancellation_is_requested(cancellation, &requested) !=
            TYPE_BRIDGE_STATUS_OK ||
        requested != 1u) {
      return 11;
    }
  }
  if (type_bridge_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK ||
      cancellation != NULL ||
      type_bridge_runtime_close(&runtime, &execution_diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      runtime != NULL || execution_diagnostics != NULL) {
    return 12;
  }
  if (fixture_schema_package_open(&package, &diagnostics) !=
      TYPE_BRIDGE_STATUS_OK) {
    return 2;
  }
  if (package == NULL || diagnostics != NULL) {
    return 3;
  }
  if (type_bridge_schema_package_authority_json(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_schema_package_projection_json(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_schema_package_semantic_fingerprint_json(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_schema_package_binding_fingerprint_json(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_schema_package_managed_scope(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_schema_package_semantic_profile(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      view.data == NULL || view.length == 0u) {
    return 4;
  }
  if (type_bridge_schema_package_close(&package) != TYPE_BRIDGE_STATUS_OK ||
      package != NULL) {
    return 5;
  }

  return 0;
}
"#,
    )
    .expect("clean consumer source is written");

    let output = Command::new("cmake")
        .args(["-S"])
        .arg(&consumer_source)
        .args(["-B"])
        .arg(&consumer_build)
        .arg(format!(
            "-DTypeBridge_DIR={}",
            runtime_package_directory.display()
        ))
        .arg(format!(
            "-Dfixture_DIR={}",
            generated_package_directory.display()
        ))
        .arg(format!(
            "-DTYPE_BRIDGE_EXPECTED_RUNTIME_VERSION={}",
            env!("CARGO_PKG_VERSION")
        ))
        .output()
        .expect("clean consumer CMake configure launches");
    assert!(
        output.status.success(),
        "clean consumer CMake configure failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_cmake_cache_path(
        &consumer_build,
        "TypeBridge_DIR",
        &runtime_package_directory,
        &install,
    );
    assert_cmake_cache_path(
        &consumer_build,
        "fixture_DIR",
        &generated_package_directory,
        &install,
    );
    let mut build_consumer = Command::new("cmake");
    build_consumer.args(["--build"]).arg(&consumer_build);
    #[cfg(windows)]
    build_consumer.args(["--config", "Debug"]);
    let output = build_consumer
        .output()
        .expect("clean consumer CMake build launches");
    assert!(
        output.status.success(),
        "clean consumer CMake build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let executable = [
        consumer_build.join(if cfg!(windows) {
            "type_bridge_clean_consumer.exe"
        } else {
            "type_bridge_clean_consumer"
        }),
        consumer_build.join("Debug/type_bridge_clean_consumer.exe"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .expect("CMake clean-consumer executable exists");

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("readelf")
            .args(["-d"])
            .arg(&executable)
            .output()
            .expect("readelf is required for the staged CMake consumer audit");
        assert!(
            output.status.success(),
            "readelf failed for {}: {}",
            executable.display(),
            String::from_utf8_lossy(&output.stderr),
        );
        let report = String::from_utf8(output.stdout).expect("readelf output is UTF-8");
        let runtime_filename = library
            .file_name()
            .and_then(|name| name.to_str())
            .expect("native library filename is UTF-8");
        let runtime_dependencies = report
            .lines()
            .filter(|line| line.contains("(NEEDED)"))
            .filter_map(|line| {
                line.split_once('[')
                    .and_then(|(_, tail)| tail.split_once(']'))
                    .map(|(name, _)| name)
                    .filter(|name| name.contains("type_bridge_c"))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            runtime_dependencies,
            vec![runtime_filename],
            "the staged CMake consumer must record the portable TypeBridge C library basename, not an absolute package path:\n{report}",
        );
    }

    #[cfg(windows)]
    fs::copy(
        install
            .join("bin")
            .join(library.file_name().expect("native library has a filename")),
        executable
            .parent()
            .expect("consumer executable has a parent")
            .join(library.file_name().expect("native library has a filename")),
    )
    .expect("runtime DLL is staged beside the clean consumer");

    let output = Command::new(&executable)
        .output()
        .expect("clean staged CMake consumer launches");
    assert!(
        output.status.success(),
        "clean staged CMake consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn runtime_cmake_package_rejects_unconfined_install_directories() {
    if !shared_consumer_required() {
        return;
    }

    assert!(
        command_exists("cmake"),
        "cmake is required for the runtime package confinement audit"
    );
    let stage = TempDirectory::new();
    let runtime_source = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (sequence, setting, expected) in [
        (
            0,
            "-DCMAKE_INSTALL_LIBDIR=../escape",
            "CMAKE_INSTALL_LIBDIR must be a non-empty relative path",
        ),
        (
            1,
            "-DCMAKE_INSTALL_INCLUDEDIR=/absolute/include",
            "CMAKE_INSTALL_INCLUDEDIR must be a non-empty relative path",
        ),
        (
            2,
            "-DCMAKE_INSTALL_BINDIR=bin/../../escape",
            "CMAKE_INSTALL_BINDIR must be a non-empty relative path",
        ),
    ] {
        let output = Command::new("cmake")
            .args(["-S"])
            .arg(runtime_source)
            .args(["-B"])
            .arg(stage.path().join(format!("hostile-{sequence}")))
            .arg(setting)
            .output()
            .expect("hostile runtime CMake configure launches");
        assert!(!output.status.success(), "CMake accepted {setting}");
        let report = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            report.contains(expected),
            "CMake rejected {setting} for the wrong reason:\n{report}"
        );
    }
}

#[cfg(unix)]
#[test]
fn clean_staged_pkg_config_consumer_supports_nested_library_directories() {
    let Some(library) = native_library_or_skip(
        "clean_staged_pkg_config_consumer_supports_nested_library_directories",
    ) else {
        return;
    };
    for tool in ["cmake", "pkg-config"] {
        assert!(
            command_exists(tool),
            "{tool} is required for the pkg-config audit"
        );
    }
    let compiler = ["gcc", "clang"]
        .into_iter()
        .find(|compiler| command_exists(compiler))
        .expect("GCC or Clang is required for the pkg-config consumer");
    let fixture = emitted_fixture();
    let stage = TempDirectory::new();
    let runtime_build = stage.path().join("runtime-build");
    let generated_source = stage.path().join("generated-source");
    let generated_build = stage.path().join("generated-build");
    let install = stage.path().join("relocated-prefix");
    let nested_libdir = "lib/typebridge-test-arch";
    write_package(&fixture.package, &generated_source);

    let output = Command::new("cmake")
        .args(["-S"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["-B"])
        .arg(&runtime_build)
        .arg(format!("-DTYPE_BRIDGE_C_LIBRARY={}", library.display()))
        .arg(format!("-DCMAKE_INSTALL_LIBDIR={nested_libdir}"))
        .output()
        .expect("nested runtime CMake configure launches");
    assert!(
        output.status.success(),
        "nested runtime CMake configure failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new("cmake")
        .args(["--install"])
        .arg(&runtime_build)
        .args(["--prefix"])
        .arg(&install)
        .output()
        .expect("nested runtime CMake install launches");
    assert!(output.status.success(), "nested runtime install failed");

    let runtime_cmake_dir = install.join(nested_libdir).join("cmake/TypeBridge");
    let output = Command::new("cmake")
        .args(["-S"])
        .arg(&generated_source)
        .args(["-B"])
        .arg(&generated_build)
        .arg(format!("-DTypeBridge_DIR={}", runtime_cmake_dir.display()))
        .arg(format!("-DCMAKE_INSTALL_LIBDIR={nested_libdir}"))
        .output()
        .expect("nested generated CMake configure launches");
    assert!(
        output.status.success(),
        "nested generated CMake configure failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new("cmake")
        .args(["--build"])
        .arg(&generated_build)
        .output()
        .expect("nested generated CMake build launches");
    assert!(output.status.success(), "nested generated build failed");
    let output = Command::new("cmake")
        .args(["--install"])
        .arg(&generated_build)
        .args(["--prefix"])
        .arg(&install)
        .output()
        .expect("nested generated CMake install launches");
    assert!(output.status.success(), "nested generated install failed");

    let pkg_config_directory = install.join(nested_libdir).join("pkgconfig");
    let runtime_pc = pkg_config_directory.join("type-bridge.pc");
    let generated_pc = pkg_config_directory.join("fixture.pc");
    for metadata in [&runtime_pc, &generated_pc] {
        let contents = fs::read_to_string(metadata).expect("pkg-config metadata is UTF-8");
        assert!(
            !contents.contains(stage.path().to_string_lossy().as_ref())
                && contents.contains("prefix=${pcfiledir}/../../.."),
            "nested pkg-config metadata is not relocatable: {}\n{contents}",
            metadata.display(),
        );
    }

    for (package, expected_version) in [("type-bridge", "1.4.0"), ("fixture", "1.0.0")] {
        let output = Command::new("pkg-config")
            .args(["--modversion", package])
            .env("PKG_CONFIG_LIBDIR", &pkg_config_directory)
            .env_remove("PKG_CONFIG_PATH")
            .output()
            .expect("pkg-config version lookup launches");
        assert!(
            output.status.success(),
            "pkg-config could not resolve {package}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            expected_version
        );
    }
    let output = Command::new("pkg-config")
        .args(["--cflags", "--libs", "fixture"])
        .env("PKG_CONFIG_LIBDIR", &pkg_config_directory)
        .env_remove("PKG_CONFIG_PATH")
        .output()
        .expect("pkg-config flags lookup launches");
    assert!(
        output.status.success(),
        "pkg-config flags failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let flags = String::from_utf8(output.stdout).expect("pkg-config flags are UTF-8");
    assert!(
        flags.contains(install.to_string_lossy().as_ref()),
        "pkg-config did not resolve against the relocated prefix: {flags}"
    );

    let source = stage.path().join("pkg-config-consumer.c");
    fs::write(
        &source,
        r#"#include <stddef.h>
#include <string.h>

#include <fixture/models.h>

int main(void) {
  type_bridge_byte_view_t version = {0};
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  if (type_bridge_runtime_version(&version) != TYPE_BRIDGE_STATUS_OK ||
      version.length != sizeof(TYPE_BRIDGE_EXPECTED_RUNTIME_VERSION) - 1u ||
      memcmp(version.data, TYPE_BRIDGE_EXPECTED_RUNTIME_VERSION,
             version.length) != 0) {
    return 1;
  }
  if (fixture_schema_package_open(&package, &diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      package == NULL || diagnostics != NULL) {
    return 2;
  }
  return type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK &&
                 package == NULL
             ? 0
             : 3;
}
"#,
    )
    .expect("pkg-config consumer source is written");
    let executable = stage.path().join("pkg-config-consumer");
    let mut compile = Command::new(compiler);
    compile
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg(format!(
            "-DTYPE_BRIDGE_EXPECTED_RUNTIME_VERSION=\"{}\"",
            env!("CARGO_PKG_VERSION")
        ))
        .arg(&source);
    compile
        .args(flags.split_whitespace())
        .arg("-o")
        .arg(&executable);
    let output = compile
        .output()
        .expect("pkg-config consumer compile launches");
    assert!(
        output.status.success(),
        "pkg-config consumer compile failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let mut run = Command::new(&executable);
    #[cfg(target_os = "linux")]
    run.env("LD_LIBRARY_PATH", install.join(nested_libdir));
    #[cfg(target_os = "macos")]
    run.env("DYLD_LIBRARY_PATH", install.join(nested_libdir));
    let output = run.output().expect("pkg-config consumer launches");
    assert!(
        output.status.success(),
        "pkg-config consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn generated_descriptor_opens_and_borrows_exact_emitted_bytes() {
    let bytes = emitted_fixture().bytes;
    let descriptor = bytes.descriptor();
    let mut package = ptr::null_mut();
    let mut diagnostics = NonNull::<TypeBridgeDiagnostics>::dangling().as_ptr();

    // SAFETY: the descriptor views and both writable output slots live for the call.
    assert_eq!(
        unsafe { type_bridge_schema_package_open_v1(&descriptor, &mut package, &mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
    assert!(!package.is_null());
    assert!(diagnostics.is_null());
    // SAFETY: `package` remains live throughout these borrowed-view calls.
    unsafe {
        assert_eq!(
            copied_view(package, type_bridge_schema_package_authority_json),
            bytes.authority
        );
        assert_eq!(
            copied_view(package, type_bridge_schema_package_projection_json),
            bytes.projection
        );
        assert_eq!(
            copied_view(
                package,
                type_bridge_schema_package_semantic_fingerprint_json
            ),
            bytes.semantic
        );
        assert_eq!(
            copied_view(package, type_bridge_schema_package_binding_fingerprint_json),
            bytes.binding
        );
        assert_eq!(
            copied_view(package, type_bridge_schema_package_managed_scope),
            bytes.scope
        );
        assert_eq!(
            copied_view(package, type_bridge_schema_package_semantic_profile),
            bytes.profile
        );
    }

    // SAFETY: ownership of the live package returns through its exact close family.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(&mut package) },
        TypeBridgeStatus::Ok,
    );
    assert!(package.is_null());
    // SAFETY: repeated close of the now-null slot is explicitly idempotent.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(&mut package) },
        TypeBridgeStatus::Ok,
    );
}

#[cfg(unix)]
#[test]
fn standalone_c17_consumer_links_and_runs_the_generated_schema_package() {
    let Some(native_library) = native_library_or_skip(
        "standalone_c17_consumer_links_and_runs_the_generated_schema_package",
    ) else {
        return;
    };
    let fixture = emitted_fixture();
    let stage = TempDirectory::new();
    write_package(&fixture.package, stage.path());
    let consumer = stage.path().join("consumer.c");
    fs::write(
        &consumer,
        r#"#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <fixture/models.h>

#define CHECK(condition) do { if (!(condition)) { return __LINE__; } } while (0)
#define VIEW(literal)                                                         \
  ((type_bridge_byte_view_t){                                                \
      (const uint8_t *)(literal), sizeof(literal) - 1u})

static int same_view(type_bridge_byte_view_t actual,
                     type_bridge_byte_view_t expected) {
  return actual.length == expected.length &&
         memcmp(actual.data, expected.data, actual.length) == 0;
}

static int execution_code_is(
    const type_bridge_execution_diagnostics_t *diagnostics,
    const char *expected) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  const size_t expected_length = strlen(expected);
  return type_bridge_execution_diagnostics_get_v1(
             diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK &&
         diagnostic.code.length == expected_length &&
         memcmp(diagnostic.code.data, expected, expected_length) == 0;
}

int main(void) {
  type_bridge_byte_view_t actual = {0};
  type_bridge_schema_package_t *package =
      (type_bridge_schema_package_t *)(uintptr_t)1u;
  type_bridge_diagnostics_t *diagnostics =
      (type_bridge_diagnostics_t *)(uintptr_t)1u;

  CHECK(type_bridge_c_abi_major() == TYPE_BRIDGE_C_ABI_MAJOR);
  CHECK(type_bridge_c_abi_minor() == TYPE_BRIDGE_C_ABI_MINOR);
  CHECK(type_bridge_runtime_version(&actual) == TYPE_BRIDGE_STATUS_OK);
  CHECK(actual.data != NULL && actual.length != 0u);

  CHECK(fixture_schema_package_open(&package, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && diagnostics == NULL);

  {
    type_bridge_execution_diagnostics_t *execution_diagnostics = NULL;
    fixture_identifier *identifier = NULL;
    fixture_secondaryzhidentifier *secondary_identifier = NULL;
    fixture_measurezhlong *measure_long = NULL;
    fixture_measurezhdouble *measure_double = NULL;
    fixture_enabled *enabled = NULL;
    fixture_bornzhon *born_on = NULL;
    fixture_observedzhat *observed_at = NULL;
    fixture_zzonedzhat *zoned_at = NULL;
    fixture_balance *balance = NULL;
    fixture_elapsed *elapsed = NULL;
    type_bridge_projected_create_t *person_create = NULL;
    fixture_person_create *generated_create = NULL;
    type_bridge_projected_thing_t *person_thing = NULL;
    type_bridge_projected_reference_t *person_reference = NULL;
    type_bridge_projected_create_t *membership_create = NULL;
    type_bridge_projected_thing_t *membership_thing = NULL;
    type_bridge_projected_reference_t *extracted_reference = NULL;
    type_bridge_projected_reference_t *wrong_reference = NULL;
    type_bridge_projected_value_t *generic_value = NULL;
    type_bridge_projected_token_v1_t foreign_token;
    int64_t long_value = 0;
    uint64_t double_bits = 0u;
    uint8_t boolean_value = 0u;
    size_t count = 0u;

    CHECK(fixture_enabled_open(package, 2u, &enabled,
                               &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(enabled == NULL && execution_diagnostics != NULL);
    CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);

    foreign_token = fixture_projected_model_token_6;
    foreign_token.projection_digest[0] ^= 1u;
    CHECK(type_bridge_projected_value_string_open(
              package, &foreign_token, VIEW("foreign"), &generic_value,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
    CHECK(generic_value == NULL && execution_diagnostics != NULL);
    CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_identifier_open(package, VIEW("ada"), &identifier,
                                  &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_secondaryzhidentifier_open(
              package, VIEW("other"), &secondary_identifier,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_measurezhlong_open(package, INT64_MIN, &measure_long,
                                     &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_measurezhdouble_open(
              package, UINT64_C(0x8000000000000000), &measure_double,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_enabled_open(package, 1u, &enabled,
                               &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_bornzhon_open(package, VIEW("2026-08-11"), &born_on,
                                &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_observedzhat_open(
              package, VIEW("2026-08-11T07:30:00.5"), &observed_at,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_zzonedzhat_open(
              package, VIEW("2026-08-11T07:30:00Z"), &zoned_at,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_balance_open(package, VIEW("12.5"), &balance,
                               &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_elapsed_open(package, VIEW("P1M2DT3.000000004S"), &elapsed,
                               &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(execution_diagnostics == NULL);

    CHECK(fixture_identifier_value(identifier, &actual,
                                   &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_view(actual, VIEW("ada")));
    CHECK(fixture_measurezhlong_value(measure_long, &long_value,
                                     &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(long_value == INT64_MIN);
    CHECK(fixture_measurezhdouble_value(measure_double, &double_bits,
                                       &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(double_bits == UINT64_C(0x8000000000000000));
    CHECK(fixture_enabled_value(enabled, &boolean_value,
                                &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(boolean_value == 1u);
    CHECK(fixture_bornzhon_value(born_on, &actual,
                                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_observedzhat_value(observed_at, &actual,
                                    &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_zzonedzhat_value(zoned_at, &actual,
                                  &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_balance_value(balance, &actual,
                                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_elapsed_value(elapsed, &actual,
                                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(same_view(actual, VIEW("P1M2DT3.000000004S")));

    actual.data = (const uint8_t *)(uintptr_t)1u;
    actual.length = 1u;
    CHECK(fixture_identifier_value(
              (const fixture_identifier *)secondary_identifier,
              &actual, &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(actual.data == NULL && actual.length == 0u);
    CHECK(execution_diagnostics != NULL);
    CHECK(execution_code_is(execution_diagnostics,
                            "c_projected_value_model_mismatch"));
    CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);

    execution_diagnostics =
        (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
    CHECK(fixture_identifier_value(
              identifier,
              (type_bridge_byte_view_t *)((uint8_t *)identifier + 1u),
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(execution_diagnostics ==
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
    execution_diagnostics = NULL;

    execution_diagnostics =
        (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
    CHECK(fixture_identifier_value(
              identifier,
              (type_bridge_byte_view_t *)(
                  (uint8_t *)&fixture_projected_model_token_7 + 1u),
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(execution_diagnostics ==
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
    execution_diagnostics = NULL;

    execution_diagnostics =
        (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
    CHECK(fixture_identifier_value(identifier, NULL,
                                   &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(execution_diagnostics == NULL);
    CHECK(fixture_identifier_value(identifier, &actual,
                                   &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_view(actual, VIEW("ada")));

    {
      const type_bridge_projected_value_t *identifier_values[] = {
          (const type_bridge_projected_value_t *)identifier};
      const type_bridge_projected_value_t *long_values[] = {
          (const type_bridge_projected_value_t *)measure_long};
      const type_bridge_projected_value_t *double_values[] = {
          (const type_bridge_projected_value_t *)measure_double};
      const type_bridge_projected_value_t *enabled_values[] = {
          (const type_bridge_projected_value_t *)enabled};
      const type_bridge_projected_value_t *date_values[] = {
          (const type_bridge_projected_value_t *)born_on};
      const type_bridge_projected_value_t *datetime_values[] = {
          (const type_bridge_projected_value_t *)observed_at};
      const type_bridge_projected_value_t *datetime_tz_values[] = {
          (const type_bridge_projected_value_t *)zoned_at};
      const type_bridge_projected_value_t *decimal_values[] = {
          (const type_bridge_projected_value_t *)balance};
      const type_bridge_projected_value_t *duration_values[] = {
          (const type_bridge_projected_value_t *)elapsed};
      const type_bridge_projected_field_input_v1_t person_fields[] = {
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_5, identifier_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_7, long_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_6, double_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_4, enabled_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_2, date_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_8, datetime_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_9, datetime_tz_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_1, decimal_values, 1u,
           {0u, 0u, 0u, 0u}},
          {sizeof(type_bridge_projected_field_input_v1_t), 1u,
           &fixture_projected_field_token_3, duration_values, 1u,
           {0u, 0u, 0u, 0u}}};
      const type_bridge_projected_create_descriptor_v1_t person_descriptor = {
          sizeof(type_bridge_projected_create_descriptor_v1_t), 1u,
          &fixture_projected_model_token_0, person_fields,
          sizeof(person_fields) / sizeof(person_fields[0]), NULL, 0u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_thing_descriptor_v1_t person_thing_descriptor = {
          sizeof(type_bridge_projected_thing_descriptor_v1_t), 1u,
          &fixture_projected_model_token_0, VIEW("0xaaa"), person_fields,
          sizeof(person_fields) / sizeof(person_fields[0]), NULL, 0u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_field_input_v1_t key = {
          sizeof(type_bridge_projected_field_input_v1_t), 1u,
          &fixture_projected_field_token_5, identifier_values, 1u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_reference_descriptor_v1_t reference_descriptor = {
          sizeof(type_bridge_projected_reference_descriptor_v1_t), 1u,
          &fixture_projected_model_token_0, VIEW("0xabc"), &key, 1u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_field_input_v1_t invalid_key = {
          sizeof(type_bridge_projected_field_input_v1_t), 1u,
          &fixture_projected_field_token_5, NULL, 0u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_reference_descriptor_v1_t invalid_reference = {
          sizeof(type_bridge_projected_reference_descriptor_v1_t), 1u,
          &fixture_projected_model_token_0, {NULL, 0u}, &invalid_key, 1u,
          {0u, 0u, 0u, 0u}};

      CHECK(type_bridge_projected_create_open_v1(
                package, &person_descriptor, &person_create,
                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(type_bridge_projected_thing_open_v1(
                package, &person_thing_descriptor, &person_thing,
                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(type_bridge_projected_reference_open_v1(
                package, &reference_descriptor, &person_reference,
                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(type_bridge_projected_reference_open_v1(
                package, &invalid_reference, &extracted_reference,
                &execution_diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(extracted_reference == NULL && execution_diagnostics != NULL);
      CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
    }

    {
      const fixture_aliases *const *unreadable_aliases =
          (const fixture_aliases *const *)(uintptr_t)1u;
      fixture_person_create_field_aliases_chunks_v1_t aliases_chunk = {0};
      fixture_person_create_args_v1_t create_args = {0};

      aliases_chunk.struct_size = sizeof(aliases_chunk);
      aliases_chunk.version = FIXTURE_CREATE_ARGS_VERSION;
      aliases_chunk.values = unreadable_aliases;
      aliases_chunk.count =
          FIXTURE_SEQUENCE_OBJECT_BYTES_MAX / sizeof(*unreadable_aliases) + 1u;
      create_args.struct_size = sizeof(create_args);
      create_args.version = FIXTURE_CREATE_ARGS_VERSION;
      create_args.field_aliases_chunks = &aliases_chunk;
      create_args.field_identifier = identifier;

      generated_create = (fixture_person_create *)(uintptr_t)1u;
      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_create_open(
                package, &create_args,
                &generated_create, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
      CHECK(generated_create ==
            (fixture_person_create *)(uintptr_t)1u);
      CHECK(execution_diagnostics ==
            (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);

      create_args.field_aliases_chunks = NULL;

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_create_open(
                package, &create_args,
                NULL, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics == NULL);

      CHECK(fixture_person_create_open(
                package, &create_args,
                &generated_create, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(generated_create != NULL && execution_diagnostics == NULL);

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_database_insert(
                NULL, generated_create, NULL,
                (fixture_person **)((uint8_t *)generated_create + 1u),
                &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics ==
            (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
      execution_diagnostics = NULL;

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_database_insert(
                NULL, generated_create, NULL, NULL,
                &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics == NULL);
      CHECK(fixture_person_create_close(&generated_create) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(generated_create == NULL);
    }

    CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_secondaryzhidentifier_close(&secondary_identifier) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_measurezhlong_close(&measure_long) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_measurezhdouble_close(&measure_double) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_enabled_close(&enabled) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_bornzhon_close(&born_on) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_observedzhat_close(&observed_at) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_zzonedzhat_close(&zoned_at) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_balance_close(&balance) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_elapsed_close(&elapsed) == TYPE_BRIDGE_STATUS_OK);

    {
      const type_bridge_projected_reference_t *role_references[] = {
          person_reference};
      const type_bridge_projected_role_input_v1_t role = {
          sizeof(type_bridge_projected_role_input_v1_t), 1u,
          &fixture_projected_role_token_0, role_references, 1u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_create_descriptor_v1_t membership_descriptor = {
          sizeof(type_bridge_projected_create_descriptor_v1_t), 1u,
          &fixture_projected_model_token_1, NULL, 0u, &role, 1u,
          {0u, 0u, 0u, 0u}};
      const type_bridge_projected_thing_descriptor_v1_t thing_descriptor = {
          sizeof(type_bridge_projected_thing_descriptor_v1_t), 1u,
          &fixture_projected_model_token_1, VIEW("0xdef"), NULL, 0u,
          &role, 1u, {0u, 0u, 0u, 0u}};
      CHECK(type_bridge_projected_create_open_v1(
                package, &membership_descriptor, &membership_create,
                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(type_bridge_projected_thing_open_v1(
                package, &thing_descriptor, &membership_thing,
                &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(execution_diagnostics == NULL);
    CHECK(type_bridge_projected_reference_close(&person_reference) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
    CHECK(package == NULL);

    {
      fixture_person_ref *nominal_reference =
          (fixture_person_ref *)(uintptr_t)1u;

      actual.data = (const uint8_t *)(uintptr_t)1u;
      actual.length = 1u;
      CHECK(fixture_person_iid((const fixture_person *)membership_thing,
                               &actual, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(actual.data == NULL && actual.length == 0u);
      CHECK(execution_diagnostics != NULL);
      CHECK(execution_code_is(execution_diagnostics,
                              "c_projected_thing_model_mismatch"));
      CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);

      CHECK(fixture_person_reference(
                (const fixture_person *)membership_thing,
                &nominal_reference, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(nominal_reference == NULL && execution_diagnostics != NULL);
      CHECK(execution_code_is(execution_diagnostics,
                              "c_projected_thing_model_mismatch"));
      CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);

      CHECK(type_bridge_projected_thing_reference(
                membership_thing, &wrong_reference, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_ref_iid(
                (const fixture_person_ref *)wrong_reference,
                (type_bridge_byte_view_t *)((uint8_t *)wrong_reference + 1u),
                &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics ==
            (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
      execution_diagnostics = NULL;

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_ref_iid(
                (const fixture_person_ref *)wrong_reference,
                NULL, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics == NULL);

      actual.data = (const uint8_t *)(uintptr_t)1u;
      actual.length = 1u;
      CHECK(fixture_person_ref_iid(
                (const fixture_person_ref *)wrong_reference,
                &actual, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(actual.data == NULL && actual.length == 0u);
      CHECK(execution_diagnostics != NULL);
      CHECK(execution_code_is(execution_diagnostics,
                              "c_projected_reference_model_mismatch"));
      CHECK(type_bridge_execution_diagnostics_close(&execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(type_bridge_projected_reference_iid(wrong_reference, &actual) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(same_view(actual, VIEW("0xdef")));
      CHECK(type_bridge_projected_reference_close(&wrong_reference) ==
            TYPE_BRIDGE_STATUS_OK);

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_iid(
                (const fixture_person *)person_thing,
                (type_bridge_byte_view_t *)((uint8_t *)person_thing + 1u),
                &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics ==
            (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
      execution_diagnostics = NULL;

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_iid(
                (const fixture_person *)person_thing,
                (type_bridge_byte_view_t *)(
                    (uint8_t *)&fixture_projected_model_token_0 + 1u),
                &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics ==
            (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
      execution_diagnostics = NULL;

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_iid((const fixture_person *)person_thing,
                               NULL, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics == NULL);

      execution_diagnostics =
          (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
      CHECK(fixture_person_reference(
                (const fixture_person *)person_thing,
                NULL, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
      CHECK(execution_diagnostics == NULL);

      CHECK(fixture_person_iid((const fixture_person *)person_thing,
                               &actual, &execution_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(same_view(actual, VIEW("0xaaa")));
    }

    CHECK(type_bridge_projected_thing_reference(
              person_thing, &extracted_reference, &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_reference_iid(extracted_reference, &actual) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_view(actual, VIEW("0xaaa")));
    CHECK(type_bridge_projected_reference_key(
              extracted_reference, &fixture_projected_field_token_5,
              &generic_value, &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_value_text(generic_value, &actual) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_view(actual, VIEW("ada")));
    CHECK(type_bridge_projected_value_close(&generic_value) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_reference_close(&extracted_reference) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(type_bridge_projected_create_field_count(
              person_create, &fixture_projected_field_token_5, &count,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 1u);
    CHECK(type_bridge_projected_create_role_count(
              membership_create, &fixture_projected_role_token_0, &count,
              &execution_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 1u);
    CHECK(type_bridge_projected_thing_role_reference_at(
              membership_thing, &fixture_projected_role_token_0, 0u,
              &extracted_reference, &execution_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_reference_iid(extracted_reference, &actual) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_view(actual, VIEW("0xabc")));
    CHECK(type_bridge_projected_reference_close(&extracted_reference) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_create_close(&person_create) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_thing_close(&person_thing) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_create_close(&membership_create) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_projected_thing_close(&membership_thing) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);

  package = (type_bridge_schema_package_t *)(uintptr_t)1u;
  CHECK(fixture_schema_package_open(&package, NULL) ==
        TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(package == NULL);
  diagnostics = (type_bridge_diagnostics_t *)(uintptr_t)1u;
  CHECK(fixture_schema_package_open(NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(diagnostics == NULL);
  return 0;
}
"#,
    )
    .expect("standalone C17 consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let native_directory = native_library
        .parent()
        .expect("native library has a parent directory");

    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage.path().join(format!("consumer-{compiler}"));
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(&generated_include)
            .arg(&generated_source)
            .arg(&consumer)
            .arg("-L")
            .arg(native_directory)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", native_directory.display()))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} failed to link the standalone C17 consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{compiler}'s standalone C17 consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(
        invocations > 0,
        "no GCC- or Clang-compatible C compiler was available"
    );
}

#[cfg(unix)]
#[test]
fn standalone_generated_relation_facade_preserves_nominal_role_boundaries() {
    let Some(native_library) = native_library_or_skip(
        "standalone_generated_relation_facade_preserves_nominal_role_boundaries",
    ) else {
        return;
    };
    let fixture = emitted_fixture();
    let relation = emitted_relation_fixture();
    let stage = TempDirectory::new();
    let fixture_stage = stage.path().join("fixture-package");
    let relation_stage = stage.path().join("relation-package");
    write_package(&fixture.package, &fixture_stage);
    write_package(&relation.package, &relation_stage);
    let consumer = stage.path().join("relation-consumer.c");
    let source = r#"#include <stdint.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <fixture/models.h>
#include <relation/models.h>

#define CHECK(condition) do { if (!(condition)) { return __LINE__; } } while (0)
#define VIEW(literal)                                                         \
  ((type_bridge_byte_view_t){                                                \
      (const uint8_t *)(literal), sizeof(literal) - 1u})

static int same_view(type_bridge_byte_view_t actual,
                     type_bridge_byte_view_t expected) {
  return actual.length == expected.length &&
         memcmp(actual.data, expected.data, actual.length) == 0;
}

static int execution_code_is(
    const type_bridge_execution_diagnostics_t *diagnostics,
    const char *expected) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  const size_t expected_length = strlen(expected);
  return type_bridge_execution_diagnostics_get_v1(
             diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK &&
         diagnostic.code.length == expected_length &&
         memcmp(diagnostic.code.data, expected, expected_length) == 0;
}

int main(void) {
  type_bridge_schema_package_t *package = NULL;
  type_bridge_schema_package_t *foreign_package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  relation_person_ref *person = NULL;
  relation_person_ref *person_copy = NULL;
  relation_robot_ref *robot = NULL;
  relation_robot_ref *robot_copy = NULL;
  relation_employee_ref *employee = NULL;
  relation_event_ref *event = NULL;
  relation_event_ref *event_copy = NULL;
  fixture_person_ref *foreign_person = NULL;
  relation_membership_member_player *member = NULL;
  relation_membership_member_player *hydrated_member = NULL;
  relation_container_item_player *event_player = NULL;
  relation_container_item_player *hydrated_event = NULL;
  relation_membership_create *membership_create = NULL;
  relation_container_create *container_create = NULL;
  type_bridge_projected_thing_t *membership_thing = NULL;
  type_bridge_projected_thing_t *container_thing = NULL;
  type_bridge_byte_view_t iid = {NULL, 0u};
  relation_membership_member_player_kind_t member_kind =
      relation_membership_member_player_kind_unknown;
  relation_container_item_player_kind_t event_kind =
      relation_container_item_player_kind_unknown;
  size_t count = 0u;

  CHECK(relation_schema_package_open(&package, &package_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && package_diagnostics == NULL);
  CHECK(fixture_schema_package_open(&foreign_package,
                                    &package_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(foreign_package != NULL && package_diagnostics == NULL);

  CHECK(relation_person_ref_from_iid(package, VIEW("0xaaa"), &person,
                                     &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_robot_ref_from_iid(package, VIEW("0xbbb"), &robot,
                                    &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_employee_ref_from_iid(package, VIEW("0xccc"), &employee,
                                       &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_event_ref_from_iid(package, VIEW("0xddd"), &event,
                                    &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_from_iid(foreign_package, VIEW("0xeee"),
                                    &foreign_person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  diagnostics = (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
  CHECK(relation_membership_member_player_from_person(
            person,
            (relation_membership_member_player **)((uint8_t *)person + 1u),
            &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(diagnostics ==
        (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
  diagnostics = NULL;
  CHECK(relation_person_ref_iid(person, &iid, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_view(iid, VIEW("0xaaa")));

  diagnostics = (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
  CHECK(relation_membership_member_player_from_person(
            person, NULL, &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(diagnostics == NULL);
  member = (relation_membership_member_player *)(uintptr_t)1u;
  CHECK(relation_membership_member_player_from_person(
            person, &member, NULL) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(member == NULL);

  CHECK(relation_membership_member_player_from_person(
            person, &member, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(member != NULL && diagnostics == NULL);
  CHECK(relation_membership_member_player_kind(
            member, &member_kind, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(member_kind == relation_membership_member_player_kind_person);
  CHECK(relation_membership_member_player_as_person(
            member, &person_copy, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_person_ref_iid(person_copy, &iid, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_view(iid, VIEW("0xaaa")));
  CHECK(relation_person_ref_close(&person_copy) == TYPE_BRIDGE_STATUS_OK);

  {
    relation_membership_create_args_v1_t args = {0};
    args.struct_size = sizeof(args);
    args.version = RELATION_CREATE_ARGS_VERSION;
    args.role_member = member;
    CHECK(relation_membership_create_open(
              package, &args, &membership_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(membership_create != NULL && diagnostics == NULL);
  CHECK(relation_membership_create_close(&membership_create) ==
        TYPE_BRIDGE_STATUS_OK);

  {
    const type_bridge_projected_reference_t *references[] = {
        (const type_bridge_projected_reference_t *)member};
    const type_bridge_projected_role_input_v1_t role = {
        sizeof(type_bridge_projected_role_input_v1_t),
        TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION,
        &MEMBERSHIP_ROLE_TOKEN, references, 1u, {0u, 0u, 0u, 0u}};
    const type_bridge_projected_thing_descriptor_v1_t descriptor = {
        sizeof(type_bridge_projected_thing_descriptor_v1_t),
        TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION,
        &MEMBERSHIP_MODEL_TOKEN, VIEW("0xf00"), NULL, 0u, &role, 1u,
        {0u, 0u, 0u, 0u}};
    CHECK(type_bridge_projected_thing_open_v1(
              package, &descriptor, &membership_thing, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(relation_membership_member_player_close(&member) ==
        TYPE_BRIDGE_STATUS_OK);

  diagnostics = (type_bridge_execution_diagnostics_t *)(uintptr_t)1u;
  CHECK(relation_membership_member(
            (const relation_membership *)membership_thing,
            (relation_membership_member_player **)
                ((uint8_t *)membership_thing + 1u),
            &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(diagnostics ==
        (type_bridge_execution_diagnostics_t *)(uintptr_t)1u);
  diagnostics = NULL;
  CHECK(type_bridge_projected_thing_iid(membership_thing, &iid) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_view(iid, VIEW("0xf00")));
  CHECK(relation_membership_member(
            (const relation_membership *)membership_thing,
            &hydrated_member, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_membership_member_player_kind(
            hydrated_member, &member_kind, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(member_kind == relation_membership_member_player_kind_person);
  CHECK(relation_membership_member_player_close(&hydrated_member) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_projected_thing_close(&membership_thing) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(relation_membership_member_player_from_robot(
            robot, &member, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_membership_member_player_kind(
            member, &member_kind, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(member_kind == relation_membership_member_player_kind_robot);
  CHECK(relation_membership_member_player_as_robot(
            member, &robot_copy, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_robot_ref_iid(robot_copy, &iid, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_view(iid, VIEW("0xbbb")));
  CHECK(relation_robot_ref_close(&robot_copy) == TYPE_BRIDGE_STATUS_OK);
  {
    relation_membership_create_args_v1_t args = {0};
    args.struct_size = sizeof(args);
    args.version = RELATION_CREATE_ARGS_VERSION;
    args.role_member = member;
    CHECK(relation_membership_create_open(
              package, &args, &membership_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(relation_membership_create_close(&membership_create) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_membership_member_player_close(&member) ==
        TYPE_BRIDGE_STATUS_OK);

  member = (relation_membership_member_player *)(uintptr_t)1u;
  CHECK(relation_membership_member_player_from_person(
            (const relation_person_ref *)employee, &member, &diagnostics) ==
        TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(member == NULL && diagnostics != NULL);
  CHECK(execution_code_is(
      diagnostics, "c_projected_reference_role_player_mismatch"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  member = (relation_membership_member_player *)(uintptr_t)1u;
  CHECK(relation_membership_member_player_from_person(
            (const relation_person_ref *)foreign_person, &member,
            &diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  CHECK(member == NULL && diagnostics != NULL);
  CHECK(execution_code_is(diagnostics, "generated_token_package_mismatch"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  membership_create = (relation_membership_create *)(uintptr_t)1u;
  CHECK(relation_membership_create_open(
            package, NULL, &membership_create, &diagnostics) !=
        TYPE_BRIDGE_STATUS_OK);
  CHECK(membership_create == NULL && diagnostics == NULL);

  CHECK(relation_container_item_player_from_event(
            event, &event_player, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_container_item_player_kind(
            event_player, &event_kind, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(event_kind == relation_container_item_player_kind_event);
  {
    const relation_container_item_player *players[] = {
        event_player, event_player};
    const relation_container_item_player *too_many[] = {
        event_player, event_player, event_player, event_player};
    relation_container_create_role_item_chunks_v1_t chunk = {0};
    relation_container_create_args_v1_t args = {0};
    chunk.struct_size = sizeof(chunk);
    chunk.version = RELATION_CREATE_ARGS_VERSION;
    chunk.values = players;
    chunk.count = 2u;
    args.struct_size = sizeof(args);
    args.version = RELATION_CREATE_ARGS_VERSION;
    args.role_item_chunks = &chunk;
    CHECK(relation_container_create_open(
              package, &args, &container_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(container_create != NULL && diagnostics == NULL);
    CHECK(relation_container_create_close(&container_create) ==
          TYPE_BRIDGE_STATUS_OK);
    chunk.values = too_many;
    chunk.count = 4u;
    CHECK(relation_container_create_open(
              package, &args, &container_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(container_create == NULL && diagnostics != NULL);
    CHECK(execution_code_is(diagnostics, "role_cardinality_violation"));
    CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  {
    const type_bridge_projected_reference_t *references[] = {
        (const type_bridge_projected_reference_t *)event_player,
        (const type_bridge_projected_reference_t *)event_player};
    const type_bridge_projected_role_input_v1_t role = {
        sizeof(type_bridge_projected_role_input_v1_t),
        TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION,
        &CONTAINER_ROLE_TOKEN, references, 2u, {0u, 0u, 0u, 0u}};
    const type_bridge_projected_thing_descriptor_v1_t descriptor = {
        sizeof(type_bridge_projected_thing_descriptor_v1_t),
        TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION,
        &CONTAINER_MODEL_TOKEN, VIEW("0xf01"), NULL, 0u, &role, 1u,
        {0u, 0u, 0u, 0u}};
    CHECK(type_bridge_projected_thing_open_v1(
              package, &descriptor, &container_thing, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(relation_container_item_player_close(&event_player) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_container_item_count(
            (const relation_container *)container_thing, &count,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 2u);
  CHECK(relation_container_item_at(
            (const relation_container *)container_thing, 1u,
            &hydrated_event, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_container_item_player_kind(
            hydrated_event, &event_kind, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(event_kind == relation_container_item_player_kind_event);
  CHECK(relation_container_item_player_as_event(
            hydrated_event, &event_copy, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_event_ref_iid(event_copy, &iid, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_view(iid, VIEW("0xddd")));
  CHECK(relation_event_ref_close(&event_copy) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_container_item_player_close(&hydrated_event) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_projected_thing_close(&container_thing) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(relation_person_ref_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_robot_ref_close(&robot) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_employee_ref_close(&employee) == TYPE_BRIDGE_STATUS_OK);
  CHECK(relation_event_ref_close(&event) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&foreign_person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&foreign_package) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  CHECK(package == NULL && foreign_package == NULL && diagnostics == NULL);
  return 0;
}
"#
    .replace(
        "MEMBERSHIP_MODEL_TOKEN",
        &format!(
            "relation_projected_model_token_{}",
            relation.membership_model_ordinal
        ),
    )
    .replace(
        "MEMBERSHIP_ROLE_TOKEN",
        &format!(
            "relation_projected_role_token_{}",
            relation.membership_role_ordinal
        ),
    )
    .replace(
        "CONTAINER_MODEL_TOKEN",
        &format!(
            "relation_projected_model_token_{}",
            relation.container_model_ordinal
        ),
    )
    .replace(
        "CONTAINER_ROLE_TOKEN",
        &format!(
            "relation_projected_role_token_{}",
            relation.container_role_ordinal
        ),
    );
    fs::write(&consumer, source).expect("generated relation C consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let native_directory = native_library
        .parent()
        .expect("native library has a parent directory");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage.path().join(format!("relation-consumer-{compiler}"));
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(fixture_stage.join("include"))
            .arg("-I")
            .arg(relation_stage.join("include"))
            .arg(fixture_stage.join("src/models.c"))
            .arg(relation_stage.join("src/models.c"))
            .arg(&consumer)
            .arg("-L")
            .arg(native_directory)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", native_directory.display()))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} failed to link the generated relation C17 consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{compiler}'s generated relation C17 consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(
        invocations > 0,
        "no GCC- or Clang-compatible C compiler was available"
    );
}

#[cfg(unix)]
#[test]
fn standalone_ordered_generated_facade_enforces_construction_hydration_and_package_fences() {
    let Some(native_library) = native_library_or_skip(
        "standalone_ordered_generated_facade_enforces_construction_hydration_and_package_fences",
    ) else {
        return;
    };
    let fixture = emitted_ordered_phase2_fixture();
    let stage = TempDirectory::new();
    let package_stage = stage.path().join("ordered-package");
    let foreign_stage = stage.path().join("ordered-foreign-package");
    write_package(&fixture.package, &package_stage);
    write_package(&fixture.foreign_package, &foreign_stage);
    let consumer = stage.path().join("ordered-phase2-consumer.c");
    let source = r#"#include <stdint.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <orderedphase/models.h>
#include <orderedforeign/models.h>

#define CHECK(condition) do { if (!(condition)) { return __LINE__; } } while (0)
#define VIEW(literal)                                                         \
  ((type_bridge_byte_view_t){                                                \
      (const uint8_t *)(literal), sizeof(literal) - 1u})

static int same_view(type_bridge_byte_view_t actual,
                     type_bridge_byte_view_t expected) {
  return actual.length == expected.length &&
         memcmp(actual.data, expected.data, actual.length) == 0;
}

static int projected_value_is(const type_bridge_projected_value_t *value,
                              type_bridge_byte_view_t expected) {
  type_bridge_byte_view_t actual = {NULL, 0u};
  return type_bridge_projected_value_text(value, &actual) ==
             TYPE_BRIDGE_STATUS_OK &&
         same_view(actual, expected);
}

static int projected_reference_is(
    const type_bridge_projected_reference_t *reference,
    type_bridge_byte_view_t expected) {
  type_bridge_byte_view_t actual = {NULL, 0u};
  return type_bridge_projected_reference_iid(reference, &actual) ==
             TYPE_BRIDGE_STATUS_OK &&
         same_view(actual, expected);
}

static int diagnostic_is(
    const type_bridge_execution_diagnostics_t *diagnostics,
    type_bridge_execution_diagnostic_category_t category,
    const char *expected_code) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  size_t count = 0u;
  const size_t expected_length = strlen(expected_code);
  return diagnostics != NULL &&
         type_bridge_execution_diagnostics_count(diagnostics, &count) ==
             TYPE_BRIDGE_STATUS_OK &&
         count == 1u &&
         type_bridge_execution_diagnostics_get_v1(
             diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK &&
         diagnostic.category == category &&
         diagnostic.code.length == expected_length &&
         memcmp(diagnostic.code.data, expected_code, expected_length) == 0;
}

int main(void) {
  type_bridge_schema_package_t *package = NULL;
  type_bridge_schema_package_t *foreign_package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  orderedphase_aliases *first_alias = NULL;
  orderedphase_aliases *second_alias = NULL;
  orderedforeign_aliases *foreign_alias = NULL;
  orderedphase_person_ref *first_person = NULL;
  orderedphase_person_ref *second_person = NULL;
  orderedphase_person_ref *hydrated_person = NULL;
  orderedphase_plainzhactivity_participant_player *first_player = NULL;
  orderedphase_plainzhactivity_participant_player *second_player = NULL;
  orderedphase_plainzhactivity_participant_player *hydrated_player = NULL;
  orderedphase_person_create *person_create = NULL;
  orderedphase_person_create *duplicate_person_create = NULL;
  orderedphase_plainzhactivity_create *activity_create = NULL;
  orderedphase_plainzhactivity_create *duplicate_activity_create = NULL;
  type_bridge_projected_thing_t *person_thing = NULL;
  type_bridge_projected_thing_t *activity_thing = NULL;
  type_bridge_projected_thing_t *duplicate_thing = NULL;
  type_bridge_projected_thing_t *foreign_person_thing = NULL;
  type_bridge_projected_value_t *generic_value = NULL;
  type_bridge_projected_reference_t *generic_reference = NULL;
  orderedphase_aliases *hydrated_alias = NULL;
  size_t count = 0u;

  CHECK(orderedphase_schema_package_open(&package, &package_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && package_diagnostics == NULL);
  CHECK(orderedforeign_schema_package_open(
            &foreign_package, &package_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(foreign_package != NULL && package_diagnostics == NULL);

  CHECK(orderedphase_aliases_open(
            package, VIEW("first"), &first_alias, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_aliases_open(
            package, VIEW("second"), &second_alias, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_person_ref_from_iid(
            package, VIEW("0x01"), &first_person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_person_ref_from_iid(
            package, VIEW("0x02"), &second_person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_from_person(
            first_person, &first_player, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_from_person(
            second_person, &second_player, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostics == NULL);

  {
    const orderedphase_aliases *head_values[] = {second_alias};
    const orderedphase_aliases *tail_values[] = {first_alias};
    orderedphase_person_create_field_aliases_chunks_v1_t tail = {0};
    orderedphase_person_create_field_aliases_chunks_v1_t head = {0};
    orderedphase_person_create_args_v1_t args = {0};
    tail.struct_size = sizeof(tail);
    tail.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    tail.values = tail_values;
    tail.count = 1u;
    head.struct_size = sizeof(head);
    head.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    head.values = head_values;
    head.count = 1u;
    head.next = &tail;
    args.struct_size = sizeof(args);
    args.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    args.field_aliases_chunks = &head;
    CHECK(orderedphase_person_create_open(
              package, &args, &person_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(person_create != NULL && diagnostics == NULL);
  CHECK(type_bridge_projected_create_field_count(
            (const type_bridge_projected_create_t *)person_create,
            &LOCAL_PERSON_FIELD_TOKEN, &count, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 2u);
  CHECK(type_bridge_projected_create_field_value_at(
            (const type_bridge_projected_create_t *)person_create,
            &LOCAL_PERSON_FIELD_TOKEN, 0u, &generic_value, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_value_is(generic_value, VIEW("second")));
  CHECK(type_bridge_projected_value_close(&generic_value) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_projected_create_field_value_at(
            (const type_bridge_projected_create_t *)person_create,
            &LOCAL_PERSON_FIELD_TOKEN, 1u, &generic_value, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_value_is(generic_value, VIEW("first")));
  CHECK(type_bridge_projected_value_close(&generic_value) ==
        TYPE_BRIDGE_STATUS_OK);

  {
    const orderedphase_aliases *duplicate_values[] = {first_alias};
    orderedphase_person_create_field_aliases_chunks_v1_t tail = {0};
    orderedphase_person_create_field_aliases_chunks_v1_t head = {0};
    orderedphase_person_create_args_v1_t args = {0};
    tail.struct_size = sizeof(tail);
    tail.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    tail.values = duplicate_values;
    tail.count = 1u;
    head.struct_size = sizeof(head);
    head.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    head.values = duplicate_values;
    head.count = 1u;
    head.next = &tail;
    args.struct_size = sizeof(args);
    args.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    args.field_aliases_chunks = &head;
    duplicate_person_create =
        (orderedphase_person_create *)(uintptr_t)1u;
    CHECK(orderedphase_person_create_open(
              package, &args, &duplicate_person_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  }
  CHECK(duplicate_person_create == NULL);
  CHECK(diagnostic_is(
      diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
      "ordered_distinct_duplicate"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  {
    const orderedphase_plainzhactivity_participant_player *head_values[] = {
        second_player};
    const orderedphase_plainzhactivity_participant_player *tail_values[] = {
        first_player};
    orderedphase_plainzhactivity_create_role_participant_chunks_v1_t tail =
        {0};
    orderedphase_plainzhactivity_create_role_participant_chunks_v1_t head =
        {0};
    orderedphase_plainzhactivity_create_args_v1_t args = {0};
    tail.struct_size = sizeof(tail);
    tail.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    tail.values = tail_values;
    tail.count = 1u;
    head.struct_size = sizeof(head);
    head.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    head.values = head_values;
    head.count = 1u;
    head.next = &tail;
    args.struct_size = sizeof(args);
    args.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    args.role_participant_chunks = &head;
    CHECK(orderedphase_plainzhactivity_create_open(
              package, &args, &activity_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(activity_create != NULL && diagnostics == NULL);
  CHECK(type_bridge_projected_create_role_count(
            (const type_bridge_projected_create_t *)activity_create,
            &LOCAL_ACTIVITY_ROLE_TOKEN, &count, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 2u);
  CHECK(type_bridge_projected_create_role_reference_at(
            (const type_bridge_projected_create_t *)activity_create,
            &LOCAL_ACTIVITY_ROLE_TOKEN, 0u, &generic_reference,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_reference_is(generic_reference, VIEW("0x02")));
  CHECK(type_bridge_projected_reference_close(&generic_reference) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_projected_create_role_reference_at(
            (const type_bridge_projected_create_t *)activity_create,
            &LOCAL_ACTIVITY_ROLE_TOKEN, 1u, &generic_reference,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_reference_is(generic_reference, VIEW("0x01")));
  CHECK(type_bridge_projected_reference_close(&generic_reference) ==
        TYPE_BRIDGE_STATUS_OK);

  {
    const orderedphase_plainzhactivity_participant_player *duplicate_values[] = {
        first_player};
    orderedphase_plainzhactivity_create_role_participant_chunks_v1_t tail =
        {0};
    orderedphase_plainzhactivity_create_role_participant_chunks_v1_t head =
        {0};
    orderedphase_plainzhactivity_create_args_v1_t args = {0};
    tail.struct_size = sizeof(tail);
    tail.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    tail.values = duplicate_values;
    tail.count = 1u;
    head.struct_size = sizeof(head);
    head.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    head.values = duplicate_values;
    head.count = 1u;
    head.next = &tail;
    args.struct_size = sizeof(args);
    args.version = ORDEREDPHASE_CREATE_ARGS_VERSION;
    args.role_participant_chunks = &head;
    duplicate_activity_create =
        (orderedphase_plainzhactivity_create *)(uintptr_t)1u;
    CHECK(orderedphase_plainzhactivity_create_open(
              package, &args, &duplicate_activity_create, &diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  }
  CHECK(duplicate_activity_create == NULL);
  CHECK(diagnostic_is(
      diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
      "ordered_distinct_duplicate"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  {
    const type_bridge_projected_value_t *values[] = {
        (const type_bridge_projected_value_t *)second_alias,
        (const type_bridge_projected_value_t *)first_alias};
    type_bridge_projected_field_input_v1_t field = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    field.struct_size = sizeof(field);
    field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    field.field = &LOCAL_PERSON_FIELD_TOKEN;
    field.values = values;
    field.value_count = 2u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_PERSON_MODEL_TOKEN;
    descriptor.iid = VIEW("0x10");
    descriptor.fields = &field;
    descriptor.field_count = 1u;
    CHECK(type_bridge_projected_thing_open_v1(
              package, &descriptor, &person_thing, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(person_thing != NULL && diagnostics == NULL);
  CHECK(orderedphase_person_aliases_count(
            (const orderedphase_person *)person_thing, &count,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 2u);
  CHECK(orderedphase_person_aliases_at(
            (const orderedphase_person *)person_thing, 0u, &hydrated_alias,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_value_is(
      (const type_bridge_projected_value_t *)hydrated_alias, VIEW("second")));
  CHECK(orderedphase_aliases_close(&hydrated_alias) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_person_aliases_at(
            (const orderedphase_person *)person_thing, 1u, &hydrated_alias,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_value_is(
      (const type_bridge_projected_value_t *)hydrated_alias, VIEW("first")));
  CHECK(orderedphase_aliases_close(&hydrated_alias) == TYPE_BRIDGE_STATUS_OK);

  {
    const type_bridge_projected_value_t *values[] = {
        (const type_bridge_projected_value_t *)first_alias,
        (const type_bridge_projected_value_t *)first_alias};
    type_bridge_projected_field_input_v1_t field = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    field.struct_size = sizeof(field);
    field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    field.field = &LOCAL_PERSON_FIELD_TOKEN;
    field.values = values;
    field.value_count = 2u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_PERSON_MODEL_TOKEN;
    descriptor.iid = VIEW("0x11");
    descriptor.fields = &field;
    descriptor.field_count = 1u;
    duplicate_thing = (type_bridge_projected_thing_t *)(uintptr_t)1u;
    CHECK(type_bridge_projected_thing_open_v1(
              package, &descriptor, &duplicate_thing, &diagnostics) ==
          TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  }
  CHECK(duplicate_thing == NULL);
  CHECK(diagnostic_is(
      diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY,
      "ordered_distinct_duplicate"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  {
    const type_bridge_projected_reference_t *references[] = {
        (const type_bridge_projected_reference_t *)second_person,
        (const type_bridge_projected_reference_t *)first_person};
    type_bridge_projected_role_input_v1_t role = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    role.struct_size = sizeof(role);
    role.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    role.role = &LOCAL_ACTIVITY_ROLE_TOKEN;
    role.references = references;
    role.reference_count = 2u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_ACTIVITY_MODEL_TOKEN;
    descriptor.iid = VIEW("0x20");
    descriptor.roles = &role;
    descriptor.role_count = 1u;
    CHECK(type_bridge_projected_thing_open_v1(
              package, &descriptor, &activity_thing, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(activity_thing != NULL && diagnostics == NULL);
  CHECK(orderedphase_plainzhactivity_participant_count(
            (const orderedphase_plainzhactivity *)activity_thing, &count,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 2u);
  CHECK(orderedphase_plainzhactivity_participant_at(
            (const orderedphase_plainzhactivity *)activity_thing, 0u,
            &hydrated_player, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_as_person(
            hydrated_player, &hydrated_person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_reference_is(
      (const type_bridge_projected_reference_t *)hydrated_person,
      VIEW("0x02")));
  CHECK(orderedphase_person_ref_close(&hydrated_person) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_close(
            &hydrated_player) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_at(
            (const orderedphase_plainzhactivity *)activity_thing, 1u,
            &hydrated_player, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_as_person(
            hydrated_player, &hydrated_person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(projected_reference_is(
      (const type_bridge_projected_reference_t *)hydrated_person,
      VIEW("0x01")));
  CHECK(orderedphase_person_ref_close(&hydrated_person) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_close(
            &hydrated_player) == TYPE_BRIDGE_STATUS_OK);

  {
    const type_bridge_projected_reference_t *references[] = {
        (const type_bridge_projected_reference_t *)first_person,
        (const type_bridge_projected_reference_t *)first_person};
    type_bridge_projected_role_input_v1_t role = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    role.struct_size = sizeof(role);
    role.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    role.role = &LOCAL_ACTIVITY_ROLE_TOKEN;
    role.references = references;
    role.reference_count = 2u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &LOCAL_ACTIVITY_MODEL_TOKEN;
    descriptor.iid = VIEW("0x21");
    descriptor.roles = &role;
    descriptor.role_count = 1u;
    duplicate_thing = (type_bridge_projected_thing_t *)(uintptr_t)1u;
    CHECK(type_bridge_projected_thing_open_v1(
              package, &descriptor, &duplicate_thing, &diagnostics) ==
          TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  }
  CHECK(duplicate_thing == NULL);
  CHECK(diagnostic_is(
      diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY,
      "ordered_distinct_duplicate"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(orderedforeign_aliases_open(
            foreign_package, VIEW("foreign"), &foreign_alias,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    const type_bridge_projected_value_t *values[] = {
        (const type_bridge_projected_value_t *)foreign_alias};
    type_bridge_projected_field_input_v1_t field = {0};
    type_bridge_projected_thing_descriptor_v1_t descriptor = {0};
    field.struct_size = sizeof(field);
    field.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    field.field = &FOREIGN_PERSON_FIELD_TOKEN;
    field.values = values;
    field.value_count = 1u;
    descriptor.struct_size = sizeof(descriptor);
    descriptor.version = TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION;
    descriptor.model = &FOREIGN_PERSON_MODEL_TOKEN;
    descriptor.iid = VIEW("0x30");
    descriptor.fields = &field;
    descriptor.field_count = 1u;
    CHECK(type_bridge_projected_thing_open_v1(
              foreign_package, &descriptor, &foreign_person_thing,
              &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(foreign_person_thing != NULL && diagnostics == NULL);
  count = SIZE_MAX;
  CHECK(orderedphase_person_aliases_count(
            (const orderedphase_person *)foreign_person_thing, &count,
            &diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  CHECK(count == 0u);
  CHECK(diagnostic_is(
      diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY,
      "generated_token_package_mismatch"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(type_bridge_projected_thing_close(&foreign_person_thing) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_projected_thing_close(&activity_thing) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_projected_thing_close(&person_thing) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_create_close(&activity_create) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_person_create_close(&person_create) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_close(
            &second_player) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_plainzhactivity_participant_player_close(
            &first_player) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_person_ref_close(&second_person) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_person_ref_close(&first_person) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedforeign_aliases_close(&foreign_alias) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_aliases_close(&second_alias) == TYPE_BRIDGE_STATUS_OK);
  CHECK(orderedphase_aliases_close(&first_alias) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&foreign_package) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  CHECK(package == NULL && foreign_package == NULL && diagnostics == NULL);
  return 0;
}
"#
    .replace(
        "LOCAL_PERSON_MODEL_TOKEN",
        &format!(
            "orderedphase_projected_model_token_{}",
            fixture.person_model_ordinal
        ),
    )
    .replace(
        "LOCAL_PERSON_FIELD_TOKEN",
        &format!(
            "orderedphase_projected_field_token_{}",
            fixture.person_aliases_field_ordinal
        ),
    )
    .replace(
        "LOCAL_ACTIVITY_MODEL_TOKEN",
        &format!(
            "orderedphase_projected_model_token_{}",
            fixture.plain_activity_model_ordinal
        ),
    )
    .replace(
        "LOCAL_ACTIVITY_ROLE_TOKEN",
        &format!(
            "orderedphase_projected_role_token_{}",
            fixture.plain_activity_participant_role_ordinal
        ),
    )
    .replace(
        "FOREIGN_PERSON_MODEL_TOKEN",
        &format!(
            "orderedforeign_projected_model_token_{}",
            fixture.person_model_ordinal
        ),
    )
    .replace(
        "FOREIGN_PERSON_FIELD_TOKEN",
        &format!(
            "orderedforeign_projected_field_token_{}",
            fixture.person_aliases_field_ordinal
        ),
    );
    fs::write(&consumer, source).expect("ordered Phase-2 C17 consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let native_directory = native_library
        .parent()
        .expect("native library has a parent directory");
    let mut invocations = 0;
    for compiler in ["gcc", "clang"] {
        if !command_exists(compiler) {
            continue;
        }
        invocations += 1;
        let executable = stage.path().join(format!("ordered-consumer-{compiler}"));
        let output = Command::new(compiler)
            .args([
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
            ])
            .arg("-I")
            .arg(&runtime_include)
            .arg("-I")
            .arg(package_stage.join("include"))
            .arg("-I")
            .arg(foreign_stage.join("include"))
            .arg(package_stage.join("src/models.c"))
            .arg(foreign_stage.join("src/models.c"))
            .arg(&consumer)
            .arg("-L")
            .arg(native_directory)
            .arg("-ltype_bridge_c")
            .arg(format!("-Wl,-rpath,{}", native_directory.display()))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
        assert!(
            output.status.success(),
            "{compiler} failed to link the ordered Phase-2 C17 consumer:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let output = Command::new(&executable)
            .output()
            .unwrap_or_else(|error| panic!("failed to run {}: {error}", executable.display()));
        assert!(
            output.status.success(),
            "{compiler}'s ordered Phase-2 C17 consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    assert!(
        invocations > 0,
        "no GCC- or Clang-compatible C compiler was available"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn standalone_generated_consumer_is_clean_under_address_and_undefined_sanitizers() {
    let Some(native_library) = native_library_or_skip(
        "standalone_generated_consumer_is_clean_under_address_and_undefined_sanitizers",
    ) else {
        return;
    };
    let compiler = ["clang", "gcc"]
        .into_iter()
        .find(|compiler| command_exists(compiler))
        .expect("Clang or GCC is required for the sanitizer probe");
    let fixture = emitted_fixture();
    let stage = TempDirectory::new();
    write_package(&fixture.package, stage.path());
    let consumer = stage.path().join("sanitized-consumer.c");
    fs::write(
        &consumer,
        r#"#include <stddef.h>

#include <typebridge/type_bridge.h>
#include <fixture/models.h>

int main(void) {
  type_bridge_byte_view_t view = {0};
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  if (fixture_schema_package_open(&package, &diagnostics) !=
      TYPE_BRIDGE_STATUS_OK || package == NULL || diagnostics != NULL) {
    return 1;
  }
  if (type_bridge_schema_package_authority_json(package, &view) !=
          TYPE_BRIDGE_STATUS_OK ||
      view.data == NULL || view.length == 0u) {
    return 2;
  }
  if (type_bridge_schema_package_close(&package) != TYPE_BRIDGE_STATUS_OK ||
      package != NULL) {
    return 3;
  }

  return 0;
}
"#,
    )
    .expect("sanitized standalone consumer is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let native_directory = native_library
        .parent()
        .expect("native library has a parent directory");
    let executable = stage.path().join("sanitized-consumer");
    let output = Command::new(compiler)
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-fsanitize=address,undefined",
            "-fno-omit-frame-pointer",
        ])
        .arg("-I")
        .arg(&runtime_include)
        .arg("-I")
        .arg(&generated_include)
        .arg(&generated_source)
        .arg(&consumer)
        .arg("-L")
        .arg(native_directory)
        .arg("-ltype_bridge_c")
        .arg(format!("-Wl,-rpath,{}", native_directory.display()))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    assert!(
        output.status.success(),
        "{compiler} failed to build the sanitized consumer:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let output = Command::new(&executable)
        .env(
            "ASAN_OPTIONS",
            "detect_leaks=1:halt_on_error=1:abort_on_error=1",
        )
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .expect("sanitized consumer launches");
    assert!(
        output.status.success(),
        "sanitized consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn hostile_descriptors_fail_closed_with_initialized_outputs_and_diagnostics() {
    let bytes = emitted_fixture().bytes;

    let mut wrong_version = bytes.descriptor();
    wrong_version.abi_major += 1;
    assert_rejected(
        &wrong_version,
        TypeBridgeStatus::Unsupported,
        "c_schema_descriptor_abi_unsupported",
    );

    let mut reserved = bytes.descriptor();
    reserved.reserved[2] = 1;
    assert_rejected(
        &reserved,
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_descriptor_layout_mismatch",
    );

    let mut invalid_view = bytes.descriptor();
    invalid_view.declared_schema_json.data = ptr::null();
    assert_rejected(
        &invalid_view,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );

    let mut oversized = bytes.descriptor();
    oversized.runtime_projection_json = TypeBridgeByteView {
        data: NonNull::<u8>::dangling().as_ptr(),
        length: MAX_CANONICAL_BYTES + 1,
    };
    assert_rejected(
        &oversized,
        TypeBridgeStatus::ResourceLimit,
        "c_schema_descriptor_byte_limit_exceeded",
    );

    let mut tampered_declared = bytes.declared.clone();
    tampered_declared.push(b' ');
    let mut mismatch = bytes.descriptor();
    mismatch.declared_schema_json = view(&tampered_declared);
    assert_rejected(
        &mismatch,
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_package_evidence_mismatch",
    );

    let invalid_utf8 = [0xff];
    let mut malformed_authority = bytes.descriptor();
    malformed_authority.schema_authority_json = view(&invalid_utf8);
    assert_rejected(
        &malformed_authority,
        TypeBridgeStatus::SchemaPackageRejected,
        "malformed_canonical_json",
    );

    let mut package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    let mut diagnostics = NonNull::<TypeBridgeDiagnostics>::dangling().as_ptr();
    // SAFETY: both output slots are writable; a null descriptor is an intentional hostile input.
    assert_eq!(
        unsafe { type_bridge_schema_package_open_v1(ptr::null(), &mut package, &mut diagnostics) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(package.is_null());
    assert!(diagnostics.is_null());
}

#[test]
fn schema_package_open_v2_accepts_exact_flat_and_chunked_v1_descriptor_layouts() {
    let fixture = emitted_fixture();

    let mut flat = fixture.bytes.descriptor();
    flat.abi_minor = 4;
    let mut package = ptr::dangling_mut();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: the descriptor backing bytes and both output slots remain live.
        unsafe { type_bridge_schema_package_open_v2(&flat, &mut package, &mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
    assert!(!package.is_null());
    assert!(diagnostics.is_null());
    // SAFETY: the returned package owns the exact copied projection bytes.
    assert_eq!(
        unsafe { copied_view(package, type_bridge_schema_package_projection_json) },
        fixture.bytes.projection,
    );
    // SAFETY: this slot uniquely owns the returned package handle.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(&mut package) },
        TypeBridgeStatus::Ok,
    );

    let parts = fixture.bytes.chunked_parts(17);
    let mut chunked = parts.descriptor();
    chunked.abi_minor = 4;
    package = ptr::dangling_mut();
    diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: every chunk table/data range and both output slots remain live.
        unsafe {
            type_bridge_schema_package_open_chunked_v2(&chunked, &mut package, &mut diagnostics)
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!package.is_null());
    assert!(diagnostics.is_null());
    // SAFETY: the returned package owns the exact reassembled authority bytes.
    assert_eq!(
        unsafe { copied_view(package, type_bridge_schema_package_authority_json) },
        fixture.bytes.authority,
    );
    // SAFETY: this slot uniquely owns the returned package handle.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(&mut package) },
        TypeBridgeStatus::Ok,
    );
}

#[test]
fn schema_package_open_v2_classifies_all_seven_nonempty_hostile_evidence_slots() {
    let fixture = emitted_fixture().bytes;
    let mutations: [fn(&mut EmittedDescriptorBytes); 7] = [
        |bytes| bytes.authority = b"{".to_vec(),
        |bytes| bytes.declared.push(b' '),
        |bytes| bytes.projection = b"{".to_vec(),
        |bytes| bytes.semantic = b"{".to_vec(),
        |bytes| bytes.binding = b"{".to_vec(),
        |bytes| bytes.scope.extend_from_slice(b"-foreign"),
        |bytes| bytes.profile = b"typedb-3.11.5/v1".to_vec(),
    ];

    for mutate in mutations {
        let mut hostile = fixture.clone();
        mutate(&mut hostile);
        let mut descriptor = hostile.descriptor();
        descriptor.abi_minor = 4;
        assert_v2_flat_rejected(
            &descriptor,
            TypeBridgeStatus::ExecutionFailed,
            TypeBridgeExecutionDiagnosticCategory::Integrity,
            "projection_evidence_mismatch",
            1,
            0,
        );
    }
}

#[test]
fn schema_package_open_v2_retains_the_exact_missing_semantic_evidence_representative() {
    let fixture = emitted_fixture();
    let mut flat = fixture.bytes.descriptor();
    flat.abi_minor = 4;
    flat.semantic_fingerprint_json = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    let mut package = ptr::dangling_mut();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: all nonempty descriptor views and both output slots remain live.
        unsafe { type_bridge_schema_package_open_v2(&flat, &mut package, &mut diagnostics) },
        TypeBridgeStatus::ExecutionFailed,
    );
    assert!(package.is_null());
    assert!(!diagnostics.is_null());
    // SAFETY: the failed admission returned one live common diagnostic handle.
    unsafe { assert_missing_semantic_fingerprint_diagnostic(diagnostics) };
    // SAFETY: this slot uniquely owns the returned diagnostic handle.
    unsafe { close_execution_diagnostics(&mut diagnostics) };

    let parts = fixture.bytes.chunked_parts(19);
    let mut chunked = parts.descriptor();
    chunked.abi_minor = 4;
    chunked.semantic_fingerprint_json = TypeBridgeChunkedByteViewV1 {
        struct_size: size_of::<TypeBridgeChunkedByteViewV1>() as u32,
        version: 1,
        chunks: ptr::null(),
        chunk_count: 0,
        total_length: 0,
        reserved: [0; 4],
    };
    package = ptr::dangling_mut();
    diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: every nonempty chunk graph and both output slots remain live.
        unsafe {
            type_bridge_schema_package_open_chunked_v2(&chunked, &mut package, &mut diagnostics)
        },
        TypeBridgeStatus::ExecutionFailed,
    );
    assert!(package.is_null());
    assert!(!diagnostics.is_null());
    // SAFETY: the failed admission returned one live common diagnostic handle.
    unsafe { assert_missing_semantic_fingerprint_diagnostic(diagnostics) };
    // SAFETY: this slot uniquely owns the returned diagnostic handle.
    unsafe { close_execution_diagnostics(&mut diagnostics) };
}

#[test]
fn schema_package_open_v2_preserves_narrow_structural_statuses() {
    let fixture = emitted_fixture();

    let mut flat_layout = fixture.bytes.descriptor();
    flat_layout.abi_minor = 4;
    flat_layout.struct_size -= 1;
    assert_v2_flat_rejected(
        &flat_layout,
        TypeBridgeStatus::InvalidArgument,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_schema_descriptor_layout_mismatch",
        0,
        0,
    );

    let mut flat_abi = fixture.bytes.descriptor();
    flat_abi.abi_minor = 4;
    flat_abi.abi_major += 1;
    assert_v2_flat_rejected(
        &flat_abi,
        TypeBridgeStatus::Unsupported,
        TypeBridgeExecutionDiagnosticCategory::UnsupportedCapability,
        "c_schema_descriptor_abi_unsupported",
        0,
        0,
    );

    let mut flat_view = fixture.bytes.descriptor();
    flat_view.abi_minor = 4;
    flat_view.declared_schema_json.data = ptr::null();
    assert_v2_flat_rejected(
        &flat_view,
        TypeBridgeStatus::InvalidArgument,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_schema_descriptor_invalid_view",
        0,
        0,
    );

    let mut flat_resource = fixture.bytes.descriptor();
    flat_resource.abi_minor = 4;
    flat_resource.runtime_projection_json = TypeBridgeByteView {
        data: NonNull::<u8>::dangling().as_ptr(),
        length: MAX_CANONICAL_BYTES + 1,
    };
    assert_v2_flat_rejected(
        &flat_resource,
        TypeBridgeStatus::ResourceLimit,
        TypeBridgeExecutionDiagnosticCategory::ResourceLimit,
        "c_schema_package_resource_limit",
        0,
        0,
    );

    let parts = fixture.bytes.chunked_parts(23);
    let mut chunked_layout = parts.descriptor();
    chunked_layout.abi_minor = 4;
    chunked_layout.reserved0 = 1;
    assert_v2_chunked_rejected(
        &chunked_layout,
        TypeBridgeStatus::InvalidArgument,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_schema_chunked_descriptor_layout_mismatch",
        0,
        0,
    );

    let mut chunked_view = parts.descriptor();
    chunked_view.abi_minor = 4;
    chunked_view.declared_schema_json.version += 1;
    assert_v2_chunked_rejected(
        &chunked_view,
        TypeBridgeStatus::InvalidArgument,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput,
        "c_schema_descriptor_invalid_view",
        0,
        0,
    );

    let mut chunked_resource = parts.descriptor();
    chunked_resource.abi_minor = 4;
    chunked_resource.runtime_projection_json.total_length = MAX_CANONICAL_BYTES + 1;
    assert_v2_chunked_rejected(
        &chunked_resource,
        TypeBridgeStatus::ResourceLimit,
        TypeBridgeExecutionDiagnosticCategory::ResourceLimit,
        "c_schema_package_resource_limit",
        0,
        0,
    );
}

#[test]
fn schema_package_open_v2_rejects_aliases_before_writes_and_initializes_valid_outputs() {
    let fixture = emitted_fixture();
    let mut flat = fixture.bytes.descriptor();
    flat.abi_minor = 4;
    let flat_before = unsafe {
        std::slice::from_raw_parts(
            (&flat as *const TypeBridgeSchemaPackageDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageDescriptorV1>(),
        )
        .to_vec()
    };
    let descriptor_output = (&mut flat.schema_authority_json.data as *mut *const u8)
        .cast::<*mut TypeBridgeSchemaPackage>();
    let mut diagnostics = NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
    assert_eq!(
        // SAFETY: the hostile package output intentionally targets descriptor storage.
        unsafe { type_bridge_schema_package_open_v2(&flat, descriptor_output, &mut diagnostics) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, NonNull::dangling().as_ptr());
    let flat_after = unsafe {
        std::slice::from_raw_parts(
            (&flat as *const TypeBridgeSchemaPackageDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageDescriptorV1>(),
        )
    };
    assert_eq!(flat_after, flat_before);

    let mut shared_slot = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    let package_slot = &mut shared_slot as *mut *mut TypeBridgeSchemaPackage;
    let diagnostics_slot = package_slot.cast::<*mut TypeBridgeExecutionDiagnostics>();
    assert_eq!(
        // SAFETY: one writable pointer slot is intentionally supplied for both outputs.
        unsafe { type_bridge_schema_package_open_v2(&flat, package_slot, diagnostics_slot) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(shared_slot, NonNull::dangling().as_ptr());

    let parts = fixture.bytes.chunked_parts(29);
    let mut chunked = parts.descriptor();
    chunked.abi_minor = 4;
    let chunked_before = unsafe {
        std::slice::from_raw_parts(
            (&chunked as *const TypeBridgeSchemaPackageChunkedDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        )
        .to_vec()
    };
    let chunked_output = (&mut chunked.schema_authority_json.chunks
        as *mut *const TypeBridgeByteView)
        .cast::<*mut TypeBridgeSchemaPackage>();
    assert_eq!(
        // SAFETY: the hostile package output intentionally targets descriptor storage.
        unsafe {
            type_bridge_schema_package_open_chunked_v2(&chunked, chunked_output, &mut diagnostics)
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, NonNull::dangling().as_ptr());
    let chunked_after = unsafe {
        std::slice::from_raw_parts(
            (&chunked as *const TypeBridgeSchemaPackageChunkedDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        )
    };
    assert_eq!(chunked_after, chunked_before);

    let mut package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    assert_eq!(
        // SAFETY: the package slot is writable and the diagnostics output is intentionally null.
        unsafe { type_bridge_schema_package_open_v2(&flat, &mut package, ptr::null_mut()) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(package.is_null());

    diagnostics = NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
    assert_eq!(
        // SAFETY: the diagnostics slot is writable and the package output is intentionally null.
        unsafe { type_bridge_schema_package_open_v2(&flat, ptr::null_mut(), &mut diagnostics) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());

    package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    diagnostics = NonNull::<TypeBridgeExecutionDiagnostics>::dangling().as_ptr();
    assert_eq!(
        // SAFETY: both writable outputs are valid and null is an intentional descriptor input.
        unsafe { type_bridge_schema_package_open_v2(ptr::null(), &mut package, &mut diagnostics) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(package.is_null());
    assert!(!diagnostics.is_null());
    // SAFETY: the failed call returned one live typed diagnostic handle.
    let view = unsafe { one_execution_diagnostic(diagnostics) };
    assert_eq!(
        view.category,
        TypeBridgeExecutionDiagnosticCategory::InvalidInput
    );
    // SAFETY: the diagnostic handle remains live while its code is copied.
    assert_eq!(
        unsafe { execution_view_text(view.code) },
        "c_schema_descriptor_null"
    );
    // SAFETY: this slot uniquely owns the returned diagnostic handle.
    unsafe { close_execution_diagnostics(&mut diagnostics) };
}

#[test]
fn chunked_schema_package_open_reassembles_exact_bytes_and_rejects_aliases_read_only() {
    let fixture = emitted_fixture();
    let parts = fixture.bytes.chunked_parts(17);
    let descriptor = parts.descriptor();
    let mut package = ptr::dangling_mut();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: every table/data range and both writable outputs remain live.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(&descriptor, &mut package, &mut diagnostics)
        },
        TypeBridgeStatus::Ok,
    );
    assert!(!package.is_null());
    assert!(diagnostics.is_null());
    // SAFETY: the returned package owns the exact reassembled projection bytes.
    assert_eq!(
        unsafe { copied_view(package, type_bridge_schema_package_projection_json) },
        fixture.bytes.projection,
    );
    // SAFETY: the slot uniquely owns the returned package.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(&mut package) },
        TypeBridgeStatus::Ok,
    );

    let descriptor_before = unsafe {
        std::slice::from_raw_parts(
            (&descriptor as *const TypeBridgeSchemaPackageChunkedDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        )
        .to_vec()
    };
    let aliased_package = (&descriptor.schema_authority_json.chunks
        as *const *const TypeBridgeByteView)
        .cast_mut()
        .cast::<*mut TypeBridgeSchemaPackage>();
    diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: the hostile interior output is intentionally readable/writable storage.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(
                &descriptor,
                aliased_package,
                &mut diagnostics,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());
    let descriptor_after = unsafe {
        std::slice::from_raw_parts(
            (&descriptor as *const TypeBridgeSchemaPackageChunkedDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        )
    };
    assert_eq!(descriptor_after, descriptor_before);

    let mut wrong_total = parts.descriptor();
    wrong_total.runtime_projection_json.total_length += 1;
    assert_rejected_chunked(
        &wrong_total,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );
}

#[test]
fn chunked_schema_package_open_rejects_malformed_views_chunks_and_interior_aliases_read_only() {
    let fixture = emitted_fixture();
    let parts = fixture.bytes.chunked_parts(17);
    let descriptor = parts.descriptor();

    let mut malformed_top = descriptor;
    malformed_top.struct_size -= 1;
    assert_rejected_chunked(
        &malformed_top,
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_chunked_descriptor_layout_mismatch",
    );
    malformed_top = descriptor;
    malformed_top.reserved0 = 1;
    assert_rejected_chunked(
        &malformed_top,
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_chunked_descriptor_layout_mismatch",
    );

    let mut malformed_view = descriptor;
    malformed_view.runtime_projection_json.version += 1;
    assert_rejected_chunked(
        &malformed_view,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );
    malformed_view = descriptor;
    malformed_view.runtime_projection_json.reserved[2] = 1;
    assert_rejected_chunked(
        &malformed_view,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );
    malformed_view = descriptor;
    malformed_view.runtime_projection_json.chunks = ptr::null();
    assert_rejected_chunked(
        &malformed_view,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );

    let mut zero_chunk = parts.projection.clone();
    zero_chunk[0].length = 0;
    let mut malformed_chunk = descriptor;
    malformed_chunk.runtime_projection_json.chunks = zero_chunk.as_ptr();
    assert_rejected_chunked(
        &malformed_chunk,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );
    let mut null_chunk = parts.projection.clone();
    null_chunk[0].data = ptr::null();
    malformed_chunk.runtime_projection_json.chunks = null_chunk.as_ptr();
    assert_rejected_chunked(
        &malformed_chunk,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );

    let table_before = unsafe {
        std::slice::from_raw_parts(
            parts.projection.as_ptr().cast::<u8>(),
            parts.projection.len() * size_of::<TypeBridgeByteView>(),
        )
        .to_vec()
    };
    let table_output = parts
        .projection
        .as_ptr()
        .cast_mut()
        .cast::<*mut TypeBridgeSchemaPackage>();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: the hostile table-interior output intentionally targets live table storage.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(&descriptor, table_output, &mut diagnostics)
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());
    let table_after = unsafe {
        std::slice::from_raw_parts(
            parts.projection.as_ptr().cast::<u8>(),
            parts.projection.len() * size_of::<TypeBridgeByteView>(),
        )
    };
    assert_eq!(table_after, table_before);

    let projection_before = fixture.bytes.projection.clone();
    let data_output = fixture
        .bytes
        .projection
        .as_ptr()
        .cast_mut()
        .cast::<*mut TypeBridgeSchemaPackage>();
    assert_eq!(
        // SAFETY: the hostile data-interior output intentionally targets live chunk bytes.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(&descriptor, data_output, &mut diagnostics)
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());
    assert_eq!(fixture.bytes.projection, projection_before);

    let mut output_bytes = [0xa5_u8; 16];
    let package_output = output_bytes
        .as_mut_ptr()
        .cast::<*mut TypeBridgeSchemaPackage>();
    let diagnostics_output =
        unsafe { output_bytes.as_mut_ptr().add(1) }.cast::<*mut TypeBridgeDiagnostics>();
    assert_eq!(
        // SAFETY: both deliberately overlapping slots lie in writable sentinel storage.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(
                &descriptor,
                package_output,
                diagnostics_output,
            )
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(output_bytes, [0xa5; 16]);
}

#[test]
fn chunked_schema_package_open_enforces_chunk_bytes_and_flat_table_hosted_object_boundaries() {
    let fixture = emitted_fixture();
    let parts = fixture.bytes.chunked_parts(17);
    let descriptor = parts.descriptor();

    let byte = [b'x'];
    let overflowing_chunks = [
        TypeBridgeByteView {
            data: byte.as_ptr(),
            length: usize::MAX,
        },
        TypeBridgeByteView {
            data: byte.as_ptr(),
            length: 1,
        },
    ];
    let mut overflow = descriptor;
    overflow.schema_authority_json = TypeBridgeChunkedByteViewV1 {
        struct_size: size_of::<TypeBridgeChunkedByteViewV1>() as u32,
        version: 1,
        chunks: overflowing_chunks.as_ptr(),
        chunk_count: overflowing_chunks.len(),
        total_length: usize::MAX,
        reserved: [0; 4],
    };
    assert_rejected_chunked(
        &overflow,
        TypeBridgeStatus::ResourceLimit,
        "c_schema_descriptor_byte_limit_exceeded",
    );

    let exact_bytes = vec![b'x'; 32_768];
    let exact_chunk = [view(&exact_bytes)];
    let mut exact_chunk_descriptor = descriptor;
    exact_chunk_descriptor.schema_authority_json = TypeBridgeChunkedByteViewV1 {
        struct_size: size_of::<TypeBridgeChunkedByteViewV1>() as u32,
        version: 1,
        chunks: exact_chunk.as_ptr(),
        chunk_count: exact_chunk.len(),
        total_length: exact_bytes.len(),
        reserved: [0; 4],
    };
    assert_rejected_chunked(
        &exact_chunk_descriptor,
        TypeBridgeStatus::SchemaPackageRejected,
        "malformed_canonical_json",
    );
    let over_bytes = vec![b'x'; 32_769];
    let over_chunk = [view(&over_bytes)];
    let mut over_chunk_descriptor = descriptor;
    over_chunk_descriptor.schema_authority_json = TypeBridgeChunkedByteViewV1 {
        chunks: over_chunk.as_ptr(),
        total_length: over_bytes.len(),
        ..exact_chunk_descriptor.schema_authority_json
    };
    assert_rejected_chunked(
        &over_chunk_descriptor,
        TypeBridgeStatus::InvalidArgument,
        "c_schema_descriptor_invalid_view",
    );

    let table_chunk_count = 65_535 / size_of::<TypeBridgeByteView>();
    assert_eq!(table_chunk_count, 4_095);
    let one_byte_chunks = vec![view(&byte); table_chunk_count + 1];
    let mut exact_table = descriptor;
    exact_table.schema_authority_json = TypeBridgeChunkedByteViewV1 {
        struct_size: size_of::<TypeBridgeChunkedByteViewV1>() as u32,
        version: 1,
        chunks: one_byte_chunks.as_ptr(),
        chunk_count: table_chunk_count,
        total_length: table_chunk_count,
        reserved: [0; 4],
    };
    assert_rejected_chunked(
        &exact_table,
        TypeBridgeStatus::SchemaPackageRejected,
        "malformed_canonical_json",
    );

    let mut over_table = exact_table;
    over_table.schema_authority_json.chunk_count += 1;
    over_table.schema_authority_json.total_length += 1;
    let descriptor_before = unsafe {
        std::slice::from_raw_parts(
            (&over_table as *const TypeBridgeSchemaPackageChunkedDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        )
        .to_vec()
    };
    let mut package = ptr::dangling_mut();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: the +1 table descriptor remains readable; preflight is read-only.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(&over_table, &mut package, &mut diagnostics)
        },
        TypeBridgeStatus::ResourceLimit,
    );
    assert_eq!(package, ptr::dangling_mut());
    assert_eq!(diagnostics, ptr::dangling_mut());
    let descriptor_after = unsafe {
        std::slice::from_raw_parts(
            (&over_table as *const TypeBridgeSchemaPackageChunkedDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageChunkedDescriptorV1>(),
        )
    };
    assert_eq!(descriptor_after, descriptor_before);
}

fn assert_rejected_chunked(
    descriptor: &TypeBridgeSchemaPackageChunkedDescriptorV1,
    expected_status: TypeBridgeStatus,
    expected_code: &str,
) {
    let mut package = ptr::dangling_mut();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: descriptor backing storage and both output slots remain live.
        unsafe {
            type_bridge_schema_package_open_chunked_v1(descriptor, &mut package, &mut diagnostics)
        },
        expected_status,
    );
    assert!(package.is_null());
    assert!(!diagnostics.is_null());
    // SAFETY: failure returned one owned diagnostic handle.
    let json = unsafe { diagnostic_text(diagnostics) };
    assert!(
        json.contains(&format!("\"code\":\"{expected_code}\"")),
        "{json}"
    );
    // SAFETY: this slot uniquely owns the diagnostic handle.
    assert_eq!(
        unsafe { type_bridge_diagnostics_close(&mut diagnostics) },
        TypeBridgeStatus::Ok,
    );
}

#[test]
fn legacy_schema_open_rejects_descriptor_and_blob_interior_outputs_without_writes() {
    let fixture = emitted_fixture();
    let mut descriptor = fixture.bytes.descriptor();
    let descriptor_before = unsafe {
        std::slice::from_raw_parts(
            (&descriptor as *const TypeBridgeSchemaPackageDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageDescriptorV1>(),
        )
        .to_vec()
    };
    let descriptor_output = (&mut descriptor.schema_authority_json.data as *mut *const u8)
        .cast::<*mut TypeBridgeSchemaPackage>();
    let mut diagnostics = ptr::dangling_mut();
    assert_eq!(
        // SAFETY: hostile descriptor-interior output is intentionally writable storage.
        unsafe {
            type_bridge_schema_package_open_v1(&descriptor, descriptor_output, &mut diagnostics)
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(diagnostics, ptr::dangling_mut());
    let descriptor_after = unsafe {
        std::slice::from_raw_parts(
            (&descriptor as *const TypeBridgeSchemaPackageDescriptorV1).cast::<u8>(),
            size_of::<TypeBridgeSchemaPackageDescriptorV1>(),
        )
    };
    assert_eq!(descriptor_after, descriptor_before);

    let authority_before = fixture.bytes.authority.clone();
    let blob_output = fixture
        .bytes
        .authority
        .as_ptr()
        .cast_mut()
        .cast::<*mut TypeBridgeSchemaPackage>();
    assert_eq!(
        // SAFETY: hostile mutable backing bytes are deliberately selected as output storage.
        unsafe { type_bridge_schema_package_open_v1(&descriptor, blob_output, &mut diagnostics) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(fixture.bytes.authority, authority_before);
    assert_eq!(diagnostics, ptr::dangling_mut());
}

#[test]
fn self_consistent_projection_that_was_not_derived_from_authority_is_rejected() {
    let mut bytes = emitted_fixture().bytes;
    let decoded =
        decode_runtime_projection_verified(&bytes.projection, &bytes.semantic, &bytes.binding)
            .expect("emitted projection verifies");
    let hollow = RuntimeProjection::try_new(
        BindingTarget::C,
        decoded.config().clone(),
        decoded.semantic_fingerprint().clone(),
        decoded.generator_handlers(),
        decoded.code_resources(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        EmissionPlan::new(
            Vec::new(),
            Vec::<BTreeSet<_>>::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("empty emission plan is structurally valid"),
    )
    .expect("hollow projection is internally self-consistent");
    bytes.projection = to_canonical_json(&hollow).expect("hollow projection encodes");
    bytes.binding =
        to_canonical_json(hollow.projection_fingerprint()).expect("binding fingerprint encodes");

    assert_rejected(
        &bytes.descriptor(),
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_package_evidence_mismatch",
    );
}

#[test]
fn projections_with_incomplete_or_mutated_emitter_resources_are_rejected() {
    let original = emitted_fixture().bytes;
    let authority = decode_schema_authority(
        &original.authority,
        &schema_authority_capability_vocabulary(),
    )
    .expect("emitted authority verifies");
    let decoded = decode_runtime_projection_verified(
        &original.projection,
        &original.semantic,
        &original.binding,
    )
    .expect("emitted projection verifies");

    let incomplete = project(
        authority.resolved_schema(),
        BindingTarget::C,
        decoded.config(),
        decoded.generator_handlers(),
        &[],
    )
    .expect("projection without resource evidence remains internally consistent");
    let mut incomplete_bytes = original.clone();
    incomplete_bytes.projection =
        to_canonical_json(&incomplete).expect("incomplete projection encodes");
    incomplete_bytes.binding = to_canonical_json(incomplete.projection_fingerprint())
        .expect("incomplete binding fingerprint encodes");
    assert_rejected(
        &incomplete_bytes.descriptor(),
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_package_evidence_mismatch",
    );

    let mut mutated_resources = decoded.code_resources().to_vec();
    let first_id = mutated_resources
        .first()
        .expect("emitted C projection references resources")
        .id()
        .as_str()
        .to_owned();
    mutated_resources[0] = CodeResourceDigest::from_bytes(first_id, b"forged C emitter resource")
        .expect("forged resource digest is structurally valid");
    let mutated = project(
        authority.resolved_schema(),
        BindingTarget::C,
        decoded.config(),
        decoded.generator_handlers(),
        &mutated_resources,
    )
    .expect("projection with mutated resource evidence remains internally consistent");
    let mut mutated_bytes = original;
    mutated_bytes.projection = to_canonical_json(&mutated).expect("mutated projection encodes");
    mutated_bytes.binding = to_canonical_json(mutated.projection_fingerprint())
        .expect("mutated binding fingerprint encodes");
    assert_rejected(
        &mutated_bytes.descriptor(),
        TypeBridgeStatus::SchemaPackageRejected,
        "c_schema_package_evidence_mismatch",
    );
}

#[test]
fn independent_schema_package_handles_are_thread_safe_under_repeated_open_read_close() {
    const THREADS: usize = 4;
    const ITERATIONS: usize = 32;

    let template = emitted_fixture().bytes;
    let start = Arc::new(Barrier::new(THREADS));
    let workers = (0..THREADS)
        .map(|_| {
            let bytes = template.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                start.wait();
                for _ in 0..ITERATIONS {
                    let descriptor = bytes.descriptor();
                    let mut package = ptr::null_mut();
                    let mut diagnostics = ptr::null_mut();
                    // SAFETY: this thread owns the descriptor backing bytes and
                    // both writable output slots for the complete call.
                    assert_eq!(
                        unsafe {
                            type_bridge_schema_package_open_v1(
                                &descriptor,
                                &mut package,
                                &mut diagnostics,
                            )
                        },
                        TypeBridgeStatus::Ok,
                    );
                    assert!(!package.is_null());
                    assert!(diagnostics.is_null());
                    // SAFETY: this thread exclusively owns the live package handle.
                    assert_eq!(
                        unsafe { copied_view(package, type_bridge_schema_package_projection_json) },
                        bytes.projection,
                    );
                    // SAFETY: this thread returns its exclusively owned handle
                    // through the matching close family before the next iteration.
                    assert_eq!(
                        unsafe { type_bridge_schema_package_close(&mut package) },
                        TypeBridgeStatus::Ok,
                    );
                    assert!(package.is_null());
                }
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().expect("concurrent C ABI worker completes");
    }
}

#[test]
fn partially_null_open_outputs_still_clear_the_writable_slot() {
    let bytes = emitted_fixture().bytes;
    let descriptor = bytes.descriptor();

    let mut package = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    // SAFETY: the package slot is writable and the diagnostics slot is an
    // intentional null argument.
    assert_eq!(
        unsafe { type_bridge_schema_package_open_v1(&descriptor, &mut package, ptr::null_mut()) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(package.is_null());

    let mut diagnostics = NonNull::<TypeBridgeDiagnostics>::dangling().as_ptr();
    // SAFETY: the diagnostics slot is writable and the package slot is an
    // intentional null argument.
    assert_eq!(
        unsafe {
            type_bridge_schema_package_open_v1(&descriptor, ptr::null_mut(), &mut diagnostics)
        },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(diagnostics.is_null());
}

#[test]
fn aliased_open_output_slots_are_rejected_without_writes() {
    let bytes = emitted_fixture().bytes;
    let descriptor = bytes.descriptor();
    let mut shared_slot = NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr();
    let package_slot = &mut shared_slot as *mut *mut TypeBridgeSchemaPackage;
    let diagnostics_slot = package_slot.cast::<*mut TypeBridgeDiagnostics>();

    // SAFETY: the one writable pointer-sized slot is deliberately supplied for
    // both outputs to prove the ABI rejects ownership aliasing before opening.
    assert_eq!(
        unsafe { type_bridge_schema_package_open_v1(&descriptor, package_slot, diagnostics_slot,) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert_eq!(
        shared_slot,
        NonNull::<TypeBridgeSchemaPackage>::dangling().as_ptr()
    );
}

#[test]
fn abi_metadata_and_null_handle_paths_are_stable_and_initialize_views() {
    // SAFETY: these ABI metadata functions take no caller-owned inputs.
    assert_eq!(unsafe { type_bridge_c_abi_major() }, 1);
    // SAFETY: these ABI metadata functions take no caller-owned inputs.
    assert_eq!(unsafe { type_bridge_c_abi_minor() }, 4);

    let mut version = TypeBridgeByteView {
        data: ptr::null(),
        length: 0,
    };
    // SAFETY: `version` is a writable output slot.
    assert_eq!(
        unsafe { type_bridge_runtime_version(&mut version) },
        TypeBridgeStatus::Ok,
    );
    // SAFETY: a successful runtime-version call returns static bytes.
    let version = unsafe { std::slice::from_raw_parts(version.data, version.length) };
    assert_eq!(version, env!("CARGO_PKG_VERSION").as_bytes());

    let mut output = TypeBridgeByteView {
        data: NonNull::<u8>::dangling().as_ptr(),
        length: usize::MAX,
    };
    // SAFETY: null is an intentional hostile handle and `output` is writable.
    assert_eq!(
        unsafe { type_bridge_schema_package_projection_json(ptr::null(), &mut output) },
        TypeBridgeStatus::InvalidArgument,
    );
    assert!(output.data.is_null());
    assert_eq!(output.length, 0);

    // SAFETY: a null output slot is an intentional hostile argument.
    assert_eq!(
        unsafe { type_bridge_runtime_version(ptr::null_mut()) },
        TypeBridgeStatus::InvalidArgument,
    );
    // SAFETY: a null ownership slot is an intentional hostile argument.
    assert_eq!(
        unsafe { type_bridge_schema_package_close(ptr::null_mut()) },
        TypeBridgeStatus::InvalidArgument,
    );
    // SAFETY: a null ownership slot is an intentional hostile argument.
    assert_eq!(
        unsafe { type_bridge_diagnostics_close(ptr::null_mut()) },
        TypeBridgeStatus::InvalidArgument,
    );
}

#[cfg(unix)]
fn required_live_environment(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => panic!("{name} must not be empty for the exact C runtime live test"),
        Err(std::env::VarError::NotPresent) => {
            panic!("{name} must be configured for the exact C runtime live test")
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            panic!("{name} must contain valid UTF-8 for the exact C runtime live test")
        }
    }
}

#[cfg(unix)]
struct IsolatedLiveDatabase<'runtime> {
    runtime: &'runtime tokio::runtime::Runtime,
    database: type_bridge_orm::Database,
    cleanup_complete: bool,
}

#[cfg(unix)]
impl IsolatedLiveDatabase<'_> {
    fn create(&self) -> bool {
        self.runtime
            .block_on(self.database.create_database())
            .is_ok()
    }

    fn cleanup(&mut self) -> bool {
        let deleted = self
            .runtime
            .block_on(self.database.delete_database())
            .is_ok();
        if !deleted {
            return false;
        }
        let closed = self.database.close().is_ok();
        self.cleanup_complete = closed;
        self.cleanup_complete
    }
}

#[cfg(unix)]
impl Drop for IsolatedLiveDatabase<'_> {
    fn drop(&mut self) {
        if !self.cleanup_complete {
            let _ = self.runtime.block_on(self.database.delete_database());
            let _ = self.database.close();
        }
    }
}

/// Exact Phase-3 acceptance: a compiled C17 consumer owns the complete
/// runtime/database/transaction lifecycle against an isolated TypeDB 3.12.3.
///
/// Run through the repository integration lane, or directly after building
/// the shared library:
///
/// ```text
/// TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
/// TYPEDB_ADDRESS=127.0.0.1:<grpc-port> TYPEDB_HTTP_PORT=<http-port> \
/// TYPE_BRIDGE_C_INTG_DATABASE=type_bridge_c_runtime_live_<unique> \
/// cargo test --locked -p type-bridge-c --test schema_package_abi \
///   live_c17_consumer_exercises_exact_3_12_3_transaction_lifecycle \
///   -- --exact --ignored --nocapture
/// ```
#[cfg(unix)]
#[test]
#[ignore = "requires an isolated exact TypeDB 3.12.3 server and the C shared library"]
fn live_c17_consumer_exercises_exact_3_12_3_transaction_lifecycle() {
    let native_library = native_library().unwrap_or_else(|| {
        panic!("build the TypeBridge C shared library before running the exact C runtime live test")
    });
    let compiler = ["gcc", "clang"]
        .into_iter()
        .find(|candidate| command_exists(candidate))
        .expect("GCC or Clang is required for the exact C runtime live test");
    let address = required_live_environment("TYPEDB_ADDRESS");
    let database_name = required_live_environment("TYPE_BRIDGE_C_INTG_DATABASE");
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned());
    let http_port_text = required_live_environment("TYPEDB_HTTP_PORT");
    let http_port = http_port_text
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .expect("TYPEDB_HTTP_PORT must be an integer from 1 through 65535");

    let fixture = emitted_fixture();
    let stage = TempDirectory::new();
    write_package(&fixture.package, stage.path());
    let consumer = stage.path().join("runtime-live.c");
    fs::write(
        &consumer,
        r#"#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <typebridge/type_bridge.h>
#include <fixture/models.h>

#define CHECK(condition) do { if (!(condition)) { return __LINE__; } } while (0)

static type_bridge_byte_view_t view_of(const char *text) {
  type_bridge_byte_view_t view;
  view.data = (const uint8_t *)text;
  view.length = strlen(text);
  return view;
}

static uint32_t required_http_port(void) {
  const char *text = getenv("TYPEDB_HTTP_PORT");
  char *end = NULL;
  unsigned long value;
  if (text == NULL || text[0] == '\0') {
    return 0u;
  }
  errno = 0;
  value = strtoul(text, &end, 10);
  if (errno != 0 || end == text || *end != '\0' || value == 0ul ||
      value > 65535ul) {
    return 0u;
  }
  return (uint32_t)value;
}

static int same_text(type_bridge_byte_view_t view, const char *expected) {
  const size_t expected_length = strlen(expected);
  return view.length == expected_length && view.data != NULL &&
         memcmp(view.data, expected, expected_length) == 0;
}

static int close_execution_diagnostics(
    type_bridge_execution_diagnostics_t **diagnostics) {
  return type_bridge_execution_diagnostics_close(diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         *diagnostics == NULL;
}

int main(void) {
  const char *address = getenv("TYPEDB_ADDRESS");
  const char *database_name = getenv("TYPE_BRIDGE_C_INTG_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
  const uint32_t http_port = required_http_port();
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_read_transaction_t *read_transaction = NULL;
  type_bridge_write_transaction_t *write_transaction = NULL;
  type_bridge_byte_view_t version = {NULL, 0u};
  type_bridge_runtime_config_v1_t runtime_config = {
      sizeof(type_bridge_runtime_config_v1_t),
      TYPE_BRIDGE_RUNTIME_CONFIG_VERSION,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN,
      0u,
      {0u, 0u, 0u, 0u}};
  type_bridge_database_config_v1_t database_config;

  CHECK(address != NULL && address[0] != '\0');
  CHECK(database_name != NULL && database_name[0] != '\0');
  CHECK(username != NULL && username[0] != '\0');
  CHECK(password != NULL);
  CHECK(http_port != 0u);

  memset(&database_config, 0, sizeof(database_config));
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  database_config.address = view_of(address);
  database_config.database = view_of(database_name);
  database_config.username = view_of(username);
  database_config.password = view_of(password);
  database_config.http_port = http_port;
  database_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;

  CHECK(fixture_schema_package_open(&package, &package_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && package_diagnostics == NULL);
  CHECK(type_bridge_runtime_open_v1(
            &runtime_config, &runtime, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(runtime != NULL && diagnostics == NULL);
  CHECK(type_bridge_database_open_v1(
            runtime, package, &database_config, NULL, &database,
            &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(database != NULL && diagnostics == NULL);
  CHECK(type_bridge_database_server_version(database, &version) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(version, "3.12.3"));

  /* Database ownership retains the verified package authority. */
  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  CHECK(package == NULL);

  CHECK(type_bridge_read_transaction_open(
            database, NULL, &read_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(read_transaction != NULL && diagnostics == NULL);
  {
    type_bridge_database_t *const original = database;
    CHECK(type_bridge_database_close(&database, &diagnostics) ==
          TYPE_BRIDGE_STATUS_IN_USE);
    CHECK(database == original && diagnostics != NULL);
    CHECK(close_execution_diagnostics(&diagnostics));
  }
  CHECK(type_bridge_read_transaction_close(
            &read_transaction, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(read_transaction == NULL && diagnostics == NULL);

  CHECK(type_bridge_write_transaction_open(
            database, NULL, &write_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(write_transaction != NULL && diagnostics == NULL);
  CHECK(type_bridge_write_transaction_commit(
            &write_transaction, NULL, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(write_transaction == NULL && diagnostics == NULL);

  CHECK(type_bridge_write_transaction_open(
            database, NULL, &write_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_write_transaction_rollback(
            &write_transaction, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(write_transaction == NULL && diagnostics == NULL);

  CHECK(type_bridge_write_transaction_open(
            database, NULL, &write_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_write_transaction_close(
            &write_transaction, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(write_transaction == NULL && diagnostics == NULL);

  {
    type_bridge_runtime_t *const original = runtime;
    CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
          TYPE_BRIDGE_STATUS_IN_USE);
    CHECK(runtime == original && diagnostics != NULL);
    CHECK(close_execution_diagnostics(&diagnostics));
  }
  CHECK(type_bridge_database_close(&database, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(database == NULL && diagnostics == NULL);
  CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(runtime == NULL && diagnostics == NULL);
  return 0;
}
"#,
    )
    .expect("exact C runtime live consumer source is written");

    let runtime_include = Path::new(env!("CARGO_MANIFEST_DIR")).join("include");
    let generated_include = stage.path().join("include");
    let generated_source = stage.path().join("src/models.c");
    let native_directory = native_library
        .parent()
        .expect("native library has a parent directory");
    let executable = stage.path().join("runtime-live");
    let compile = Command::new(compiler)
        .args([
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
        ])
        .arg("-I")
        .arg(&runtime_include)
        .arg("-I")
        .arg(&generated_include)
        .arg(&generated_source)
        .arg(&consumer)
        .arg("-L")
        .arg(native_directory)
        .arg("-ltype_bridge_c")
        .arg(format!("-Wl,-rpath,{}", native_directory.display()))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {compiler}: {error}"));
    assert!(
        compile.status.success(),
        "{compiler} failed to build the exact C runtime live consumer:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let administration_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("the live database administration runtime opens");
    let options = type_bridge_orm::ConnectOptions {
        http_port,
        tls: false,
        server_version: None,
    };
    let administrator = administration_runtime
        .block_on(type_bridge_orm::Database::connect_with_options(
            &address,
            &database_name,
            &username,
            &password,
            options,
        ))
        .unwrap_or_else(|_| panic!("the isolated TypeDB administration connection opens"));
    let detected = administrator
        .server_version()
        .expect("the live administration connection has authoritative version evidence");
    assert_eq!(
        (detected.major, detected.minor, detected.patch),
        (3, 12, 3),
        "the exact C runtime live test requires TypeDB 3.12.3"
    );
    assert!(
        !administration_runtime
            .block_on(administrator.database_exists())
            .unwrap_or_else(|_| panic!("the isolated live database absence check succeeds")),
        "TYPE_BRIDGE_C_INTG_DATABASE must name an absent isolated database"
    );
    let mut isolated_database = IsolatedLiveDatabase {
        runtime: &administration_runtime,
        database: administrator,
        cleanup_complete: false,
    };
    assert!(
        isolated_database.create(),
        "the isolated live database is created"
    );

    let run = Command::new(&executable)
        .env("TYPEDB_ADDRESS", &address)
        .env("TYPEDB_HTTP_PORT", &http_port_text)
        .env("TYPE_BRIDGE_C_INTG_DATABASE", &database_name)
        .env("TYPEDB_USERNAME", &username)
        .env("TYPEDB_PASSWORD", &password)
        .output()
        .expect("the exact C runtime live consumer launches");

    assert!(
        isolated_database.cleanup(),
        "the isolated live database is deleted and its administration connection closes"
    );
    assert!(
        run.status.success(),
        "the exact C runtime live consumer failed with {}:\nstdout:\n{}\nstderr:\n{}",
        run.status,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}
