#pragma once

#include <typebridge/type_bridge.h>

#define TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION 2u
#define TYPE_BRIDGE_DATABASE_CUSTOM_ROOT_CA_BYTES_MAX 1048576u
#define TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA 2u

typedef int32_t type_bridge_projected_batch_operation_t;
#define TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT                         \
  ((type_bridge_projected_batch_operation_t)1)
#define TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT                            \
  ((type_bridge_projected_batch_operation_t)2)
#define TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE                         \
  ((type_bridge_projected_batch_operation_t)3)
#define TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE                         \
  ((type_bridge_projected_batch_operation_t)4)

#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER                  \
  ((type_bridge_generated_opaque_input_kind_t)34)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH                          \
  ((type_bridge_generated_opaque_input_kind_t)35)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT                   \
  ((type_bridge_generated_opaque_input_kind_t)36)

/*
 * Version-2 direct database policy. Every byte view is copied during the
 * call. Connection and answer limits are tighten-only common ceilings.
 * Reserved words must be zero.
 */
typedef struct type_bridge_database_config_v2 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_byte_view_t address;
  type_bridge_byte_view_t database;
  type_bridge_byte_view_t username;
  type_bridge_byte_view_t password;
  uint32_t http_port;
  uint32_t tls_mode;
  type_bridge_byte_view_t custom_root_ca_pem;
  type_bridge_query_execution_limits_v1_t connection_limits;
  type_bridge_query_execution_limits_v1_t answer_limits;
  uint64_t reserved[4];
} type_bridge_database_config_v2_t;

typedef struct type_bridge_projected_batch_builder
    type_bridge_projected_batch_builder_t;
typedef struct type_bridge_projected_batch type_bridge_projected_batch_t;
typedef struct type_bridge_projected_batch_result
    type_bridge_projected_batch_result_t;

#ifdef __cplusplus
extern "C" {
#endif

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_open_v2(
    const type_bridge_schema_package_descriptor_v1_t *descriptor,
    type_bridge_schema_package_t **out_package,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_open_chunked_v2(
    const type_bridge_schema_package_chunked_descriptor_v1_t *descriptor,
    type_bridge_schema_package_t **out_package,
    type_bridge_execution_diagnostics_t **out_diagnostics);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_config_validate_v2(
    const type_bridge_database_config_v2_t *config,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_open_v2(
    const type_bridge_runtime_t *runtime,
    const type_bridge_schema_package_t *package,
    const type_bridge_database_config_v2_t *config,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_database_t **out_database,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_open_v2(
    const type_bridge_database_t *database,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_read_transaction_t **out_transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_open_v2(
    const type_bridge_database_t *database,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_write_transaction_t **out_transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_commit_v2(
    type_bridge_write_transaction_t **transaction,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/* Policy-aware exact entity CRUD. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_insert_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_put_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_get_by_iid_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_update_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_delete_by_iid_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_count_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_entity_get_by_iid_v2(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_entity_count_v2(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_insert_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_put_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_get_by_iid_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_update_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_delete_by_iid_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_count_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/* Policy-aware exact relation CRUD. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_insert_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_put_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_get_by_iid_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_update_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_delete_by_iid_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_count_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_relation_get_by_iid_v2(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_relation_count_v2(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_insert_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_put_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_get_by_iid_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_update_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_delete_by_iid_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_count_v2(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/* Bounded homogeneous projected mutation batches. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_builder_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_projected_batch_operation_t operation,
    const type_bridge_query_execution_limits_v1_t *construction_limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_builder_t **out_builder,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_builder_add_v1(
    type_bridge_projected_batch_builder_t *builder,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_builder_finish(
    type_bridge_projected_batch_builder_t **builder,
    type_bridge_projected_batch_t **out_batch,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_builder_close(
    type_bridge_projected_batch_builder_t **builder);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_close(type_bridge_projected_batch_t **batch);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_projected_batch_execute_v1(
    const type_bridge_database_t *database,
    const type_bridge_projected_batch_t *batch,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_projected_batch_execute_v1(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_batch_t *batch,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_result_count(
    const type_bridge_projected_batch_result_t *result,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_batch_operation_t expected_operation,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_result_thing_at(
    const type_bridge_projected_batch_result_t *result,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_batch_operation_t expected_operation,
    size_t index,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_batch_result_close(
    type_bridge_projected_batch_result_t **result);

#ifdef __cplusplus
}
#endif
