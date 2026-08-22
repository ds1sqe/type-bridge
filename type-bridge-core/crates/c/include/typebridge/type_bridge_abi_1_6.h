#pragma once

#include <typebridge/type_bridge_abi_1_5.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct type_bridge_canonical_bytes type_bridge_canonical_bytes_t;
typedef struct type_bridge_canonical_archive_builder
    type_bridge_canonical_archive_builder_t;
typedef struct type_bridge_canonical_archive type_bridge_canonical_archive_t;

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
