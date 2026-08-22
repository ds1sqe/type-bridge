#pragma once

#include <typebridge/type_bridge_abi_1_5.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TYPE_BRIDGE_PROJECTED_CODEC_OPTIONS_V1 1u
#define TYPE_BRIDGE_PROJECTED_CODEC_HAS_TIMEOUT 1u

#define TYPE_BRIDGE_GENERATED_INPUT_CANONICAL_BYTES                         \
  ((type_bridge_generated_opaque_input_kind_t)37u)
#define TYPE_BRIDGE_GENERATED_INPUT_CANONICAL_ARCHIVE_BUILDER               \
  ((type_bridge_generated_opaque_input_kind_t)38u)
#define TYPE_BRIDGE_GENERATED_INPUT_CANONICAL_ARCHIVE                       \
  ((type_bridge_generated_opaque_input_kind_t)39u)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_STRUCT                        \
  ((type_bridge_generated_opaque_input_kind_t)40u)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_STRUCT_MEMBER                 \
  ((type_bridge_generated_opaque_input_kind_t)41u)
#define TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CODEC_OPTIONS                 \
  ((type_bridge_generated_opaque_input_kind_t)42u)

typedef struct type_bridge_canonical_bytes type_bridge_canonical_bytes_t;
typedef struct type_bridge_canonical_archive_builder
    type_bridge_canonical_archive_builder_t;
typedef struct type_bridge_canonical_archive type_bridge_canonical_archive_t;
typedef struct type_bridge_projected_struct type_bridge_projected_struct_t;
typedef struct type_bridge_projected_struct_member
    type_bridge_projected_struct_member_t;

typedef struct type_bridge_projected_codec_options_v1 {
  uint64_t struct_size;
  uint32_t version;
  uint32_t flags;
  uint64_t timeout_milliseconds;
  uint64_t max_input_bytes;
  uint64_t max_output_bytes;
  uint64_t max_depth;
  uint64_t max_records;
  uint64_t max_members;
  const type_bridge_cancellation_t *cancellation;
} type_bridge_projected_codec_options_v1_t;

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_attribute_v1(
    const type_bridge_projected_value_t *value,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_create_v1(
    const type_bridge_projected_create_t *value,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_reference_v1(
    const type_bridge_projected_reference_t *value,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_snapshot_v1(
    const type_bridge_projected_thing_t *value,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_encode_struct_v1(
    const type_bridge_projected_struct_t *value,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_canonical_bytes_t **out_bytes,
    type_bridge_execution_diagnostics_t **out_diagnostics);

TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_attribute_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_attribute,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_projected_value_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_create_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_model,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_projected_create_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_reference_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_model,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_projected_reference_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_snapshot_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_model,
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_projected_thing_t **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_record_decode_struct_v1(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t bytes,
    const type_bridge_projected_token_v1_t *expected_struct,
    const type_bridge_projected_codec_options_v1_t *options,
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
    const type_bridge_projected_codec_options_v1_t *options,
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
    const type_bridge_projected_codec_options_v1_t *options,
    type_bridge_canonical_archive_t **out_archive,
    type_bridge_execution_diagnostics_t **out_diagnostics);
TYPE_BRIDGE_API type_bridge_status_t TYPE_BRIDGE_CALL
type_bridge_canonical_archive_count(
    const type_bridge_canonical_archive_t *archive,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics);
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
