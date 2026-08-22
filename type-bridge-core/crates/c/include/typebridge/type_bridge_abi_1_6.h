#pragma once

#include <typebridge/type_bridge_abi_1_5.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct type_bridge_canonical_bytes type_bridge_canonical_bytes_t;
typedef struct type_bridge_canonical_archive_builder
    type_bridge_canonical_archive_builder_t;
typedef struct type_bridge_canonical_archive type_bridge_canonical_archive_t;
typedef struct type_bridge_projected_struct type_bridge_projected_struct_t;
typedef struct type_bridge_projected_struct_member
    type_bridge_projected_struct_member_t;

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_attribute_v1(
    const type_bridge_projected_value_t *value,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_create_v1(
    const type_bridge_projected_create_t *value,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_reference_v1(
    const type_bridge_projected_reference_t *value,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_snapshot_v1(
    const type_bridge_projected_thing_t *value,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_struct_v1(
    const type_bridge_projected_struct_t *value,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_attribute_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_attribute,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_create_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_create_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_reference_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_reference_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_snapshot_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_thing_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_struct_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_struct,
    type_bridge_projected_struct_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_close(type_bridge_projected_struct_t **value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_at_v1(
    const type_bridge_projected_struct_t *value,
    const type_bridge_projected_token_v1_t *expected_struct,
    size_t index,
    type_bridge_projected_struct_member_t **out_member,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_kind(
    const type_bridge_projected_struct_member_t *value,
    type_bridge_projected_value_kind_t *out_kind);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_text(
    const type_bridge_projected_struct_member_t *value,
    type_bridge_byte_view_t *out_text);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_long(
    const type_bridge_projected_struct_member_t *value,
    int64_t *out_value);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_double_bits(
    const type_bridge_projected_struct_member_t *value,
    uint64_t *out_bits);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_boolean(
    const type_bridge_projected_struct_member_t *value,
    uint8_t *out_boolean);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_projected_struct_member_close(
    type_bridge_projected_struct_member_t **value);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_builder_open_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_canonical_archive_builder_t **out_builder,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_builder_append_record_v1(
    type_bridge_canonical_archive_builder_t *builder,
    type_bridge_byte_view_t record,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_builder_finish_v1(
    type_bridge_canonical_archive_builder_t **builder,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_builder_close(
    type_bridge_canonical_archive_builder_t **builder);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_open_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    type_bridge_canonical_archive_t **out_archive,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_count(
    const type_bridge_canonical_archive_t *archive,
    size_t *out_count);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_record_at(
    const type_bridge_canonical_archive_t *archive,
    size_t index,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_close(type_bridge_canonical_archive_t **archive);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_bytes_view(
    const type_bridge_canonical_bytes_t *bytes,
    type_bridge_byte_view_t *out_view);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_bytes_close(type_bridge_canonical_bytes_t **bytes);

#ifdef __cplusplus
}
#endif
