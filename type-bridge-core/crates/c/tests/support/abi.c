#include <stddef.h>
#include <stdint.h>

#include <typebridge/type_bridge.h>

#ifdef __cplusplus
#define ABI_ASSERT(value, message) static_assert(value, message)
#define ABI_ALIGNOF(type) alignof(type)
#else
#define ABI_ASSERT(value, message) _Static_assert(value, message)
#define ABI_ALIGNOF(type) _Alignof(type)
#endif


#if UINTPTR_MAX == UINT64_MAX
ABI_ASSERT(sizeof(type_bridge_migration_execution_options_v1_t) == 48u, "migration options size");
ABI_ASSERT(ABI_ALIGNOF(type_bridge_migration_execution_options_v1_t) == 8u, "migration options alignment");
ABI_ASSERT(sizeof(type_bridge_projected_codec_options_v1_t) == 72u, "codec options size");
ABI_ASSERT(ABI_ALIGNOF(type_bridge_projected_codec_options_v1_t) == 8u, "codec options alignment");
ABI_ASSERT(offsetof(type_bridge_projected_codec_options_v1_t, cancellation) == 64u, "codec cancellation offset");
#endif

ABI_ASSERT(TYPE_BRIDGE_C_ABI_MAJOR == 1u, "ABI major drifted");
ABI_ASSERT(TYPE_BRIDGE_C_ABI_MINOR == 6u, "C ABI is no longer supported");
ABI_ASSERT(TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION == 2u,
                    "database config version drifted");
ABI_ASSERT(TYPE_BRIDGE_DATABASE_CUSTOM_ROOT_CA_BYTES_MAX == 1048576u,
                    "custom-root ceiling drifted");
ABI_ASSERT(TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA == 2u,
                    "custom-root TLS tag drifted");
ABI_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT == 1,
                    "insert batch tag drifted");
ABI_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT == 2,
                    "put batch tag drifted");
ABI_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE == 3,
                    "update batch tag drifted");
ABI_ASSERT(TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE == 4,
                    "delete batch tag drifted");
ABI_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER == 34,
                    "batch-builder input tag drifted");
ABI_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH == 35,
                    "batch input tag drifted");
ABI_ASSERT(TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT == 36,
                    "batch-result input tag drifted");
ABI_ASSERT(sizeof(type_bridge_query_execution_limits_v1_t) == 104u,
                    "nested limits layout drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, struct_size) == 0u,
                    "config struct_size offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, version) == 4u,
                    "config version offset drifted");

#if UINTPTR_MAX == UINT64_MAX
ABI_ASSERT(sizeof(type_bridge_database_config_v2_t) == 336u,
                    "LP64/LLP64 config size drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, address) == 8u,
                    "LP64/LLP64 address offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, database) == 24u,
                    "LP64/LLP64 database offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, username) == 40u,
                    "LP64/LLP64 username offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, password) == 56u,
                    "LP64/LLP64 password offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, http_port) == 72u,
                    "LP64/LLP64 http_port offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, tls_mode) == 76u,
                    "LP64/LLP64 tls_mode offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, custom_root_ca_pem) == 80u,
                    "LP64/LLP64 custom-root offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, connection_limits) == 96u,
                    "LP64/LLP64 connection limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, answer_limits) == 200u,
                    "LP64/LLP64 answer limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, reserved) == 304u,
                    "LP64/LLP64 reserved offset drifted");
#elif UINTPTR_MAX == UINT32_MAX
ABI_ASSERT(sizeof(type_bridge_database_config_v2_t) == 296u,
                    "ILP32 config size drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, address) == 8u,
                    "ILP32 address offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, database) == 16u,
                    "ILP32 database offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, username) == 24u,
                    "ILP32 username offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, password) == 32u,
                    "ILP32 password offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, http_port) == 40u,
                    "ILP32 http_port offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, tls_mode) == 44u,
                    "ILP32 tls_mode offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, custom_root_ca_pem) == 48u,
                    "ILP32 custom-root offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, connection_limits) == 56u,
                    "ILP32 connection limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, answer_limits) == 160u,
                    "ILP32 answer limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_v2_t, reserved) == 264u,
                    "ILP32 reserved offset drifted");
#else
#error "C ABI supports only frozen 32-bit and 64-bit pointer layouts"
#endif

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *schema_open_fn)(
    const type_bridge_schema_package_descriptor_v1_t *,
    type_bridge_schema_package_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *schema_chunked_open_fn)(
    const type_bridge_schema_package_chunked_descriptor_v1_t *,
    type_bridge_schema_package_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *config_validate_fn)(
    const type_bridge_database_config_v2_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_open_fn)(
    const type_bridge_runtime_t *, const type_bridge_schema_package_t *,
    const type_bridge_database_config_v2_t *,
    const type_bridge_cancellation_t *, type_bridge_database_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_open_fn)(
    const type_bridge_database_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_read_transaction_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_open_fn)(
    const type_bridge_database_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_write_transaction_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_commit_fn)(
    type_bridge_write_transaction_t **,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_create_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_get_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_byte_view_t, const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_update_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_byte_view_t, const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_delete_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_byte_view_t, const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_count_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_get_fn)(
    const type_bridge_read_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_count_fn)(
    const type_bridge_read_transaction_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_create_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_get_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_update_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_delete_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, type_bridge_byte_view_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_count_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_open_fn)(
    const type_bridge_schema_package_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_builder_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_add_fn)(
    type_bridge_projected_batch_builder_t *, type_bridge_byte_view_t,
    const type_bridge_projected_create_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_finish_fn)(
    type_bridge_projected_batch_builder_t **, type_bridge_projected_batch_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_builder_close_fn)(
    type_bridge_projected_batch_builder_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_close_fn)(
    type_bridge_projected_batch_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_batch_execute_fn)(
    const type_bridge_database_t *, const type_bridge_projected_batch_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_batch_execute_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_batch_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_count_fn)(
    const type_bridge_projected_batch_result_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t, size_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_thing_fn)(
    const type_bridge_projected_batch_result_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t, size_t,
    type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_close_fn)(
    type_bridge_projected_batch_result_t **);

#define TAKE(type, symbol) do { type value = &symbol; (void)value; } while (0)

void type_bridge_abi_probe(void) {
  TAKE(schema_open_fn, type_bridge_schema_package_open_v2);
  TAKE(schema_chunked_open_fn, type_bridge_schema_package_open_chunked_v2);
  TAKE(config_validate_fn, type_bridge_database_config_validate_v2);
  TAKE(database_open_fn, type_bridge_database_open_v2);
  TAKE(read_open_fn, type_bridge_read_transaction_open_v2);
  TAKE(write_open_fn, type_bridge_write_transaction_open_v2);
  TAKE(write_commit_fn, type_bridge_write_transaction_commit_v2);
  TAKE(database_create_fn, type_bridge_database_entity_insert_v2);
  TAKE(database_create_fn, type_bridge_database_entity_put_v2);
  TAKE(database_get_fn, type_bridge_database_entity_get_by_iid_v2);
  TAKE(database_update_fn, type_bridge_database_entity_update_v2);
  TAKE(database_delete_fn, type_bridge_database_entity_delete_by_iid_v2);
  TAKE(database_count_fn, type_bridge_database_entity_count_v2);
  TAKE(read_get_fn, type_bridge_read_transaction_entity_get_by_iid_v2);
  TAKE(read_count_fn, type_bridge_read_transaction_entity_count_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_entity_insert_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_entity_put_v2);
  TAKE(write_get_fn, type_bridge_write_transaction_entity_get_by_iid_v2);
  TAKE(write_update_fn, type_bridge_write_transaction_entity_update_v2);
  TAKE(write_delete_fn, type_bridge_write_transaction_entity_delete_by_iid_v2);
  TAKE(write_count_fn, type_bridge_write_transaction_entity_count_v2);
  TAKE(database_create_fn, type_bridge_database_relation_insert_v2);
  TAKE(database_create_fn, type_bridge_database_relation_put_v2);
  TAKE(database_get_fn, type_bridge_database_relation_get_by_iid_v2);
  TAKE(database_update_fn, type_bridge_database_relation_update_v2);
  TAKE(database_delete_fn, type_bridge_database_relation_delete_by_iid_v2);
  TAKE(database_count_fn, type_bridge_database_relation_count_v2);
  TAKE(read_get_fn, type_bridge_read_transaction_relation_get_by_iid_v2);
  TAKE(read_count_fn, type_bridge_read_transaction_relation_count_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_relation_insert_v2);
  TAKE(write_create_fn, type_bridge_write_transaction_relation_put_v2);
  TAKE(write_get_fn, type_bridge_write_transaction_relation_get_by_iid_v2);
  TAKE(write_update_fn, type_bridge_write_transaction_relation_update_v2);
  TAKE(write_delete_fn, type_bridge_write_transaction_relation_delete_by_iid_v2);
  TAKE(write_count_fn, type_bridge_write_transaction_relation_count_v2);
  TAKE(batch_builder_open_fn, type_bridge_projected_batch_builder_open_v1);
  TAKE(batch_builder_add_fn, type_bridge_projected_batch_builder_add_v1);
  TAKE(batch_builder_finish_fn, type_bridge_projected_batch_builder_finish);
  TAKE(batch_builder_close_fn, type_bridge_projected_batch_builder_close);
  TAKE(batch_close_fn, type_bridge_projected_batch_close);
  TAKE(database_batch_execute_fn, type_bridge_database_projected_batch_execute_v1);
  TAKE(write_batch_execute_fn, type_bridge_write_transaction_projected_batch_execute_v1);
  TAKE(batch_result_count_fn, type_bridge_projected_batch_result_count);
  TAKE(batch_result_thing_fn, type_bridge_projected_batch_result_thing_at);
  TAKE(batch_result_close_fn, type_bridge_projected_batch_result_close);
}
