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

ABI_ASSERT(TYPE_BRIDGE_C_ABI_MAJOR == 2u, "ABI major drifted");
ABI_ASSERT(TYPE_BRIDGE_C_ABI_MINOR == 0u, "C ABI is no longer supported");
ABI_ASSERT(TYPE_BRIDGE_DATABASE_CONFIG_VERSION == 2u,
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
ABI_ASSERT(offsetof(type_bridge_database_config_t, struct_size) == 0u,
                    "config struct_size offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, version) == 4u,
                    "config version offset drifted");

#if UINTPTR_MAX == UINT64_MAX
ABI_ASSERT(sizeof(type_bridge_database_config_t) == 336u,
                    "LP64/LLP64 config size drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, address) == 8u,
                    "LP64/LLP64 address offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, database) == 24u,
                    "LP64/LLP64 database offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, username) == 40u,
                    "LP64/LLP64 username offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, password) == 56u,
                    "LP64/LLP64 password offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, http_port) == 72u,
                    "LP64/LLP64 http_port offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, tls_mode) == 76u,
                    "LP64/LLP64 tls_mode offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, custom_root_ca_pem) == 80u,
                    "LP64/LLP64 custom-root offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, connection_limits) == 96u,
                    "LP64/LLP64 connection limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, answer_limits) == 200u,
                    "LP64/LLP64 answer limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, reserved) == 304u,
                    "LP64/LLP64 reserved offset drifted");
#elif UINTPTR_MAX == UINT32_MAX
ABI_ASSERT(sizeof(type_bridge_database_config_t) == 296u,
                    "ILP32 config size drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, address) == 8u,
                    "ILP32 address offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, database) == 16u,
                    "ILP32 database offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, username) == 24u,
                    "ILP32 username offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, password) == 32u,
                    "ILP32 password offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, http_port) == 40u,
                    "ILP32 http_port offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, tls_mode) == 44u,
                    "ILP32 tls_mode offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, custom_root_ca_pem) == 48u,
                    "ILP32 custom-root offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, connection_limits) == 56u,
                    "ILP32 connection limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, answer_limits) == 160u,
                    "ILP32 answer limits offset drifted");
ABI_ASSERT(offsetof(type_bridge_database_config_t, reserved) == 264u,
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
    const type_bridge_database_config_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_open_fn)(
    const type_bridge_runtime_t *, const type_bridge_schema_package_t *,
    const type_bridge_database_config_t *,
    const type_bridge_cancellation_t *, type_bridge_database_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_query_limits_open_fn)(
    const type_bridge_runtime_t *, const type_bridge_schema_package_t *,
    const type_bridge_database_config_t *,
    const type_bridge_query_limit_override_v1_t *, size_t,
    const type_bridge_cancellation_t *, type_bridge_database_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_query_limits_open_fn)(
    const type_bridge_schema_package_t *,
    const type_bridge_proxy_connection_options_v1_t *,
    const type_bridge_query_limit_override_v1_t *, size_t,
    const type_bridge_cancellation_t *, type_bridge_proxy_connection_t **,
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
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_get_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_iid_t *, const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_update_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_iid_t *, const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_delete_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_iid_t *, const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_count_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_get_fn)(
    const type_bridge_read_transaction_t *,
    const type_bridge_projected_token_v1_t *, const type_bridge_iid_t *,
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
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_get_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, const type_bridge_iid_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_thing_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_update_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, const type_bridge_iid_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_delete_fn)(
    const type_bridge_write_transaction_t *,
    const type_bridge_projected_token_v1_t *, const type_bridge_iid_t *,
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
    type_bridge_projected_batch_builder_t *, const type_bridge_iid_t *,
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
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *batch_result_item_fn)(
    const type_bridge_projected_batch_result_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_projected_batch_operation_t, type_bridge_projected_thing_t **,
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

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_filter_open_fn)(
    const type_bridge_schema_package_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_manager_filter_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_filter_iid_fn)(
    const type_bridge_manager_filter_t *, const type_bridge_iid_t *,
    type_bridge_manager_filter_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_filter_iids_fn)(
    const type_bridge_manager_filter_t *, const type_bridge_iid_t *const *, size_t,
    type_bridge_manager_filter_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_filter_compare_fn)(
    const type_bridge_manager_filter_t *, const type_bridge_projected_token_v1_t *,
    type_bridge_query_comparison_t, const type_bridge_projected_value_t *,
    type_bridge_manager_filter_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_filter_close_fn)(type_bridge_manager_filter_t **);

typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_all_fn)(
    const type_bridge_database_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_manager_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_first_fn)(
    const type_bridge_database_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_count_fn)(
    const type_bridge_database_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint64_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_exists_fn)(
    const type_bridge_database_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint8_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_transaction_manager_all_fn)(
    const type_bridge_read_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_manager_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_transaction_manager_first_fn)(
    const type_bridge_read_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_transaction_manager_count_fn)(
    const type_bridge_read_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint64_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *read_transaction_manager_exists_fn)(
    const type_bridge_read_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint8_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_all_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_manager_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_first_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_count_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint64_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_exists_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint8_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_all_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_manager_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_first_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_count_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint64_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_exists_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    uint8_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_result_count_fn)(
    const type_bridge_manager_result_t *, const type_bridge_projected_token_v1_t *,
    size_t *, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_result_at_fn)(
    const type_bridge_manager_result_t *, const type_bridge_projected_token_v1_t *,
    size_t, type_bridge_projected_thing_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_result_close_fn)(type_bridge_manager_result_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_update_fn)(
    const type_bridge_database_t *, const type_bridge_manager_filter_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_delete_fn)(
    const type_bridge_database_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_update_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_delete_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_update_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_manager_filter_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_delete_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_manager_filter_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_create_fn)(
    const type_bridge_database_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_create_fn)(
    const type_bridge_write_transaction_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_create_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_projected_token_v1_t *,
    const type_bridge_projected_create_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_batch_execute_fn)(
    const type_bridge_proxy_connection_t *, const type_bridge_projected_batch_t *,
    const type_bridge_query_execution_limits_v1_t *, const type_bridge_cancellation_t *,
    type_bridge_projected_batch_result_t **, type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *manager_filter_validate_fn)(
    const type_bridge_manager_filter_t *, const type_bridge_manager_filter_t *,
    const type_bridge_projected_token_v1_t *,
    type_bridge_execution_diagnostics_t **);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_delete_returning_fn)(
    const type_bridge_database_t *owner,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_iid_t *iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out,
    type_bridge_execution_diagnostics_t **diagnostics);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *database_manager_delete_many_returning_fn)(
    const type_bridge_database_t *owner,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_iid_t *const *iids, size_t count,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out,
    type_bridge_execution_diagnostics_t **diagnostics);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_delete_returning_fn)(
    const type_bridge_write_transaction_t *owner,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_iid_t *iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out,
    type_bridge_execution_diagnostics_t **diagnostics);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *write_transaction_manager_delete_many_returning_fn)(
    const type_bridge_write_transaction_t *owner,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_iid_t *const *iids, size_t count,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out,
    type_bridge_execution_diagnostics_t **diagnostics);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_delete_returning_fn)(
    const type_bridge_proxy_connection_t *owner,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_iid_t *iid,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out,
    type_bridge_execution_diagnostics_t **diagnostics);
typedef type_bridge_status_t (TYPE_BRIDGE_CALL *proxy_connection_manager_delete_many_returning_fn)(
    const type_bridge_proxy_connection_t *owner,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_iid_t *const *iids, size_t count,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out,
    type_bridge_execution_diagnostics_t **diagnostics);

#define TAKE(type, symbol) do { type value = &symbol; (void)value; } while (0)

void type_bridge_abi_probe(void) {
  TAKE(database_manager_delete_returning_fn, type_bridge_database_manager_delete_returning);
  TAKE(database_manager_delete_many_returning_fn, type_bridge_database_manager_delete_many_returning);
  TAKE(write_transaction_manager_delete_returning_fn, type_bridge_write_transaction_manager_delete_returning);
  TAKE(write_transaction_manager_delete_many_returning_fn, type_bridge_write_transaction_manager_delete_many_returning);
  TAKE(proxy_connection_manager_delete_returning_fn, type_bridge_proxy_connection_manager_delete_returning);
  TAKE(proxy_connection_manager_delete_many_returning_fn, type_bridge_proxy_connection_manager_delete_many_returning);
  TAKE(manager_filter_validate_fn, type_bridge_manager_filter_validate_model);
  TAKE(proxy_batch_execute_fn, type_bridge_proxy_connection_projected_batch_execute);
  TAKE(proxy_batch_execute_fn, type_bridge_proxy_connection_projected_batch_delete_returning);
  TAKE(database_manager_create_fn, type_bridge_database_manager_insert);
  TAKE(database_manager_create_fn, type_bridge_database_manager_put);
  TAKE(write_transaction_manager_create_fn, type_bridge_write_transaction_manager_insert);
  TAKE(write_transaction_manager_create_fn, type_bridge_write_transaction_manager_put);
  TAKE(proxy_connection_manager_create_fn, type_bridge_proxy_connection_manager_insert);
  TAKE(proxy_connection_manager_create_fn, type_bridge_proxy_connection_manager_put);
  TAKE(database_manager_update_fn, type_bridge_database_manager_update);
  TAKE(database_manager_delete_fn, type_bridge_database_manager_delete);
  TAKE(write_transaction_manager_update_fn, type_bridge_write_transaction_manager_update);
  TAKE(write_transaction_manager_delete_fn, type_bridge_write_transaction_manager_delete);
  TAKE(proxy_connection_manager_update_fn, type_bridge_proxy_connection_manager_update);
  TAKE(proxy_connection_manager_delete_fn, type_bridge_proxy_connection_manager_delete);
  TAKE(manager_filter_open_fn, type_bridge_manager_filter_open);
  TAKE(manager_filter_iid_fn, type_bridge_manager_filter_iid);
  TAKE(manager_filter_iids_fn, type_bridge_manager_filter_iids);
  TAKE(manager_filter_compare_fn, type_bridge_manager_filter_compare);
  TAKE(manager_filter_close_fn, type_bridge_manager_filter_close);
  TAKE(database_manager_all_fn, type_bridge_database_manager_all);
  TAKE(database_manager_first_fn, type_bridge_database_manager_first);
  TAKE(database_manager_count_fn, type_bridge_database_manager_count);
  TAKE(database_manager_exists_fn, type_bridge_database_manager_exists);
  TAKE(read_transaction_manager_all_fn, type_bridge_read_transaction_manager_all);
  TAKE(read_transaction_manager_first_fn, type_bridge_read_transaction_manager_first);
  TAKE(read_transaction_manager_count_fn, type_bridge_read_transaction_manager_count);
  TAKE(read_transaction_manager_exists_fn, type_bridge_read_transaction_manager_exists);
  TAKE(write_transaction_manager_all_fn, type_bridge_write_transaction_manager_all);
  TAKE(write_transaction_manager_first_fn, type_bridge_write_transaction_manager_first);
  TAKE(write_transaction_manager_count_fn, type_bridge_write_transaction_manager_count);
  TAKE(write_transaction_manager_exists_fn, type_bridge_write_transaction_manager_exists);
  TAKE(proxy_connection_manager_all_fn, type_bridge_proxy_connection_manager_all);
  TAKE(proxy_connection_manager_first_fn, type_bridge_proxy_connection_manager_first);
  TAKE(proxy_connection_manager_count_fn, type_bridge_proxy_connection_manager_count);
  TAKE(proxy_connection_manager_exists_fn, type_bridge_proxy_connection_manager_exists);
  TAKE(manager_result_count_fn, type_bridge_manager_result_count);
  TAKE(manager_result_at_fn, type_bridge_manager_result_at);
  TAKE(manager_result_close_fn, type_bridge_manager_result_close);
  TAKE(schema_open_fn, type_bridge_schema_package_open);
  TAKE(schema_chunked_open_fn, type_bridge_schema_package_open_chunked);
  TAKE(config_validate_fn, type_bridge_database_config_validate);
  TAKE(database_open_fn, type_bridge_database_open);
  TAKE(database_query_limits_open_fn, type_bridge_database_open_with_query_limits);
  TAKE(proxy_query_limits_open_fn, type_bridge_proxy_connection_open_with_query_limits_v1);
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
  TAKE(database_batch_execute_fn, type_bridge_database_projected_batch_delete_returning);
  TAKE(write_batch_execute_fn, type_bridge_write_transaction_projected_batch_delete_returning);
  TAKE(batch_result_count_fn, type_bridge_projected_batch_result_count);
  TAKE(batch_result_item_fn, type_bridge_projected_batch_result_item);
  TAKE(batch_result_thing_fn, type_bridge_projected_batch_result_thing_at);
  TAKE(batch_result_close_fn, type_bridge_projected_batch_result_close);
}
