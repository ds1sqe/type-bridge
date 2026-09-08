#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <tb_generatedcv5/models.h>
#include <tb_generatedcv5foreign/models.h>

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

static int write_text_file(const char *path, const char *text) {
  type_bridge_byte_view_t view = {(const unsigned char *)text, strlen(text)};
  return write_file(path, view);
}

static int equal_text(type_bridge_byte_view_t view, const char *expected) {
  size_t length = strlen(expected);
  return view.length == length && memcmp(view.data, expected, length) == 0;
}

static int expect_diagnostic(
    type_bridge_status_t status, type_bridge_status_t expected_status,
    type_bridge_execution_diagnostics_t **diagnostics,
    type_bridge_execution_diagnostic_category_t expected_category,
    const char *expected_code, int require_schema_path) {
  size_t count = 0;
  type_bridge_execution_diagnostic_view_v1_t item = {0};
  type_bridge_execution_diagnostic_path_view_v1_t path_item = {0};
  int ok;
  if (status != expected_status || diagnostics == NULL || *diagnostics == NULL ||
      type_bridge_execution_diagnostics_count(*diagnostics, &count) !=
          TYPE_BRIDGE_STATUS_OK ||
      count != 1u ||
      type_bridge_execution_diagnostics_get_v1(*diagnostics, 0u, &item) !=
          TYPE_BRIDGE_STATUS_OK)
    return 0;
  ok = item.category == expected_category && equal_text(item.code, expected_code);
  if (require_schema_path) {
    ok = ok && item.path_count == 1u && item.detail_count == 0u &&
         type_bridge_execution_diagnostics_path_get_v1(
             *diagnostics, 0u, 0u, &path_item) == TYPE_BRIDGE_STATUS_OK &&
         path_item.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_CONTRACT_FIELD &&
         equal_text(path_item.primary, "declared_schema_identity");
  }
  if (type_bridge_execution_diagnostics_close(diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      *diagnostics != NULL)
    return 0;
  return ok;
}

static type_bridge_projected_codec_options_v1_t options(void) {
  type_bridge_projected_codec_options_v1_t value = {0};
  value.struct_size = sizeof(value);
  value.version = TYPE_BRIDGE_PROJECTED_CODEC_OPTIONS_V1;
  value.max_input_bytes = 32u * 1024u * 1024u;
  value.max_output_bytes = 32u * 1024u * 1024u;
  value.max_depth = 64u;
  value.max_records = 4096u;
  value.max_members = 65536u;
  return value;
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
  type_bridge_schema_package_t *foreign_package = NULL;
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
  type_bridge_cancellation_t *cancellation = NULL;
  type_bridge_projected_codec_options_v1_t controlled;
  type_bridge_canonical_archive_builder_t *failed_builder = NULL;
  type_bridge_canonical_bytes_t *failed_bytes = NULL;
  type_bridge_canonical_archive_t *failed_archive = NULL;

  if (argc != 4) return 2;
  if (tb_generatedcv5_schema_package_open(&package, &package_diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      tb_generatedcv5foreign_schema_package_open(&foreign_package,
                                                 &package_diagnostics) !=
          TYPE_BRIDGE_STATUS_OK)
    return 3;
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

  controlled = options();
  if (type_bridge_cancellation_open(&cancellation) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_cancellation_request(cancellation) != TYPE_BRIDGE_STATUS_OK)
    return 50;
  controlled.cancellation = cancellation;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_builder_open_v1(
              package, &controlled, &failed_builder, &diagnostics),
          TYPE_BRIDGE_STATUS_CANCELLED, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_CANCELLED,
          "projected_codec_cancelled", 0) ||
      failed_builder != NULL || failed_bytes != NULL)
    return 51;
  if (type_bridge_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK)
    return 52;

  controlled = options();
  controlled.flags = TYPE_BRIDGE_PROJECTED_CODEC_HAS_TIMEOUT;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_open_v1(
              package, archive_view, &controlled, &failed_archive, &diagnostics),
          TYPE_BRIDGE_STATUS_RESOURCE_LIMIT, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
          "projected_codec_deadline_exceeded", 0) ||
      failed_archive != NULL)
    return 53;

  controlled = options();
  controlled.max_input_bytes = archive_view.length - 1u;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_open_v1(
              package, archive_view, &controlled, &failed_archive, &diagnostics),
          TYPE_BRIDGE_STATUS_RESOURCE_LIMIT, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
          "projected_codec_input_limit", 0) ||
      failed_archive != NULL)
    return 54;

  controlled = options();
  controlled.max_output_bytes = archive_view.length - 1u;
  if (type_bridge_canonical_archive_builder_open_v1(
          package, &controlled, &failed_builder, &diagnostics) !=
      TYPE_BRIDGE_STATUS_OK)
    return 55;
  for (index = 0; index < RECORD_COUNT; ++index)
    if (type_bridge_canonical_archive_builder_append_record_v1(
            failed_builder, record_views[index], &diagnostics) !=
        TYPE_BRIDGE_STATUS_OK)
      return 56;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_builder_finish_v1(
              &failed_builder, &failed_bytes, &diagnostics),
          TYPE_BRIDGE_STATUS_RESOURCE_LIMIT, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
          "projected_codec_output_limit", 0) ||
      failed_builder == NULL || failed_bytes != NULL ||
      type_bridge_canonical_archive_builder_close(&failed_builder) !=
          TYPE_BRIDGE_STATUS_OK)
    return 57;

  controlled = options();
  controlled.max_records = RECORD_COUNT - 1u;
  if (type_bridge_canonical_archive_builder_open_v1(
          package, &controlled, &failed_builder, &diagnostics) !=
      TYPE_BRIDGE_STATUS_OK)
    return 58;
  for (index = 0; index < RECORD_COUNT - 1u; ++index)
    if (type_bridge_canonical_archive_builder_append_record_v1(
            failed_builder, record_views[index], &diagnostics) !=
        TYPE_BRIDGE_STATUS_OK)
      return 59;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_builder_append_record_v1(
              failed_builder, record_views[RECORD_COUNT - 1u], &diagnostics),
          TYPE_BRIDGE_STATUS_RESOURCE_LIMIT, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
          "projected_codec_member_limit", 0) ||
      type_bridge_canonical_archive_builder_close(&failed_builder) !=
          TYPE_BRIDGE_STATUS_OK)
    return 60;

  controlled = options();
  controlled.max_depth = 1u;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_open_v1(
              package, archive_view, &controlled, &failed_archive, &diagnostics),
          TYPE_BRIDGE_STATUS_RESOURCE_LIMIT, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
          "projected_codec_depth_limit", 0) ||
      failed_archive != NULL)
    return 61;

  if (type_bridge_canonical_archive_builder_open_v1(
          foreign_package, NULL, &failed_builder, &diagnostics) !=
      TYPE_BRIDGE_STATUS_OK)
    return 62;
  if (!expect_diagnostic(
          type_bridge_canonical_archive_builder_append_record_v1(
              failed_builder, record_views[0], &diagnostics),
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT, &diagnostics,
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
          "projected_record_schema_mismatch", 1) ||
      type_bridge_canonical_archive_builder_close(&failed_builder) !=
          TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_archive_builder_close(&failed_builder) !=
          TYPE_BRIDGE_STATUS_OK)
    return 63;

  if (!write_text_file(
          argv[3],
          "{\"binding\":\"c\",\"cancellation\":{\"code\":\"projected_codec_cancelled\",\"partial_output\":false},\"deadline\":{\"code\":\"projected_codec_deadline_exceeded\",\"partial_output\":false},\"diagnostic\":{\"category\":\"invalid_input\",\"code\":\"projected_record_schema_mismatch\",\"path\":[\"declared_schema_identity\"],\"payload_absent\":true},\"format\":\"typebridge.sdk-v5-operational-evidence/v1\",\"lifecycle\":{\"archive_closed\":true,\"builder_closed\":true,\"bytes_closed\":true,\"decoded_closed\":true,\"repeat_close\":true,\"sibling_usable\":true},\"resource_limits\":{\"depth_code\":\"projected_codec_depth_limit\",\"input_code\":\"projected_codec_input_limit\",\"member_code\":\"projected_codec_member_limit\",\"output_code\":\"projected_codec_output_limit\",\"partial_output\":false},\"test_id\":\"c.generated_sdk_v5_canonical_codec\"}"))
    return 64;

  if (type_bridge_canonical_archive_close(&verified) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_archive_close(&verified) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_bytes_close(&archive) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_canonical_bytes_close(&archive) != TYPE_BRIDGE_STATUS_OK)
    return 65;
  for (index = 0; index < RECORD_COUNT; ++index) {
    if (type_bridge_canonical_bytes_close(&records[index]) !=
            TYPE_BRIDGE_STATUS_OK ||
        type_bridge_canonical_bytes_close(&records[index]) !=
            TYPE_BRIDGE_STATUS_OK)
      return 66;
    free(inputs[index]);
  }
  type_bridge_schema_package_close(&package);
  type_bridge_schema_package_close(&foreign_package);
  return 0;
}
