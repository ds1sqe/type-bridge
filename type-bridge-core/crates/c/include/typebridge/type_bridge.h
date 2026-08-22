#ifndef TYPEBRIDGE_TYPE_BRIDGE_H
#define TYPEBRIDGE_TYPE_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#if defined(TYPE_BRIDGE_C_BUILDING)
#define TYPE_BRIDGE_API __declspec(dllexport)
#else
#define TYPE_BRIDGE_API __declspec(dllimport)
#endif
#define TYPE_BRIDGE_CALL __cdecl
#else
#define TYPE_BRIDGE_API __attribute__((visibility("default")))
#define TYPE_BRIDGE_CALL
#endif

/*
 * Native layout/behavior compatibility, independent from the TypeBridge
 * product version returned by type_bridge_runtime_version().
 */
#define TYPE_BRIDGE_C_ABI_MAJOR 1u
#define TYPE_BRIDGE_C_ABI_MINOR 5u
#define TYPE_BRIDGE_CHUNKED_BYTE_VIEW_VERSION 1u
#define TYPE_BRIDGE_PROJECTED_TOKEN_VERSION 1u

#ifdef __cplusplus
extern "C" {
#endif

typedef int32_t type_bridge_status_t;

#define TYPE_BRIDGE_STATUS_OK ((type_bridge_status_t)0)
#define TYPE_BRIDGE_STATUS_INVALID_ARGUMENT ((type_bridge_status_t)1)
#define TYPE_BRIDGE_STATUS_SCHEMA_PACKAGE_REJECTED ((type_bridge_status_t)2)
#define TYPE_BRIDGE_STATUS_UNSUPPORTED ((type_bridge_status_t)3)
#define TYPE_BRIDGE_STATUS_RESOURCE_LIMIT ((type_bridge_status_t)4)
#define TYPE_BRIDGE_STATUS_EXECUTION_FAILED ((type_bridge_status_t)5)
#define TYPE_BRIDGE_STATUS_CANCELLED ((type_bridge_status_t)6)
#define TYPE_BRIDGE_STATUS_COMMIT_OUTCOME_UNKNOWN ((type_bridge_status_t)7)
#define TYPE_BRIDGE_STATUS_IN_USE ((type_bridge_status_t)8)
#define TYPE_BRIDGE_STATUS_PANIC ((type_bridge_status_t)255)

typedef struct type_bridge_byte_view {
  const uint8_t *data;
  size_t length;
} type_bridge_byte_view_t;

#define TYPE_BRIDGE_PROJECTED_TOKEN_MODEL 1u
#define TYPE_BRIDGE_PROJECTED_TOKEN_FIELD 2u
#define TYPE_BRIDGE_PROJECTED_TOKEN_ROLE 3u
#define TYPE_BRIDGE_PROJECTED_TOKEN_FUNCTION 4u

/*
 * Generated-only semantic token. Application code receives these constants
 * from a generated schema package; it must not construct or modify them.
 */
typedef struct type_bridge_projected_token_v1 {
  uint32_t struct_size;
  uint32_t version;
  uint32_t kind;
  uint32_t ordinal;
  uint8_t projection_digest[32];
  uint64_t reserved[4];
} type_bridge_projected_token_v1_t;

/*
 * Version-1 generated schema descriptor. All byte views are caller-owned,
 * immutable for the duration of open_v1, non-empty, and need not be NUL
 * terminated. Reserved words must be zero.
 */
typedef struct type_bridge_schema_package_descriptor_v1 {
  uint32_t struct_size;
  uint32_t abi_major;
  uint32_t abi_minor;
  type_bridge_byte_view_t schema_authority_json;
  type_bridge_byte_view_t declared_schema_json;
  type_bridge_byte_view_t runtime_projection_json;
  type_bridge_byte_view_t semantic_fingerprint_json;
  type_bridge_byte_view_t binding_fingerprint_json;
  type_bridge_byte_view_t managed_scope;
  type_bridge_byte_view_t semantic_profile;
  uint64_t reserved[4];
} type_bridge_schema_package_descriptor_v1_t;

/* Versioned bounded scatter view used by generated large schema packages. */
typedef struct type_bridge_chunked_byte_view_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_byte_view_t *chunks;
  size_t chunk_count;
  size_t total_length;
  uint64_t reserved[4];
} type_bridge_chunked_byte_view_v1_t;

typedef struct type_bridge_schema_package_chunked_descriptor_v1 {
  uint32_t struct_size;
  uint32_t abi_major;
  uint32_t abi_minor;
  uint32_t reserved0;
  type_bridge_chunked_byte_view_v1_t schema_authority_json;
  type_bridge_chunked_byte_view_v1_t declared_schema_json;
  type_bridge_chunked_byte_view_v1_t runtime_projection_json;
  type_bridge_chunked_byte_view_v1_t semantic_fingerprint_json;
  type_bridge_chunked_byte_view_v1_t binding_fingerprint_json;
  type_bridge_chunked_byte_view_v1_t managed_scope;
  type_bridge_chunked_byte_view_v1_t semantic_profile;
  uint64_t reserved[4];
} type_bridge_schema_package_chunked_descriptor_v1_t;

typedef struct type_bridge_schema_package type_bridge_schema_package_t;
typedef struct type_bridge_diagnostics type_bridge_diagnostics_t;
typedef struct type_bridge_execution_diagnostics
    type_bridge_execution_diagnostics_t;
typedef struct type_bridge_projected_value type_bridge_projected_value_t;
typedef struct type_bridge_projected_reference type_bridge_projected_reference_t;
typedef struct type_bridge_projected_create type_bridge_projected_create_t;
typedef struct type_bridge_projected_create_builder
    type_bridge_projected_create_builder_t;
typedef struct type_bridge_projected_thing type_bridge_projected_thing_t;
typedef struct type_bridge_runtime type_bridge_runtime_t;
typedef struct type_bridge_database type_bridge_database_t;
typedef struct type_bridge_read_transaction type_bridge_read_transaction_t;
typedef struct type_bridge_write_transaction type_bridge_write_transaction_t;
typedef struct type_bridge_cancellation type_bridge_cancellation_t;
typedef struct type_bridge_query_session type_bridge_query_session_t;
typedef struct type_bridge_query_binding type_bridge_query_binding_t;
typedef struct type_bridge_query_field type_bridge_query_field_t;
typedef struct type_bridge_query_role type_bridge_query_role_t;
typedef struct type_bridge_query_function type_bridge_query_function_t;
typedef struct type_bridge_query_function_value
    type_bridge_query_function_value_t;
typedef struct type_bridge_query_function_call
    type_bridge_query_function_call_t;
typedef struct type_bridge_query_predicate type_bridge_query_predicate_t;
typedef struct type_bridge_query_order type_bridge_query_order_t;
typedef struct type_bridge_query_selection type_bridge_query_selection_t;
typedef struct type_bridge_query type_bridge_query_t;
typedef struct type_bridge_query_terminal type_bridge_query_terminal_t;
typedef struct type_bridge_query_result type_bridge_query_result_t;
typedef struct type_bridge_query_remote_context
    type_bridge_query_remote_context_t;
typedef struct type_bridge_query_remote_pending
    type_bridge_query_remote_pending_t;
typedef struct type_bridge_query_remote_claim type_bridge_query_remote_claim_t;

#define TYPE_BRIDGE_RUNTIME_CONFIG_VERSION 1u
#define TYPE_BRIDGE_DATABASE_CONFIG_VERSION 1u
#define TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN 1u
#define TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MAX 64u
#define TYPE_BRIDGE_DATABASE_ADDRESS_BYTES_MAX 4096u
#define TYPE_BRIDGE_DATABASE_NAME_BYTES_MAX 256u
#define TYPE_BRIDGE_DATABASE_USERNAME_BYTES_MAX 4096u
#define TYPE_BRIDGE_DATABASE_PASSWORD_BYTES_MAX 65536u
#define TYPE_BRIDGE_THING_IID_BYTES_MAX 258u
#define TYPE_BRIDGE_PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX 256u
#define TYPE_BRIDGE_TLS_DISABLED 0u
#define TYPE_BRIDGE_TLS_NATIVE_ROOTS 1u

#define TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION 1u
#define TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_VERSION 1u
#define TYPE_BRIDGE_QUERY_SELECTED_SLOT_MAX 16u
#define TYPE_BRIDGE_QUERY_ORDER_TERM_MAX 16u
#define TYPE_BRIDGE_QUERY_BOOLEAN_TERM_MAX 256u
#define TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_MAX 256u
#define TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX 65535u
#define TYPE_BRIDGE_QUERY_OUTPUT_NAME_BYTES_MAX 128u
#define TYPE_BRIDGE_QUERY_DEFAULT_TIMEOUT_MILLISECONDS UINT64_C(30000)
#define TYPE_BRIDGE_QUERY_DEFAULT_ITEMS UINT64_C(65536)
#define TYPE_BRIDGE_QUERY_DEFAULT_BYTES UINT64_C(33554432)
#define TYPE_BRIDGE_QUERY_DEFAULT_GRAPH_NODES UINT64_C(65536)
#define TYPE_BRIDGE_QUERY_DEFAULT_ATTRIBUTE_VALUES UINT64_C(65536)
#define TYPE_BRIDGE_QUERY_DEFAULT_COLLECTION_MEMBERS UINT64_C(65536)
#define TYPE_BRIDGE_QUERY_DEFAULT_ROLE_PLAYERS UINT64_C(65536)
#define TYPE_BRIDGE_QUERY_DEFAULT_STATEMENTS 3u
#define TYPE_BRIDGE_QUERY_REMOTE_ENVELOPE_BYTES_MAX 33554432u

typedef uint32_t type_bridge_query_match_mode_t;
#define TYPE_BRIDGE_QUERY_MATCH_EXACT ((type_bridge_query_match_mode_t)1)
#define TYPE_BRIDGE_QUERY_MATCH_SUBTYPES ((type_bridge_query_match_mode_t)2)

typedef uint32_t type_bridge_query_comparison_t;
#define TYPE_BRIDGE_QUERY_COMPARE_EQUAL ((type_bridge_query_comparison_t)1)
#define TYPE_BRIDGE_QUERY_COMPARE_NOT_EQUAL ((type_bridge_query_comparison_t)2)
#define TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN ((type_bridge_query_comparison_t)3)
#define TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN_OR_EQUAL                         \
  ((type_bridge_query_comparison_t)4)
#define TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN                              \
  ((type_bridge_query_comparison_t)5)
#define TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL                     \
  ((type_bridge_query_comparison_t)6)
#define TYPE_BRIDGE_QUERY_COMPARE_CONTAINS ((type_bridge_query_comparison_t)7)
#define TYPE_BRIDGE_QUERY_COMPARE_STARTS_WITH                              \
  ((type_bridge_query_comparison_t)8)
#define TYPE_BRIDGE_QUERY_COMPARE_ENDS_WITH ((type_bridge_query_comparison_t)9)
#define TYPE_BRIDGE_QUERY_COMPARE_REGEX ((type_bridge_query_comparison_t)10)

typedef uint32_t type_bridge_query_predicate_combine_t;
#define TYPE_BRIDGE_QUERY_PREDICATE_AND                                    \
  ((type_bridge_query_predicate_combine_t)1)
#define TYPE_BRIDGE_QUERY_PREDICATE_OR                                     \
  ((type_bridge_query_predicate_combine_t)2)
#define TYPE_BRIDGE_QUERY_PREDICATE_NOT                                    \
  ((type_bridge_query_predicate_combine_t)3)

typedef uint32_t type_bridge_query_sort_direction_t;
#define TYPE_BRIDGE_QUERY_SORT_ASCENDING                                   \
  ((type_bridge_query_sort_direction_t)1)
#define TYPE_BRIDGE_QUERY_SORT_DESCENDING                                  \
  ((type_bridge_query_sort_direction_t)2)

typedef uint32_t type_bridge_query_missing_order_t;
#define TYPE_BRIDGE_QUERY_MISSING_REJECT ((type_bridge_query_missing_order_t)1)
#define TYPE_BRIDGE_QUERY_MISSING_FIRST ((type_bridge_query_missing_order_t)2)
#define TYPE_BRIDGE_QUERY_MISSING_LAST ((type_bridge_query_missing_order_t)3)

typedef uint32_t type_bridge_query_selection_kind_t;
#define TYPE_BRIDGE_QUERY_SELECTION_ONE                                    \
  ((type_bridge_query_selection_kind_t)1)
#define TYPE_BRIDGE_QUERY_SELECTION_COLLECT                                \
  ((type_bridge_query_selection_kind_t)2)

typedef uint32_t type_bridge_query_shape_kind_t;
#define TYPE_BRIDGE_QUERY_SHAPE_POSITIONAL ((type_bridge_query_shape_kind_t)1)
#define TYPE_BRIDGE_QUERY_SHAPE_NAMED ((type_bridge_query_shape_kind_t)2)

typedef uint32_t type_bridge_query_terminal_kind_t;
#define TYPE_BRIDGE_QUERY_TERMINAL_ROWS ((type_bridge_query_terminal_kind_t)1)
#define TYPE_BRIDGE_QUERY_TERMINAL_PAGE ((type_bridge_query_terminal_kind_t)2)
#define TYPE_BRIDGE_QUERY_TERMINAL_COUNT ((type_bridge_query_terminal_kind_t)3)
#define TYPE_BRIDGE_QUERY_TERMINAL_EXISTS ((type_bridge_query_terminal_kind_t)4)
#define TYPE_BRIDGE_QUERY_TERMINAL_REDUCE ((type_bridge_query_terminal_kind_t)5)
#define TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELD                            \
  ((type_bridge_query_terminal_kind_t)6)
#define TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELDS                           \
  ((type_bridge_query_terminal_kind_t)7)
#define TYPE_BRIDGE_QUERY_TERMINAL_FIRST ((type_bridge_query_terminal_kind_t)8)

typedef uint32_t type_bridge_query_row_cardinality_t;
#define TYPE_BRIDGE_QUERY_ROWS_EXACTLY_ONE                                 \
  ((type_bridge_query_row_cardinality_t)1)
#define TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY                                \
  ((type_bridge_query_row_cardinality_t)2)

typedef uint32_t type_bridge_query_reducer_kind_t;
#define TYPE_BRIDGE_QUERY_REDUCER_COUNT ((type_bridge_query_reducer_kind_t)1)
#define TYPE_BRIDGE_QUERY_REDUCER_SUM ((type_bridge_query_reducer_kind_t)2)
#define TYPE_BRIDGE_QUERY_REDUCER_MIN ((type_bridge_query_reducer_kind_t)3)
#define TYPE_BRIDGE_QUERY_REDUCER_MAX ((type_bridge_query_reducer_kind_t)4)
#define TYPE_BRIDGE_QUERY_REDUCER_MEAN ((type_bridge_query_reducer_kind_t)5)
#define TYPE_BRIDGE_QUERY_REDUCER_MEDIAN ((type_bridge_query_reducer_kind_t)6)
#define TYPE_BRIDGE_QUERY_REDUCER_STD ((type_bridge_query_reducer_kind_t)7)

typedef uint32_t type_bridge_query_result_kind_t;
#define TYPE_BRIDGE_QUERY_RESULT_ROWS ((type_bridge_query_result_kind_t)1)
#define TYPE_BRIDGE_QUERY_RESULT_PAGE ((type_bridge_query_result_kind_t)2)
#define TYPE_BRIDGE_QUERY_RESULT_COUNT ((type_bridge_query_result_kind_t)3)
#define TYPE_BRIDGE_QUERY_RESULT_REDUCTION ((type_bridge_query_result_kind_t)4)
#define TYPE_BRIDGE_QUERY_RESULT_FIELD_REDUCTION                            \
  ((type_bridge_query_result_kind_t)5)
#define TYPE_BRIDGE_QUERY_RESULT_FIELD_TUPLE_REDUCTION                      \
  ((type_bridge_query_result_kind_t)6)
#define TYPE_BRIDGE_QUERY_RESULT_EXISTS ((type_bridge_query_result_kind_t)7)

typedef uint32_t type_bridge_query_reduction_group_kind_t;
#define TYPE_BRIDGE_QUERY_REDUCTION_GROUP_NONE                              \
  ((type_bridge_query_reduction_group_kind_t)0)
#define TYPE_BRIDGE_QUERY_REDUCTION_GROUP_THING                             \
  ((type_bridge_query_reduction_group_kind_t)1)
#define TYPE_BRIDGE_QUERY_REDUCTION_GROUP_FIELD                             \
  ((type_bridge_query_reduction_group_kind_t)2)
#define TYPE_BRIDGE_QUERY_REDUCTION_GROUP_FIELDS                            \
  ((type_bridge_query_reduction_group_kind_t)3)

typedef uint32_t type_bridge_query_reduced_value_kind_t;
#define TYPE_BRIDGE_QUERY_REDUCED_COUNT                                     \
  ((type_bridge_query_reduced_value_kind_t)1)
#define TYPE_BRIDGE_QUERY_REDUCED_LONG                                      \
  ((type_bridge_query_reduced_value_kind_t)2)
#define TYPE_BRIDGE_QUERY_REDUCED_DOUBLE                                    \
  ((type_bridge_query_reduced_value_kind_t)3)

typedef uint32_t type_bridge_query_function_argument_kind_t;
#define TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_BINDING                         \
  ((type_bridge_query_function_argument_kind_t)1)
#define TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_VALUE                           \
  ((type_bridge_query_function_argument_kind_t)2)
#define TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_CALL                            \
  ((type_bridge_query_function_argument_kind_t)3)

/*
 * Every generated nominal argument witness is ABI-layout-identical to this
 * common descriptor. Binding, projected scalar, and prior scalar-call operands
 * are checked by retained session and actual value domain; they do not claim
 * originating-function provenance.
 */
typedef struct type_bridge_query_function_argument_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_query_function_argument_kind_t kind;
  uint32_t reserved0;
  const type_bridge_query_binding_t *binding;
  const type_bridge_query_function_value_t *value;
  const type_bridge_query_function_call_t *call;
  uint64_t reserved[4];
} type_bridge_query_function_argument_v1_t;

/*
 * `members` is a static table in projected FunctionSignature parameter order.
 * Each strictly increasing, non-overlapping offset identifies one by-value
 * common-layout nominal witness within `args`; native code byte-reads those
 * witnesses directly, so generated wrappers need no schema-sized local array.
 */
typedef struct type_bridge_query_function_argument_member_v1 {
  uint32_t struct_size;
  uint32_t version;
  size_t args_offset;
  uint64_t reserved[4];
} type_bridge_query_function_argument_member_v1_t;

/* Common prefix of every generated per-function arguments struct. */
typedef struct type_bridge_query_function_arguments_header_v1 {
  uint32_t struct_size;
  uint32_t version;
  uint64_t reserved[4];
} type_bridge_query_function_arguments_header_v1_t;

typedef struct type_bridge_query_function_arguments_graph_v1 {
  uint32_t struct_size;
  uint32_t version;
  const void *args;
  size_t args_size;
  const type_bridge_query_function_argument_member_v1_t *members;
  size_t member_count;
  uint64_t reserved[3];
} type_bridge_query_function_arguments_graph_v1_t;

/*
 * Graphs through TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_MAX members are fully
 * traversed and alias-fenced. A larger member table or hosted object exceeds
 * the safe traversal ceiling and returns RESOURCE_LIMIT read-only: output
 * slots and diagnostics are not initialized, so unwalked input bytes cannot
 * be corrupted through a hostile output alias.
 */

typedef struct type_bridge_query_order_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_query_field_t *field;
  const type_bridge_projected_token_v1_t *expected_field;
  type_bridge_query_sort_direction_t direction;
  type_bridge_query_missing_order_t missing;
  uint32_t reserved0;
  uint64_t reserved[4];
} type_bridge_query_order_descriptor_v1_t;

typedef struct type_bridge_query_selection_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_query_binding_t *binding;
  const type_bridge_projected_token_v1_t *expected_model;
  type_bridge_query_match_mode_t expected_mode;
  type_bridge_query_selection_kind_t kind;
  uint8_t distinct;
  uint8_t reserved0[7];
  const type_bridge_query_order_t *const *orders;
  size_t order_count;
  uint64_t reserved[4];
} type_bridge_query_selection_descriptor_v1_t;

typedef struct type_bridge_query_shape_slot_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_query_selection_t *selection;
  const type_bridge_projected_token_v1_t *expected_model;
  type_bridge_query_match_mode_t expected_mode;
  type_bridge_query_selection_kind_t expected_kind;
  uint32_t reserved0;
  type_bridge_byte_view_t name;
  uint64_t reserved[4];
} type_bridge_query_shape_slot_v1_t;

typedef struct type_bridge_query_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_query_shape_kind_t shape_kind;
  uint32_t reserved0;
  const type_bridge_query_shape_slot_v1_t *slots;
  size_t slot_count;
  uint64_t reserved[4];
} type_bridge_query_descriptor_v1_t;

typedef struct type_bridge_query_reducer_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_query_reducer_kind_t kind;
  uint32_t reserved0;
  const type_bridge_query_field_t *input;
  const type_bridge_projected_token_v1_t *expected_field;
  uint64_t reserved[4];
} type_bridge_query_reducer_v1_t;

typedef struct type_bridge_query_field_reference_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_query_field_t *field;
  const type_bridge_projected_token_v1_t *expected_field;
  uint64_t reserved[4];
} type_bridge_query_field_reference_v1_t;

typedef struct type_bridge_query_terminal_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_query_terminal_kind_t kind;
  type_bridge_query_row_cardinality_t cardinality;
  const type_bridge_query_binding_t *root;
  const type_bridge_projected_token_v1_t *expected_root_model;
  type_bridge_query_match_mode_t expected_root_mode;
  uint32_t reserved1;
  const type_bridge_query_order_t *const *orders;
  size_t order_count;
  uint64_t offset;
  uint64_t limit;
  uint8_t include_total;
  uint8_t reserved0[7];
  const type_bridge_query_binding_t *group_binding;
  const type_bridge_projected_token_v1_t *expected_group_model;
  type_bridge_query_match_mode_t expected_group_mode;
  uint32_t reserved2;
  const type_bridge_query_field_reference_v1_t *group_fields;
  size_t group_field_count;
  const type_bridge_query_reducer_v1_t *reducers;
  size_t reducer_count;
  uint64_t reserved[4];
} type_bridge_query_terminal_descriptor_v1_t;

typedef struct type_bridge_query_execution_limits_v1 {
  uint32_t struct_size;
  uint32_t version;
  uint64_t timeout_milliseconds;
  uint64_t items;
  uint64_t bytes;
  uint64_t graph_nodes;
  uint64_t attribute_values;
  uint64_t collection_members;
  uint64_t role_players;
  uint32_t statements;
  uint32_t reserved0;
  uint64_t reserved[4];
} type_bridge_query_execution_limits_v1_t;

/*
 * NULL execution limits select these canonical hard defaults. A non-NULL
 * descriptor is componentwise clamped to these hard maxima and every zero
 * field is an actual zero ceiling, never a request to substitute a default.
 * Direct execution reports the shared statement-limit diagnostic before its
 * first provider statement. Remote preparation preserves zero through the
 * authenticated request when the executor advertises that capability.
 */
#define TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT                         \
  {                                                                            \
    (uint32_t)sizeof(type_bridge_query_execution_limits_v1_t),                 \
        TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_VERSION,                            \
        TYPE_BRIDGE_QUERY_DEFAULT_TIMEOUT_MILLISECONDS,                        \
        TYPE_BRIDGE_QUERY_DEFAULT_ITEMS,                                       \
        TYPE_BRIDGE_QUERY_DEFAULT_BYTES,                                       \
        TYPE_BRIDGE_QUERY_DEFAULT_GRAPH_NODES,                                 \
        TYPE_BRIDGE_QUERY_DEFAULT_ATTRIBUTE_VALUES,                            \
        TYPE_BRIDGE_QUERY_DEFAULT_COLLECTION_MEMBERS,                          \
        TYPE_BRIDGE_QUERY_DEFAULT_ROLE_PLAYERS,                                \
        TYPE_BRIDGE_QUERY_DEFAULT_STATEMENTS, 0u, {0u, 0u, 0u, 0u}            \
  }

typedef struct type_bridge_query_page_metadata_v1 {
  uint32_t struct_size;
  uint32_t version;
  uint64_t offset;
  uint64_t limit;
  uint8_t has_total;
  uint8_t reserved0[7];
  uint64_t total;
  uint64_t reserved[4];
} type_bridge_query_page_metadata_v1_t;

typedef struct type_bridge_query_reduced_value_metadata_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_query_reduced_value_kind_t kind;
  uint8_t present;
  uint8_t reserved0[3];
  uint64_t reserved[4];
} type_bridge_query_reduced_value_metadata_v1_t;

typedef struct type_bridge_runtime_config_v1 {
  uint32_t struct_size;
  uint32_t version;
  uint32_t worker_threads;
  uint32_t reserved0;
  uint64_t reserved[4];
} type_bridge_runtime_config_v1_t;

/*
 * All byte views are copied during database_open_v1. Address, database, and
 * username are non-empty; password may be empty. `address` is exactly one
 * canonical credential-free host:port or [IPv6]:port endpoint. Version 1
 * rejects comma-delimited endpoint lists until a per-endpoint HTTP probe
 * policy is defined. `http_port` is the authoritative TypeDB HTTP
 * version-probe port. The C ABI never accepts a caller-claimed server version
 * and admits only a detected exact 3.12.1. database_open_v1 also requires the
 * verified package's exact typedb-3.12.1/v1 semantic profile; other verified
 * profiles return TYPE_BRIDGE_STATUS_UNSUPPORTED before provider I/O.
 */
typedef struct type_bridge_database_config_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_byte_view_t address;
  type_bridge_byte_view_t database;
  type_bridge_byte_view_t username;
  type_bridge_byte_view_t password;
  uint32_t http_port;
  uint32_t tls_mode;
  uint64_t reserved[4];
} type_bridge_database_config_v1_t;

typedef int32_t type_bridge_projected_value_kind_t;
#define TYPE_BRIDGE_PROJECTED_VALUE_STRING                                    \
  ((type_bridge_projected_value_kind_t)1)
#define TYPE_BRIDGE_PROJECTED_VALUE_LONG                                      \
  ((type_bridge_projected_value_kind_t)2)
#define TYPE_BRIDGE_PROJECTED_VALUE_DOUBLE                                    \
  ((type_bridge_projected_value_kind_t)3)
#define TYPE_BRIDGE_PROJECTED_VALUE_BOOLEAN                                   \
  ((type_bridge_projected_value_kind_t)4)
#define TYPE_BRIDGE_PROJECTED_VALUE_DATE                                      \
  ((type_bridge_projected_value_kind_t)5)
#define TYPE_BRIDGE_PROJECTED_VALUE_DATETIME                                  \
  ((type_bridge_projected_value_kind_t)6)
#define TYPE_BRIDGE_PROJECTED_VALUE_DATETIME_TZ                               \
  ((type_bridge_projected_value_kind_t)7)
#define TYPE_BRIDGE_PROJECTED_VALUE_DECIMAL                                   \
  ((type_bridge_projected_value_kind_t)8)
#define TYPE_BRIDGE_PROJECTED_VALUE_DURATION                                  \
  ((type_bridge_projected_value_kind_t)9)

#define TYPE_BRIDGE_PROJECTED_MODEL_INPUT_VERSION 1u
#define TYPE_BRIDGE_PROJECTED_COLLECTION_LEN_MAX 65536u
#define TYPE_BRIDGE_GENERATED_ALIAS_PREFLIGHT_VERSION 1u

typedef uint32_t type_bridge_generated_opaque_input_kind_t;
#define TYPE_BRIDGE_GENERATED_INPUT_BYTES                                     \
  ((type_bridge_generated_opaque_input_kind_t)1)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN                           \
  ((type_bridge_generated_opaque_input_kind_t)2)
#define TYPE_BRIDGE_GENERATED_INPUT_SCHEMA_PACKAGE                            \
  ((type_bridge_generated_opaque_input_kind_t)3)
#define TYPE_BRIDGE_GENERATED_INPUT_RUNTIME                                   \
  ((type_bridge_generated_opaque_input_kind_t)4)
#define TYPE_BRIDGE_GENERATED_INPUT_DATABASE                                  \
  ((type_bridge_generated_opaque_input_kind_t)5)
#define TYPE_BRIDGE_GENERATED_INPUT_READ_TRANSACTION                          \
  ((type_bridge_generated_opaque_input_kind_t)6)
#define TYPE_BRIDGE_GENERATED_INPUT_WRITE_TRANSACTION                         \
  ((type_bridge_generated_opaque_input_kind_t)7)
#define TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION                              \
  ((type_bridge_generated_opaque_input_kind_t)8)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_VALUE                           \
  ((type_bridge_generated_opaque_input_kind_t)9)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_REFERENCE                       \
  ((type_bridge_generated_opaque_input_kind_t)10)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE                          \
  ((type_bridge_generated_opaque_input_kind_t)11)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_THING                           \
  ((type_bridge_generated_opaque_input_kind_t)12)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_VALUE_POINTER_ARRAY             \
  ((type_bridge_generated_opaque_input_kind_t)13)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_REFERENCE_POINTER_ARRAY         \
  ((type_bridge_generated_opaque_input_kind_t)14)
#define TYPE_BRIDGE_GENERATED_INPUT_DIAGNOSTICS                               \
  ((type_bridge_generated_opaque_input_kind_t)15)
#define TYPE_BRIDGE_GENERATED_INPUT_EXECUTION_DIAGNOSTICS                     \
  ((type_bridge_generated_opaque_input_kind_t)16)
#define TYPE_BRIDGE_GENERATED_INPUT_CREATE_ARGS_GRAPH                         \
  ((type_bridge_generated_opaque_input_kind_t)17)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_SESSION                             \
  ((type_bridge_generated_opaque_input_kind_t)18)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_BINDING                             \
  ((type_bridge_generated_opaque_input_kind_t)19)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_FIELD                               \
  ((type_bridge_generated_opaque_input_kind_t)20)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_ROLE                                \
  ((type_bridge_generated_opaque_input_kind_t)21)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_PREDICATE                           \
  ((type_bridge_generated_opaque_input_kind_t)22)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_ORDER                               \
  ((type_bridge_generated_opaque_input_kind_t)23)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_SELECTION                           \
  ((type_bridge_generated_opaque_input_kind_t)24)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY                                     \
  ((type_bridge_generated_opaque_input_kind_t)25)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_TERMINAL                            \
  ((type_bridge_generated_opaque_input_kind_t)26)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_RESULT                              \
  ((type_bridge_generated_opaque_input_kind_t)27)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_CONTEXT                      \
  ((type_bridge_generated_opaque_input_kind_t)28)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_PENDING                      \
  ((type_bridge_generated_opaque_input_kind_t)29)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_REMOTE_CLAIM                        \
  ((type_bridge_generated_opaque_input_kind_t)30)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION                            \
  ((type_bridge_generated_opaque_input_kind_t)31)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_VALUE                      \
  ((type_bridge_generated_opaque_input_kind_t)32)
#define TYPE_BRIDGE_GENERATED_INPUT_QUERY_FUNCTION_CALL                       \
  ((type_bridge_generated_opaque_input_kind_t)33)

#define TYPE_BRIDGE_GENERATED_CREATE_GRAPH_VERSION 1u
#define TYPE_BRIDGE_GENERATED_CREATE_MEMBER_COUNT_MAX 1020u
#define TYPE_BRIDGE_GENERATED_CREATE_HOSTED_OBJECT_BYTES_MAX 65535u

typedef uint32_t type_bridge_generated_create_member_kind_t;
#define TYPE_BRIDGE_GENERATED_CREATE_MEMBER_VALUE_SCALAR                      \
  ((type_bridge_generated_create_member_kind_t)1)
#define TYPE_BRIDGE_GENERATED_CREATE_MEMBER_VALUE_SEQUENCE                    \
  ((type_bridge_generated_create_member_kind_t)2)
#define TYPE_BRIDGE_GENERATED_CREATE_MEMBER_REFERENCE_SCALAR                  \
  ((type_bridge_generated_create_member_kind_t)3)
#define TYPE_BRIDGE_GENERATED_CREATE_MEMBER_REFERENCE_SEQUENCE                \
  ((type_bridge_generated_create_member_kind_t)4)

typedef struct type_bridge_generated_create_handle_chunk_v1
    type_bridge_generated_create_handle_chunk_v1_t;

struct type_bridge_generated_create_handle_chunk_v1 {
  size_t struct_size;
  uint32_t version;
  const void *const *values;
  size_t count;
  const type_bridge_generated_create_handle_chunk_v1_t *next;
  uint32_t reserved[4];
};

typedef struct type_bridge_generated_create_member_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_generated_create_member_kind_t kind;
  uint32_t reserved0;
  size_t args_offset;
  const type_bridge_projected_token_v1_t *token;
  uint64_t reserved[4];
} type_bridge_generated_create_member_v1_t;

typedef struct type_bridge_generated_create_args_graph_v1 {
  uint32_t struct_size;
  uint32_t version;
  const void *args;
  size_t args_size;
  const type_bridge_generated_create_member_v1_t *members;
  size_t member_count;
  uint64_t reserved[3];
} type_bridge_generated_create_args_graph_v1_t;

typedef struct type_bridge_generated_opaque_input_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_generated_opaque_input_kind_t kind;
  uint32_t reserved0;
  const void *pointer;
  size_t count;
  uint64_t reserved[4];
} type_bridge_generated_opaque_input_v1_t;

typedef struct type_bridge_generated_output_range_v1 {
  uint32_t struct_size;
  uint32_t version;
  void *pointer;
  size_t length;
  uint64_t reserved[4];
} type_bridge_generated_output_range_v1_t;

typedef struct type_bridge_projected_field_input_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_projected_token_v1_t *field;
  const type_bridge_projected_value_t *const *values;
  size_t value_count;
  uint64_t reserved[4];
} type_bridge_projected_field_input_v1_t;

typedef struct type_bridge_projected_role_input_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_projected_token_v1_t *role;
  const type_bridge_projected_reference_t *const *references;
  size_t reference_count;
  uint64_t reserved[4];
} type_bridge_projected_role_input_v1_t;

typedef struct type_bridge_projected_reference_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_projected_token_v1_t *model;
  type_bridge_byte_view_t iid;
  const type_bridge_projected_field_input_v1_t *keys;
  size_t key_count;
  uint64_t reserved[4];
} type_bridge_projected_reference_descriptor_v1_t;

typedef struct type_bridge_projected_create_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_projected_token_v1_t *model;
  const type_bridge_projected_field_input_v1_t *fields;
  size_t field_count;
  const type_bridge_projected_role_input_v1_t *roles;
  size_t role_count;
  uint64_t reserved[4];
} type_bridge_projected_create_descriptor_v1_t;

typedef struct type_bridge_projected_thing_descriptor_v1 {
  uint32_t struct_size;
  uint32_t version;
  const type_bridge_projected_token_v1_t *model;
  type_bridge_byte_view_t iid;
  const type_bridge_projected_field_input_v1_t *fields;
  size_t field_count;
  const type_bridge_projected_role_input_v1_t *roles;
  size_t role_count;
  uint64_t reserved[4];
} type_bridge_projected_thing_descriptor_v1_t;

typedef int32_t type_bridge_execution_diagnostic_category_t;
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT                         \
  ((type_bridge_execution_diagnostic_category_t)1)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_UNSUPPORTED_CAPABILITY                \
  ((type_bridge_execution_diagnostic_category_t)2)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT                        \
  ((type_bridge_execution_diagnostic_category_t)3)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY                             \
  ((type_bridge_execution_diagnostic_category_t)4)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PROVIDER                              \
  ((type_bridge_execution_diagnostic_category_t)5)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_TRANSACTION                           \
  ((type_bridge_execution_diagnostic_category_t)6)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_CANCELLED                             \
  ((type_bridge_execution_diagnostic_category_t)7)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTERNAL                              \
  ((type_bridge_execution_diagnostic_category_t)8)

typedef int32_t type_bridge_execution_diagnostic_path_kind_t;
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_ARGUMENT                         \
  ((type_bridge_execution_diagnostic_path_kind_t)1)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_INDEX                            \
  ((type_bridge_execution_diagnostic_path_kind_t)2)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_TYPE                             \
  ((type_bridge_execution_diagnostic_path_kind_t)3)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_FIELD                            \
  ((type_bridge_execution_diagnostic_path_kind_t)4)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_ROLE                             \
  ((type_bridge_execution_diagnostic_path_kind_t)5)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_REQUEST                    \
  ((type_bridge_execution_diagnostic_path_kind_t)6)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PLAN                       \
  ((type_bridge_execution_diagnostic_path_kind_t)7)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OPERATION                  \
  ((type_bridge_execution_diagnostic_path_kind_t)8)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PREDICATE                  \
  ((type_bridge_execution_diagnostic_path_kind_t)9)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT                     \
  ((type_bridge_execution_diagnostic_path_kind_t)10)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_PROVIDER_EVIDENCE          \
  ((type_bridge_execution_diagnostic_path_kind_t)11)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_RESULT                     \
  ((type_bridge_execution_diagnostic_path_kind_t)12)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_BINDING                    \
  ((type_bridge_execution_diagnostic_path_kind_t)13)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_FIELD                      \
  ((type_bridge_execution_diagnostic_path_kind_t)14)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_ROLE                       \
  ((type_bridge_execution_diagnostic_path_kind_t)15)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_ROLE_EDGE                  \
  ((type_bridge_execution_diagnostic_path_kind_t)16)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT_SLOT                \
  ((type_bridge_execution_diagnostic_path_kind_t)17)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_OUTPUT_NAME                \
  ((type_bridge_execution_diagnostic_path_kind_t)18)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_FIELD                   \
  ((type_bridge_execution_diagnostic_path_kind_t)19)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_IDENTITY                \
  ((type_bridge_execution_diagnostic_path_kind_t)20)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_UNKNOWN                          \
  ((type_bridge_execution_diagnostic_path_kind_t)255)

typedef int32_t type_bridge_execution_diagnostic_detail_kind_t;
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_BOOLEAN                        \
  ((type_bridge_execution_diagnostic_detail_kind_t)1)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COUNT                          \
  ((type_bridge_execution_diagnostic_detail_kind_t)2)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_BYTE_COUNT                     \
  ((type_bridge_execution_diagnostic_detail_kind_t)3)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_CAPABILITY                     \
  ((type_bridge_execution_diagnostic_detail_kind_t)4)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_VALUE_TYPE                     \
  ((type_bridge_execution_diagnostic_detail_kind_t)5)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TYPE                           \
  ((type_bridge_execution_diagnostic_detail_kind_t)6)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_FIELD                          \
  ((type_bridge_execution_diagnostic_detail_kind_t)7)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_ROLE                           \
  ((type_bridge_execution_diagnostic_detail_kind_t)8)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_FINGERPRINT                    \
  ((type_bridge_execution_diagnostic_detail_kind_t)9)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_PROVIDER_OPERATION             \
  ((type_bridge_execution_diagnostic_detail_kind_t)10)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COMMIT_OUTCOME                 \
  ((type_bridge_execution_diagnostic_detail_kind_t)11)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT                           \
  ((type_bridge_execution_diagnostic_detail_kind_t)12)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_SIGNED                         \
  ((type_bridge_execution_diagnostic_detail_kind_t)13)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT_LIST                      \
  ((type_bridge_execution_diagnostic_detail_kind_t)14)
#define TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_UNKNOWN                        \
  ((type_bridge_execution_diagnostic_detail_kind_t)255)

typedef struct type_bridge_execution_diagnostic_view_v1 {
  uint32_t struct_size;
  uint32_t version;
  type_bridge_execution_diagnostic_category_t category;
  uint32_t reserved0;
  type_bridge_byte_view_t code;
  type_bridge_byte_view_t message;
  size_t path_count;
  size_t detail_count;
  uint64_t reserved[4];
} type_bridge_execution_diagnostic_view_v1_t;

typedef struct type_bridge_execution_diagnostic_path_view_v1 {
  uint32_t struct_size;
  type_bridge_execution_diagnostic_path_kind_t kind;
  uint64_t index;
  type_bridge_byte_view_t primary;
  type_bridge_byte_view_t secondary;
  type_bridge_byte_view_t tertiary;
  uint64_t reserved[4];
} type_bridge_execution_diagnostic_path_view_v1_t;

typedef struct type_bridge_execution_diagnostic_detail_view_v1 {
  uint32_t struct_size;
  type_bridge_execution_diagnostic_detail_kind_t kind;
  uint8_t boolean_value;
  uint8_t reserved0[7];
  uint64_t unsigned_value;
  type_bridge_byte_view_t key;
  type_bridge_byte_view_t primary;
  type_bridge_byte_view_t secondary;
  type_bridge_byte_view_t tertiary;
  type_bridge_byte_view_t quaternary;
  uint64_t reserved[4];
} type_bridge_execution_diagnostic_detail_view_v1_t;

/*
 * Opaque handles are single-owner values. Do not copy an owned pointer into
 * multiple ownership slots, pass it to a different handle family's close
 * function, or race access with close. Borrowed byte views remain valid only
 * while their originating handle remains open.
 */

TYPE_BRIDGE_API uint32_t TYPE_BRIDGE_CALL type_bridge_c_abi_major(void);
TYPE_BRIDGE_API uint32_t TYPE_BRIDGE_CALL type_bridge_c_abi_minor(void);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_runtime_version(type_bridge_byte_view_t *out_version);

/*
 * Snapshot and verify a generated schema package. Every non-NULL output slot
 * is initialized before validation, including when the other slot is NULL.
 * The two output slot addresses must be distinct.
 * On success, *out_package is owned by the caller and *out_diagnostics is
 * NULL. On ordinary failure, *out_package is NULL and *out_diagnostics
 * contains canonical structured diagnostics.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_open_v1(
    const type_bridge_schema_package_descriptor_v1_t *descriptor,
    type_bridge_schema_package_t **out_package,
    type_bridge_diagnostics_t **out_diagnostics);
/*
 * Generated large-package entry point. The aggregate chunk count across all
 * seven resources is bounded by TYPE_BRIDGE_PROJECTED_COLLECTION_LEN_MAX.
 * Each flat chunk table is one hosted object of at most 65,535 bytes and every
 * chunk contains at most 32,768 bytes. With this ABI's 16-byte byte-view layout,
 * a valid table therefore contains at most 4,095 chunks; the aggregate ceiling
 * remains an overflow and defense-in-depth bound across all seven tables.
 * An aggregate excess returns RESOURCE_LIMIT read-only and leaves both outputs
 * untouched because bounded data/output disjointness cannot be established.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_open_chunked_v1(
    const type_bridge_schema_package_chunked_descriptor_v1_t *descriptor,
    type_bridge_schema_package_t **out_package,
    type_bridge_diagnostics_t **out_diagnostics);

/* Idempotent for a non-NULL slot whose current value is NULL. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_close(type_bridge_schema_package_t **package);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_authority_json(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_projection_json(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_semantic_fingerprint_json(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_binding_fingerprint_json(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_managed_scope(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_schema_package_semantic_profile(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t *out_value);

/*
 * Diagnostics are an opaque Rust-owned allocation. The JSON view is borrowed
 * until diagnostics_close and is canonical UTF-8 containing an ordered array
 * of Diagnostic objects (category, code, message, path, and details).
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_diagnostics_json(
    const type_bridge_diagnostics_t *diagnostics,
    type_bridge_byte_view_t *out_json);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_diagnostics_close(type_bridge_diagnostics_t **diagnostics);

/*
 * SDK execution diagnostics expose only stable typed, bounded, redacted
 * components. Every returned byte view is borrowed until the diagnostics
 * handle is closed. Details are ordered by their stable key.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_count(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t *out_count);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_get_v1(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t index,
    type_bridge_execution_diagnostic_view_v1_t *out_diagnostic);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_path_get_v1(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t diagnostic_index,
    size_t path_index,
    type_bridge_execution_diagnostic_path_view_v1_t *out_path);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_detail_get_v1(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t diagnostic_index,
    size_t detail_index,
    type_bridge_execution_diagnostic_detail_view_v1_t *out_detail);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_detail_signed(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t diagnostic_index,
    size_t detail_index,
    int64_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_detail_list_count(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t diagnostic_index,
    size_t detail_index,
    size_t *out_count);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_detail_list_get(
    const type_bridge_execution_diagnostics_t *diagnostics,
    size_t diagnostic_index,
    size_t detail_index,
    size_t list_index,
    type_bridge_byte_view_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_execution_diagnostics_close(
    type_bridge_execution_diagnostics_t **diagnostics);

/*
 * The synchronous runtime hides a Tokio multi-thread scheduler. A runtime
 * owns database children and a database owns transaction children. Parent
 * close returns TYPE_BRIDGE_STATUS_IN_USE without changing the owner pointer
 * while any child remains. A database retains its verified package evidence,
 * so the original schema-package handle may close after database_open_v1.
 * Child open/access/terminal calls must not race their parent close.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_runtime_open_v1(
    const type_bridge_runtime_config_v1_t *config,
    type_bridge_runtime_t **out_runtime,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_runtime_close(
    type_bridge_runtime_t **runtime,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/*
 * Cancellation is thread-safe and sticky. Every operation checks it before
 * provider dispatch. Typed-query execution additionally propagates it to the
 * provider's in-flight answer stream. CRUD operations remain pre-dispatch
 * only and never relabel a provider result that won the dispatch race.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_cancellation_open(type_bridge_cancellation_t **out_cancellation);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_cancellation_request(
    const type_bridge_cancellation_t *cancellation);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_cancellation_is_requested(
    const type_bridge_cancellation_t *cancellation,
    uint8_t *out_requested);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_cancellation_close(type_bridge_cancellation_t **cancellation);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_open_v1(
    const type_bridge_runtime_t *runtime,
    const type_bridge_schema_package_t *package,
    const type_bridge_database_config_v1_t *config,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_database_t **out_database,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_server_version(
    const type_bridge_database_t *database,
    type_bridge_byte_view_t *out_version);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_close(
    type_bridge_database_t **database,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/*
 * Transaction opens borrow the exact package authority retained by the
 * database; the original package handle may already be closed. Read and write
 * handles are distinct C types. Model operations will borrow these handles;
 * only the terminal functions below consume them. A commit cancelled before
 * dispatch leaves the write handle live. Once commit dispatch begins, success
 * or the classified provider outcome consumes the handle. Active write close
 * rolls back; read close closes without commit. Every operation on one mutable
 * transaction handle, including its terminal, requires exclusive serialized
 * caller access. If a borrowed provider operation panics, the ABI contains the
 * panic and poisons that C transaction handle. A poisoned handle rejects all
 * later CRUD and write commit before provider I/O; read close and write
 * rollback/close remain permitted and consume the handle normally.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_open(
    const type_bridge_database_t *database,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_read_transaction_t **out_transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_close(
    type_bridge_read_transaction_t **transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_open(
    const type_bridge_database_t *database,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_write_transaction_t **out_transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_commit(
    type_bridge_write_transaction_t **transaction,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_rollback(
    type_bridge_write_transaction_t **transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_close(
    type_bridge_write_transaction_t **transaction,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/*
 * Immutable typed queries accept generated tokens only and keep all plans
 * opaque. TYPE_BRIDGE_QUERY_TERMINAL_FIRST is an explicit bounded-many
 * fetch with offset zero and limit one; it returns a rows result containing
 * zero or one row. TYPE_BRIDGE_QUERY_TERMINAL_ROWS with cardinality
 * TYPE_BRIDGE_QUERY_ROWS_EXACTLY_ONE is the distinct exactly-one terminal.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_session_open(const type_bridge_schema_package_t *package,
    type_bridge_query_session_t **out_session,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_session_close(type_bridge_query_session_t **session);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_binding_open_v1(const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_query_match_mode_t mode, type_bridge_query_binding_t **out_binding,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_binding_close(type_bridge_query_binding_t **binding);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_field_open(const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *field,
    type_bridge_query_field_t **out_field,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_field_close(type_bridge_query_field_t **field);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_role_open(const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *role,
    type_bridge_query_role_t **out_role,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_role_close(type_bridge_query_role_t **role);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_open(const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *function,
    type_bridge_query_function_t **out_function,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_close(type_bridge_query_function_t **function);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_value_open(
    const type_bridge_query_session_t *session,
    const type_bridge_projected_value_t *value,
    type_bridge_query_function_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_value_close(
    type_bridge_query_function_value_t **value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_open_v1(
    const type_bridge_query_function_t *function,
    const type_bridge_projected_token_v1_t *expected_function,
    const type_bridge_query_function_arguments_graph_v1_t *arguments,
    type_bridge_query_function_call_t **out_call,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_close(type_bridge_query_function_call_t **call);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_compare_field(
    const type_bridge_query_function_call_t *call,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_compare_value(
    const type_bridge_query_function_call_t *call,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_function_value_t *value,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_function_call_compare_call(
    const type_bridge_query_function_call_t *call,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_function_call_t *other,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_field_compare_function(
    const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_function_call_t *call,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_binding_iid_v1(const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode, type_bridge_byte_view_t iid,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_binding_iid_in_v1(const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode,
    const type_bridge_byte_view_t *iids, size_t iid_count,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_field_compare_value(const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_query_comparison_t comparison,
    const type_bridge_projected_value_t *value,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_field_compare_field(const type_bridge_query_field_t *left,
    const type_bridge_projected_token_v1_t *expected_left,
    type_bridge_query_comparison_t comparison,
    const type_bridge_query_field_t *right,
    const type_bridge_projected_token_v1_t *expected_right,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_field_presence(const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field, uint8_t present,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_role_connects(const type_bridge_query_role_t *role,
    const type_bridge_projected_token_v1_t *expected_role,
    const type_bridge_query_binding_t *player,
    const type_bridge_projected_token_v1_t *expected_player_model,
    type_bridge_query_match_mode_t expected_player_mode,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_session_reachable(const type_bridge_query_session_t *session,
    const type_bridge_projected_token_v1_t *relation,
    const type_bridge_projected_token_v1_t *role_from,
    const type_bridge_projected_token_v1_t *role_to,
    const type_bridge_query_binding_t *source,
    const type_bridge_projected_token_v1_t *expected_source_model,
    type_bridge_query_match_mode_t expected_source_mode,
    const type_bridge_query_binding_t *target,
    const type_bridge_projected_token_v1_t *expected_target_model,
    type_bridge_query_match_mode_t expected_target_mode, uint8_t min_depth,
    uint8_t max_depth, type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_predicate_combine(type_bridge_query_predicate_combine_t operation,
    const type_bridge_query_predicate_t *left,
    const type_bridge_query_predicate_t *right,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_predicate_close(type_bridge_query_predicate_t **predicate);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_order_open_v1(
    const type_bridge_query_order_descriptor_v1_t *descriptor,
    type_bridge_query_order_t **out_order,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_order_close(type_bridge_query_order_t **order);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_selection_open_v1(
    const type_bridge_query_selection_descriptor_v1_t *descriptor,
    type_bridge_query_selection_t **out_selection,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_selection_close(type_bridge_query_selection_t **selection);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_open_v1(const type_bridge_query_session_t *session,
    const type_bridge_query_descriptor_v1_t *descriptor,
    type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_add_hidden(const type_bridge_query_t *query,
    const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode, type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_where(const type_bridge_query_t *query,
    const type_bridge_query_predicate_t *predicate,
    type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_allow_cross_join(const type_bridge_query_t *query,
    const type_bridge_query_binding_t *left,
    const type_bridge_projected_token_v1_t *expected_left_model,
    type_bridge_query_match_mode_t expected_left_mode,
    const type_bridge_query_binding_t *right,
    const type_bridge_projected_token_v1_t *expected_right_model,
    type_bridge_query_match_mode_t expected_right_mode,
    type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_close(type_bridge_query_t **query);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_terminal_open_v1(const type_bridge_query_t *query,
    const type_bridge_query_terminal_descriptor_v1_t *descriptor,
    type_bridge_query_terminal_t **out_terminal,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_terminal_close(type_bridge_query_terminal_t **terminal);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_query_execute_v1(const type_bridge_database_t *database,
    const type_bridge_query_terminal_t *terminal,
    type_bridge_query_terminal_kind_t expected_terminal_kind,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_query_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_query_execute_v1(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_query_terminal_t *terminal,
    type_bridge_query_terminal_kind_t expected_terminal_kind,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_query_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/*
 * Caller-transport remote execution performs no hidden network operation.
 * Context open copies and authenticates one capability advertisement against
 * the exact retained generated package. Prepare mints a fresh invocation and
 * owns exact request bytes until pending close. The request view is borrowed
 * from the pending handle. The pending handle reports the maximum response
 * snapshot before caller-owned transport begins. Claim is atomic and one-shot
 * while both pending and claim handles remain explicitly closeable. Decode
 * snapshots at most that reported limit, consumes the claim on every semantic
 * outcome after successful FFI preflight, and returns the same owned result
 * family as direct execution. Preflight and boundary-allocation failures leave
 * the claim recoverable. Cancellation covers local preparation/decode only;
 * caller-owned transport cancellation remains the caller's responsibility.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_context_open_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t advertisement,
    const type_bridge_query_execution_limits_v1_t *limits,
    type_bridge_query_remote_context_t **out_context,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_context_close(
    type_bridge_query_remote_context_t **context);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_prepare_v1(
    const type_bridge_query_remote_context_t *context,
    const type_bridge_query_terminal_t *terminal,
    type_bridge_query_terminal_kind_t expected_terminal_kind,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_query_remote_pending_t **out_pending,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_pending_request_bytes(
    const type_bridge_query_remote_pending_t *pending,
    type_bridge_byte_view_t *out_request);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_pending_claim(
    const type_bridge_query_remote_pending_t *pending,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_query_remote_claim_t **out_claim,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_pending_close(
    type_bridge_query_remote_pending_t **pending);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_pending_response_snapshot_limit(
    const type_bridge_query_remote_pending_t *pending, size_t *out_limit);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_claim_decode_v1(
    const type_bridge_query_remote_claim_t *claim,
    type_bridge_query_terminal_kind_t expected_terminal_kind,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_byte_view_t response, type_bridge_query_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_remote_claim_close(
    type_bridge_query_remote_claim_t **claim);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_kind(const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t *out_kind);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_row_count(const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_kind, size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_row_slot_count(const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t slot_index,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode,
    type_bridge_query_selection_kind_t expected_kind, size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_row_slot_thing_at(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t slot_index, size_t thing_index,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode,
    type_bridge_query_selection_kind_t expected_kind,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_page_metadata_v1(
    const type_bridge_query_result_t *result,
    type_bridge_query_page_metadata_v1_t *out_metadata,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_count(const type_bridge_query_result_t *result,
    uint64_t *out_count, type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_exists(const type_bridge_query_result_t *result,
    uint8_t *out_exists, type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_row_count(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_group_kind(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    type_bridge_query_reduction_group_kind_t *out_kind,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_group_thing(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_group_field_count(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t *out_count, type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_group_field_at(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t field_index,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_value_metadata_v1(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t value_index,
    type_bridge_query_reduced_value_kind_t expected_value_kind,
    type_bridge_query_reduced_value_metadata_v1_t *out_metadata,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_value_count(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t value_index, uint64_t *out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_value_long(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t value_index, int64_t *out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_reduction_value_double_bits(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t value_index, uint64_t *out_bits,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_query_result_close(type_bridge_query_result_t **result);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_model_ordinal(
    const type_bridge_projected_thing_t *thing, uint32_t *out_ordinal,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_clone(const type_bridge_projected_thing_t *thing,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/*
 * Generated-only exact entity CRUD SPI. Model arguments are generated model
 * tokens; mutation payloads are immutable projected-create handles. IID views
 * are copied during the call and must use canonical TypeDB `0x` identity text.
 * No function accepts a schema label or JSON model payload.
 * Insert, put, and update require the create model to equal the fixed generated
 * model token and reject an explicit cross-nominal cast before provider I/O.
 *
 * Database operations own their transaction. Successful database writes
 * rehydrate before one classified commit. Borrowed transaction operations
 * never commit, roll back, close, or consume the caller's handle. A successful
 * get may return NULL to represent absence. Delete is one blind exact mutation
 * and absence is success. Cancellation is checked only before dispatch; once
 * dispatch begins, the provider's actual result wins.
 * A contained provider panic poisons a borrowed transaction as described by
 * the transaction lifecycle contract above.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_insert(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_put(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_get_by_iid(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_update(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_delete_by_iid(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_entity_count(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_entity_get_by_iid(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_entity_count(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_insert(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_put(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_get_by_iid(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_update(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_delete_by_iid(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_entity_count(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);

/*
 * Generated-only exact relation CRUD SPI. Ownership, cancellation, nullable
 * get, blind-delete, count, panic containment, and classified-commit behavior
 * match the entity SPI above. Insert, put, and update require an exact relation
 * model token and an exact projected-create model. Every closed role-player
 * reference is revalidated for role domain/cardinality and package identity.
 * A known database origin is checked before a database-owned write opens its
 * transaction; borrowed operations check against the already-open transaction
 * and never terminally consume it. Relation references may be role players.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_insert(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_put(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_get_by_iid(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_update(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_delete_by_iid(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_database_relation_count(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_relation_get_by_iid(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_read_transaction_relation_count(
    const type_bridge_read_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_insert(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_put(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_get_by_iid(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_update(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_delete_by_iid(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_byte_view_t iid,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_write_transaction_relation_count(
    const type_bridge_write_transaction_t *transaction,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_cancellation_t *cancellation,
    uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);


/*
 * Projected scalars are immutable and branded by an exact verified schema
 * package plus generated attribute-model token. Text inputs are copied during
 * the call. Double input/output uses exact IEEE-754 bits and Boolean input is
 * exactly zero or one.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_string_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    type_bridge_byte_view_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_long_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    int64_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_double_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    uint64_t input_bits,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_boolean_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    uint8_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_date_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    type_bridge_byte_view_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_datetime_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    type_bridge_byte_view_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_datetime_tz_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    type_bridge_byte_view_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_decimal_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    type_bridge_byte_view_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_duration_open(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *attribute_token,
    type_bridge_byte_view_t input,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
/* Generated-only, read-only exact nominal attribute-model fence. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_validate_model(
    const type_bridge_projected_value_t *value,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_kind(
    const type_bridge_projected_value_t *value,
    type_bridge_projected_value_kind_t *out_kind);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_text(
    const type_bridge_projected_value_t *value,
    type_bridge_byte_view_t *out_text);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_long(
    const type_bridge_projected_value_t *value,
    int64_t *out_long);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_double_bits(
    const type_bridge_projected_value_t *value,
    uint64_t *out_bits);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_boolean(
    const type_bridge_projected_value_t *value,
    uint8_t *out_boolean);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_value_close(type_bridge_projected_value_t **value);

/*
 * Model constructors snapshot every descriptor, nested descriptor, pointer
 * array, IID, and immutable handle value during the call. Returned handles
 * retain immutable package state and remain valid after the original package
 * handle closes.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_collection_limit_diagnostics(
    type_bridge_execution_diagnostics_t **out_diagnostics);
/*
 * Read-only generated-wrapper alias preflight. `input_count` is bounded by
 * TYPE_BRIDGE_PROJECTED_COLLECTION_LEN_MAX and `output_count` is one or two.
 * A zero input count ignores `inputs`; nonzero requires non-NULL.
 * Object kinds require count=1. Pointer-array kinds validate both the array
 * storage and every non-NULL pointee using the runtime's exact opaque sizes.
 * CREATE_ARGS_GRAPH also requires count=1 and describes one versioned argument
 * object plus a versioned member table. Scalar member slots may be NULL to mean
 * absent. Sequence member slots are linked lists of versioned, nonempty chunks;
 * every sequence element must be non-NULL and every list must terminate within
 * the frozen hosted-object and traversal ceilings.
 *
 * Direct object, descriptor, table, chunk, pointer-array, and opaque-handle
 * ranges are fenced against every output before opaque objects are read. The
 * graph path then globally charges one model, every member, and every handle's
 * cached projected resource measure before it inspects nested borrowed ranges.
 * This is a binding-neutral amplification and alias-safety budget; the
 * streaming builder remains the exact authority for projection-specific base
 * byte costs and semantic create validity. An over-budget graph returns
 * RESOURCE_LIMIT before nested-range inspection. This exceptional safety
 * result intentionally carries no diagnostics. Every return path leaves all
 * described input and output bytes untouched.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_generated_opaque_alias_preflight_v1(
    const type_bridge_generated_opaque_input_v1_t *inputs,
    size_t input_count,
    const type_bridge_generated_output_range_v1_t *outputs,
    size_t output_count);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_reference_descriptor_v1_t *descriptor,
    type_bridge_projected_reference_t **out_reference,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_create_descriptor_v1_t *descriptor,
    type_bridge_projected_create_t **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics);
/*
 * Generated-only streaming create construction. Add copies but never consumes
 * input handles. A field token requires only the value lane; a role token
 * requires only the reference lane. Each selected lane is bounded by
 * TYPE_BRIDGE_PROJECTED_CREATE_BUILDER_CHUNK_LEN_MAX and must be nonempty.
 * A chunk above that ceiling returns RESOURCE_LIMIT before inspecting either
 * array and leaves `out_diagnostics` untouched, so even an unbounded hostile
 * array cannot force an unsafe alias walk. Finish consumes and clears the
 * builder on every post-preflight outcome.
 */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_projected_create_builder_t **out_builder,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_add_v1(
    type_bridge_projected_create_builder_t *builder,
    const type_bridge_projected_token_v1_t *member,
    const type_bridge_projected_value_t *const *values,
    size_t value_count,
    const type_bridge_projected_reference_t *const *references,
    size_t reference_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_finish(
    type_bridge_projected_create_builder_t **builder,
    type_bridge_projected_create_t **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_builder_close(
    type_bridge_projected_create_builder_t **builder);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_thing_descriptor_v1_t *descriptor,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_iid(
    const type_bridge_projected_reference_t *reference,
    type_bridge_byte_view_t *out_iid);
/* Generated-only, read-only exact nominal-model fence. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_validate_model(
    const type_bridge_projected_reference_t *reference,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_validate_role(
    const type_bridge_projected_reference_t *reference,
    const type_bridge_projected_token_v1_t *role,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_clone(
    const type_bridge_projected_reference_t *reference,
    type_bridge_projected_reference_t **out_reference,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_model_ordinal(
    const type_bridge_projected_reference_t *reference,
    uint32_t *out_ordinal,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_key(
    const type_bridge_projected_reference_t *reference,
    const type_bridge_projected_token_v1_t *field,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_field_count(
    const type_bridge_projected_create_t *create,
    const type_bridge_projected_token_v1_t *field,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_field_value_at(
    const type_bridge_projected_create_t *create,
    const type_bridge_projected_token_v1_t *field,
    size_t index,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_role_count(
    const type_bridge_projected_create_t *create,
    const type_bridge_projected_token_v1_t *role,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_role_reference_at(
    const type_bridge_projected_create_t *create,
    const type_bridge_projected_token_v1_t *role,
    size_t index,
    type_bridge_projected_reference_t **out_reference,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_iid(
    const type_bridge_projected_thing_t *thing,
    type_bridge_byte_view_t *out_iid);
/* Generated-only, read-only exact nominal-model fence. */
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_validate_model(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_reference(
    const type_bridge_projected_thing_t *thing,
    type_bridge_projected_reference_t **out_reference,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_field_count(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *field,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_field_value_at(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *field,
    size_t index,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_scalar_field_value(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *field,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_scalar_role_reference(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *role,
    type_bridge_projected_reference_t **out_reference,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_role_count(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *role,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_role_reference_at(
    const type_bridge_projected_thing_t *thing,
    const type_bridge_projected_token_v1_t *role,
    size_t index,
    type_bridge_projected_reference_t **out_reference,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_reference_close(
    type_bridge_projected_reference_t **reference);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_create_close(type_bridge_projected_create_t **create);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_thing_close(type_bridge_projected_thing_t **thing);

#ifdef __cplusplus
}
#endif

#endif /* TYPEBRIDGE_TYPE_BRIDGE_H */
