#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <tb_generatedcv5/models.h>

#define RECORD_COUNT 9u

static int read_file(const char *path, unsigned char **out, size_t *out_len) {
  FILE *stream = fopen(path, "rb");
  long length;
  unsigned char *bytes;
  if (stream == NULL || fseek(stream, 0, SEEK_END) != 0 ||
      (length = ftell(stream)) <= 0 || fseek(stream, 0, SEEK_SET) != 0) {
    if (stream != NULL) fclose(stream);
    return 0;
  }
  bytes = (unsigned char *)malloc((size_t)length);
  if (bytes == NULL || fread(bytes, 1u, (size_t)length, stream) != (size_t)length ||
      fclose(stream) != 0) {
    free(bytes);
    return 0;
  }
  *out = bytes;
  *out_len = (size_t)length;
  return 1;
}

static int write_file(const char *path, type_bridge_byte_view_t view) {
  FILE *stream = fopen(path, "wbx");
  int ok;
  if (stream == NULL) return 0;
  ok = fwrite(view.data, 1u, view.length, stream) == view.length &&
       fflush(stream) == 0 && fclose(stream) == 0;
  return ok;
}

static int equal_view(type_bridge_byte_view_t left, type_bridge_byte_view_t right) {
  return left.length == right.length &&
         memcmp(left.data, right.data, left.length) == 0;
}

#define ROUND_TRIP(INDEX, TYPE, DECODE, ENCODE, CLOSE)                         \
  do {                                                                         \
    TYPE *value = NULL;                                                        \
    type_bridge_byte_view_t input_view = {inputs[INDEX], input_lengths[INDEX]};\
    if (DECODE(package, input_view, &value, &diagnostics) !=                   \
            TYPE_BRIDGE_STATUS_OK ||                                           \
        ENCODE(value, &records[INDEX], &diagnostics) != TYPE_BRIDGE_STATUS_OK ||\
        CLOSE(&value) != TYPE_BRIDGE_STATUS_OK ||                              \
        type_bridge_canonical_bytes_view(records[INDEX], &record_views[INDEX]) !=\
            TYPE_BRIDGE_STATUS_OK ||                                           \
        !equal_view(input_view, record_views[INDEX])) {                        \
      return 20 + (int)(INDEX);                                                \
    }                                                                          \
  } while (0)

int main(int argc, char **argv) {
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_canonical_archive_builder_t *builder = NULL;
  type_bridge_canonical_archive_t *verified = NULL;
  type_bridge_canonical_bytes_t *records[RECORD_COUNT] = {0};
  type_bridge_byte_view_t record_views[RECORD_COUNT] = {{0}};
  unsigned char *inputs[RECORD_COUNT] = {0};
  size_t input_lengths[RECORD_COUNT] = {0};
  type_bridge_canonical_bytes_t *archive = NULL;
  type_bridge_byte_view_t archive_view = {0};
  char path[4096];
  size_t index;
  size_t count = 0;

  if (argc != 3) return 2;
  if (tb_generatedcv5_schema_package_open(&package, &package_diagnostics) !=
      TYPE_BRIDGE_STATUS_OK) return 3;
  for (index = 0; index < RECORD_COUNT; ++index) {
    if (snprintf(path, sizeof(path), "%s/record-%zu.bin", argv[1], index) < 0 ||
        !read_file(path, &inputs[index], &input_lengths[index])) return 4;
  }

  ROUND_TRIP(0, tb_generatedcv5_robotzuid,
             tb_generatedcv5_robotzuid_canonical_decode,
             tb_generatedcv5_robotzuid_canonical_encode,
             tb_generatedcv5_robotzuid_close);
  ROUND_TRIP(1, tb_generatedcv5_playerzhstats,
             tb_generatedcv5_playerzhstats_canonical_decode,
             tb_generatedcv5_playerzhstats_canonical_encode,
             tb_generatedcv5_playerzhstats_canonical_close);
  ROUND_TRIP(2, tb_generatedcv5_person_create,
             tb_generatedcv5_person_create_canonical_decode,
             tb_generatedcv5_person_create_canonical_encode,
             tb_generatedcv5_person_create_close);
  ROUND_TRIP(3, tb_generatedcv5_person,
             tb_generatedcv5_person_canonical_decode,
             tb_generatedcv5_person_canonical_encode,
             tb_generatedcv5_person_close);
  ROUND_TRIP(4, tb_generatedcv5_membership_create,
             tb_generatedcv5_membership_create_canonical_decode,
             tb_generatedcv5_membership_create_canonical_encode,
             tb_generatedcv5_membership_create_close);
  ROUND_TRIP(5, tb_generatedcv5_interaction_create,
             tb_generatedcv5_interaction_create_canonical_decode,
             tb_generatedcv5_interaction_create_canonical_encode,
             tb_generatedcv5_interaction_create_close);
  ROUND_TRIP(6, tb_generatedcv5_container_create,
             tb_generatedcv5_container_create_canonical_decode,
             tb_generatedcv5_container_create_canonical_encode,
             tb_generatedcv5_container_create_close);
  ROUND_TRIP(7, tb_generatedcv5_employment,
             tb_generatedcv5_employment_canonical_decode,
             tb_generatedcv5_employment_canonical_encode,
             tb_generatedcv5_employment_close);
  ROUND_TRIP(8, tb_generatedcv5_event_ref,
             tb_generatedcv5_event_ref_canonical_decode,
             tb_generatedcv5_event_ref_canonical_encode,
             tb_generatedcv5_event_ref_close);

  if (type_bridge_canonical_archive_builder_open_v1(
          package, NULL, &builder, &diagnostics) != TYPE_BRIDGE_STATUS_OK)
    return 40;
  for (index = 0; index < RECORD_COUNT; ++index) {
    if (type_bridge_canonical_archive_builder_append_record_v1(
            builder, record_views[index], &diagnostics) != TYPE_BRIDGE_STATUS_OK)
      return 41;
  }
  if (type_bridge_canonical_archive_builder_finish_v1(
          &builder, &archive, &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_bytes_view(archive, &archive_view) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_archive_open_v1(package, archive_view, NULL, &verified,
                                            &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_archive_count(verified, &count, &diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      count != RECORD_COUNT)
    return 42;

  for (index = 0; index < RECORD_COUNT; ++index) {
    type_bridge_canonical_bytes_t *recovered = NULL;
    type_bridge_byte_view_t recovered_view = {0};
    if (type_bridge_canonical_archive_record_at(verified, index, &recovered,
                                                &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
        type_bridge_canonical_bytes_view(recovered, &recovered_view) !=
            TYPE_BRIDGE_STATUS_OK ||
        !equal_view(recovered_view, record_views[index]))
      return 43;
    type_bridge_canonical_bytes_close(&recovered);
    snprintf(path, sizeof(path), "%s/record-%zu.bin", argv[2], index);
    if (!write_file(path, record_views[index])) return 44;
  }
  snprintf(path, sizeof(path), "%s/archive.bin", argv[2]);
  if (!write_file(path, archive_view)) return 45;

  type_bridge_canonical_archive_close(&verified);
  type_bridge_canonical_bytes_close(&archive);
  for (index = 0; index < RECORD_COUNT; ++index) {
    type_bridge_canonical_bytes_close(&records[index]);
    free(inputs[index]);
  }
  type_bridge_schema_package_close(&package);
  return 0;
}
