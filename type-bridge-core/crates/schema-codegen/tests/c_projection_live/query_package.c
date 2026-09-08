#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Compile the byte-exact generated translation unit once as C. Keeping this
 * shim in the same translation unit also lets the acceptance consumer exercise
 * the flat ABI-1.6 admission form from the exact embedded chunk tables. */
#include "src/models.c"

static int
query_flatten_resource(const type_bridge_chunked_byte_view_v1_t *resource,
                        uint8_t **out_storage,
                        type_bridge_byte_view_t *out_view) {
  size_t index;
  size_t offset = 0u;
  uint8_t *storage = NULL;

  if (resource == NULL || out_storage == NULL || out_view == NULL) {
    return 0;
  }
  if (resource->total_length != 0u) {
    storage = (uint8_t *)malloc(resource->total_length);
    if (storage == NULL) {
      return 0;
    }
  }
  for (index = 0u; index < resource->chunk_count; ++index) {
    const type_bridge_byte_view_t chunk = resource->chunks[index];
    if (chunk.length > resource->total_length - offset ||
        (chunk.length != 0u && chunk.data == NULL)) {
      free(storage);
      return 0;
    }
    if (chunk.length != 0u) {
      memcpy(storage + offset, chunk.data, chunk.length);
    }
    offset += chunk.length;
  }
  if (offset != resource->total_length) {
    free(storage);
    return 0;
  }
  *out_storage = storage;
  out_view->data = storage;
  out_view->length = resource->total_length;
  return 1;
}

type_bridge_status_t TYPE_BRIDGE_CALL query_schema_package_open_flat_v2(
    type_bridge_schema_package_t **out_package,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  const type_bridge_schema_package_chunked_descriptor_v1_t *chunks =
      &fixture_schema_package_chunks_v1;
  const type_bridge_chunked_byte_view_v1_t *resources[7];
  uint8_t *storage[7] = {NULL, NULL, NULL, NULL, NULL, NULL, NULL};
  type_bridge_byte_view_t views[7];
  type_bridge_schema_package_descriptor_v1_t descriptor;
  type_bridge_status_t status = TYPE_BRIDGE_STATUS_RESOURCE_LIMIT;
  size_t index;

  resources[0] = &chunks->schema_authority_json;
  resources[1] = &chunks->declared_schema_json;
  resources[2] = &chunks->runtime_projection_json;
  resources[3] = &chunks->semantic_fingerprint_json;
  resources[4] = &chunks->binding_fingerprint_json;
  resources[5] = &chunks->managed_scope;
  resources[6] = &chunks->semantic_profile;
  memset(views, 0, sizeof(views));
  for (index = 0u; index < 7u; ++index) {
    if (!query_flatten_resource(resources[index], &storage[index],
                                 &views[index])) {
      goto cleanup;
    }
  }

  memset(&descriptor, 0, sizeof(descriptor));
  descriptor.struct_size = sizeof(descriptor);
  descriptor.abi_major = chunks->abi_major;
  descriptor.abi_minor = chunks->abi_minor;
  descriptor.schema_authority_json = views[0];
  descriptor.declared_schema_json = views[1];
  descriptor.runtime_projection_json = views[2];
  descriptor.semantic_fingerprint_json = views[3];
  descriptor.binding_fingerprint_json = views[4];
  descriptor.managed_scope = views[5];
  descriptor.semantic_profile = views[6];
  status = type_bridge_schema_package_open_v2(&descriptor, out_package,
                                              out_diagnostics);

cleanup:
  for (index = 0u; index < 7u; ++index) {
    free(storage[index]);
  }
  return status;
}
