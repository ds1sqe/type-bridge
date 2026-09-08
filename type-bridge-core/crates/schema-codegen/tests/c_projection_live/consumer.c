#define _POSIX_C_SOURCE 200809L

#include <arpa/inet.h>
#include <errno.h>
#include <inttypes.h>
#include <netinet/in.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#include <typebridge/type_bridge.h>
#include <fixture/models.h>

#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "generated C entity check failed at line %d\n",         \
              __LINE__);                                                       \
      return __LINE__;                                                         \
    }                                                                          \
  } while (0)

static type_bridge_byte_view_t view_of(const char *text) {
  type_bridge_byte_view_t view;
  view.data = (const uint8_t *)text;
  view.length = strlen(text);
  return view;
}

static int same_text(type_bridge_byte_view_t actual, const char *expected) {
  const size_t expected_length = strlen(expected);
  return actual.length == expected_length && actual.data != NULL &&
         memcmp(actual.data, expected, expected_length) == 0;
}

static int copy_stable_text(type_bridge_byte_view_t value, char *output,
                            size_t capacity) {
  size_t index;
  if (value.data == NULL || value.length == 0u || value.length >= capacity) {
    return 0;
  }
  for (index = 0u; index < value.length; ++index) {
    const uint8_t byte = value.data[index];
    if (byte < 0x20u || byte > 0x7eu || byte == '"' || byte == '\\') {
      return 0;
    }
  }
  memcpy(output, value.data, value.length);
  output[value.length] = '\0';
  return 1;
}

static int emit_v2_observation(const char *observation_ref,
                               const char *proof_kind,
                               const char *canonical_json) {
  const char *cursor;
  if (observation_ref == NULL || proof_kind == NULL || canonical_json == NULL ||
      canonical_json[0] != '{') {
    return 0;
  }
  for (cursor = canonical_json; *cursor != '\0'; ++cursor) {
    if (*cursor == '\t' || *cursor == '\r' || *cursor == '\n') {
      return 0;
    }
  }
  return printf("TYPE_BRIDGE_C_V2_FACT\t%s\t%s\t%s\n", observation_ref,
                proof_kind, canonical_json) > 0;
}

static int emit_v2_observationf(const char *observation_ref,
                                const char *proof_kind,
                                const char *format, ...) {
  char canonical_json[4096];
  int length;
  va_list arguments;
  va_start(arguments, format);
  length = vsnprintf(canonical_json, sizeof(canonical_json), format, arguments);
  va_end(arguments);
  if (length < 0 || (size_t)length >= sizeof(canonical_json)) {
    return 0;
  }
  return emit_v2_observation(observation_ref, proof_kind, canonical_json);
}

typedef struct sdk_person_rows_observation {
  size_t count;
  char keys[4][64];
  int64_t scores[4];
  char first_alias[64];
  char first_nickname[64];
  char first_reference_key[64];
  uint32_t first_scalar_domain_mask;
} sdk_person_rows_observation_t;

#define SDK_SCALAR_DOMAIN_MASK_ALL UINT32_C(0x1ff)

typedef struct sdk_reducer_observation {
  uint64_t count;
  int64_t sum;
  int64_t minimum;
  int64_t maximum;
  uint64_t mean_bits;
  uint64_t median_bits;
  uint64_t standard_deviation_bits;
  char binding_keys[2][64];
  uint64_t binding_counts[2];
  int64_t field_keys[2];
  uint64_t field_counts[2];
  int64_t tuple_keys[2][2];
  uint64_t tuple_counts[2];
} sdk_reducer_observation_t;

typedef struct sdk_not_unique_observation {
  type_bridge_execution_diagnostic_category_t category;
  type_bridge_execution_diagnostic_path_kind_t path_kind;
  type_bridge_execution_diagnostic_detail_kind_t detail_kind;
  uint64_t actual;
  int redacted;
  char code[64];
  char message[256];
  char detail_key[64];
} sdk_not_unique_observation_t;

typedef struct sdk_topology_observation {
  size_t cross_pair_count;
  char cross_pairs[4][2][64];
  char reachable_from[64];
  char reachable_to[64];
  size_t max_hops;
} sdk_topology_observation_t;

typedef struct sdk_selection_shape_observation {
  char named_origin[64];
  char named_participants[2][64];
  char positional_origin[64];
  char positional_participants[2][64];
  size_t named_participant_count;
  size_t positional_participant_count;
  int collected_distinct;
  type_bridge_query_sort_direction_t collection_order;
} sdk_selection_shape_observation_t;

typedef struct sdk_query_lifecycle_observation {
  int direct_lane_observed;
  int remote_lane_observed;
  int ancestor_usable_after_descendant_close;
  int close_idempotent;
  int descendant_usable_after_ancestor_close;
  int handle_invalidated;
  size_t post_close_io_count;
  int post_close_rejected;
  int session_usable_after_query_close;
  int sibling_usable;
  int result_usable_after_query_close;
} sdk_query_lifecycle_observation_t;

typedef struct sdk_resource_limit_observation {
  type_bridge_execution_diagnostic_category_t category;
  size_t zero_dimensions;
  size_t plus_one_dimensions;
  int no_partial_result;
  char code[64];
  char dimension[64];
} sdk_resource_limit_observation_t;

static int emit_model_values_observation(
    const char *proof_kind,
    const sdk_person_rows_observation_t *observation) {
  if (observation == NULL || observation->count != 2u) {
    return 0;
  }
  if (observation->first_alias[0] == '\0' ||
      observation->first_nickname[0] == '\0' ||
      observation->first_reference_key[0] == '\0' ||
      observation->first_scalar_domain_mask !=
          SDK_SCALAR_DOMAIN_MASK_ALL) {
    return 0;
  }
  return emit_v2_observationf(
      "model_values_and_references", proof_kind,
      "{\"aliases\":[\"%s\"],\"key\":\"%s\",\"model\":\"person\","
      "\"nickname\":\"%s\",\"reference\":{\"key\":\"%s\",\"model\":"
      "\"person\"},\"scalar_domains\":[\"boolean\",\"date\",\"datetime\","
      "\"datetime_tz\",\"decimal\",\"double\",\"duration\",\"long\","
      "\"string\"]}",
      observation->first_alias, observation->keys[0],
      observation->first_nickname, observation->first_reference_key);
}

static int emit_owner_iid_observation(
    const char *proof_kind,
    const sdk_person_rows_observation_t *owners,
    const sdk_person_rows_observation_t *optional) {
  if (owners == NULL || owners->count != 2u || optional == NULL ||
      optional->count != 1u) {
    return 0;
  }
  return emit_v2_observationf(
      "owner_iid_set", proof_kind,
      "{\"iid_set_keys\":[\"%s\",\"%s\"],\"optional_field\":\"nickname\","
      "\"optional_present_keys\":[\"%s\"],\"owner_field\":\"score\","
      "\"owner_keys\":[\"%s\",\"%s\"]}",
      owners->keys[0], owners->keys[1], optional->keys[0], owners->keys[0],
      owners->keys[1]);
}

static int emit_scalar_domain_observation(
    const char *proof_kind,
    const sdk_person_rows_observation_t *observation,
    type_bridge_query_comparison_t comparison, int64_t operand) {
  if (observation == NULL || observation->count != 1u ||
      comparison != TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL) {
    return 0;
  }
  return emit_v2_observationf(
      "scalar_domain", proof_kind,
      "{\"domain\":\"long\",\"keys\":[\"%s\"],\"operand\":%" PRId64
      ",\"operator\":\"gte\"}",
      observation->keys[0], operand);
}

static int emit_schema_function_observation(
    const char *proof_kind, int64_t minimum,
    const sdk_person_rows_observation_t *outer,
    const sdk_person_rows_observation_t *nested) {
  if (outer == NULL || outer->count != 2u || nested == NULL ||
      nested->count != 2u) {
    return 0;
  }
  return emit_v2_observationf(
      "schema_function", proof_kind,
      "{\"minimum\":%" PRId64 ",\"nested_values\":[%" PRId64 ",%" PRId64
      "],\"values\":[%" PRId64 ",%" PRId64 "]}",
      minimum, nested->scores[0], nested->scores[1], outer->scores[0],
      outer->scores[1]);
}

static int emit_grouped_reducer_observation(
    const char *proof_kind, const sdk_reducer_observation_t *value) {
  if (value == NULL) {
    return 0;
  }
  return emit_v2_observationf(
      "grouped_reducer", proof_kind,
      "{\"groups\":{\"binding\":[{\"count\":%" PRIu64
      ",\"key\":\"%s\",\"model\":\"person\"},{\"count\":%" PRIu64
      ",\"key\":\"%s\",\"model\":\"person\"}],\"field\":[{\"count\":"
      "%" PRIu64 ",\"key\":%" PRId64 "},{\"count\":%" PRIu64
      ",\"key\":%" PRId64 "}],\"field_tuple\":[{\"count\":%" PRIu64
      ",\"key\":[%" PRId64 ",%" PRId64 "]},{\"count\":%" PRIu64
      ",\"key\":[%" PRId64 ",%" PRId64
      "]}]},\"reducers\":{\"count\":%" PRIu64 ",\"max\":%" PRId64
      ",\"mean_bits\":\"%016" PRIx64 "\",\"median_bits\":\"%016" PRIx64
      "\",\"min\":%" PRId64 ",\"std_bits\":\"%016" PRIx64
      "\",\"sum\":%" PRId64 "}}",
      value->binding_counts[0], value->binding_keys[0],
      value->binding_counts[1], value->binding_keys[1],
      value->field_counts[0], value->field_keys[0], value->field_counts[1],
      value->field_keys[1], value->tuple_counts[0], value->tuple_keys[0][0],
      value->tuple_keys[0][1], value->tuple_counts[1],
      value->tuple_keys[1][0], value->tuple_keys[1][1], value->count,
      value->maximum, value->mean_bits, value->median_bits, value->minimum,
      value->standard_deviation_bits, value->sum);
}

static int emit_exact_subtypes_observation(const char *proof_kind,
                                           const char *exact_key,
                                           const char *employee_key,
                                           const char *manager_key) {
  return emit_v2_observationf(
      "exact_subtypes", proof_kind,
      "{\"declared_model\":\"employee\",\"exact\":[{\"key\":\"%s\","
      "\"model\":\"employee\"}],\"subtypes\":[{\"key\":\"%s\",\"model\":"
      "\"employee\"},{\"key\":\"%s\",\"model\":\"manager\"}]}",
      exact_key, employee_key, manager_key);
}

static int emit_hydrated_result_observation(const char *proof_kind,
                                            const char *member_key) {
  return emit_v2_observationf(
      "hydrated_result", proof_kind,
      "{\"rows\":[{\"model\":\"membership\",\"roles\":{\"member\":[{"
      "\"key\":\"%s\",\"model\":\"person\"}]}}]}",
      member_key);
}

static int emit_roles_observation(const char *proof_kind,
                                  const char *member_key,
                                  const char *origin_key,
                                  const char *destination_key,
                                  const char *first_participant_key,
                                  const char *second_participant_key) {
  return emit_v2_observationf(
      "roles", proof_kind,
      "{\"membership\":{\"players\":[{\"key\":\"%s\",\"model\":"
      "\"person\"}],\"relation\":\"membership\",\"role\":\"member\"},"
      "\"network_link\":{\"destination\":\"%s\",\"origin\":\"%s\","
      "\"participants\":[\"%s\",\"%s\"],\"relation\":\"network-link\"}}",
      member_key, destination_key, origin_key, first_participant_key,
      second_participant_key);
}

static int emit_scalar_boolean_observation(
    const char *proof_kind, const sdk_person_rows_observation_t *and_rows,
    const sdk_person_rows_observation_t *field_rows,
    const sdk_person_rows_observation_t *not_rows,
    const sdk_person_rows_observation_t *or_rows) {
  if (and_rows == NULL || and_rows->count != 1u || field_rows == NULL ||
      field_rows->count != 1u || not_rows == NULL || not_rows->count != 1u ||
      or_rows == NULL || or_rows->count != 2u) {
    return 0;
  }
  return emit_v2_observationf(
      "scalar_boolean", proof_kind,
      "{\"and_keys\":[\"%s\"],\"field_comparison_keys\":[\"%s\"],"
      "\"not_keys\":[\"%s\"],\"or_keys\":[\"%s\",\"%s\"]}",
      and_rows->keys[0], field_rows->keys[0], not_rows->keys[0],
      or_rows->keys[0], or_rows->keys[1]);
}

static int emit_selection_shapes_observation(
    const char *proof_kind,
    const sdk_selection_shape_observation_t *observation) {
  if (observation == NULL || observation->named_participant_count != 2u ||
      observation->positional_participant_count != 2u ||
      !observation->collected_distinct ||
      observation->collection_order != TYPE_BRIDGE_QUERY_SORT_ASCENDING) {
    return 0;
  }
  return emit_v2_observationf(
      "selection_shapes", proof_kind,
      "{\"collected_distinct\":%s,\"collection_order\":\"identifier_asc\","
      "\"named\":{\"origin\":\"%s\",\"participants\":[\"%s\",\"%s\"]},"
      "\"positional\":[\"%s\",[\"%s\",\"%s\"]]}",
      observation->collected_distinct ? "true" : "false",
      observation->named_origin, observation->named_participants[0],
      observation->named_participants[1], observation->positional_origin,
      observation->positional_participants[0],
      observation->positional_participants[1]);
}

static int emit_topology_observation(
    const char *proof_kind,
    const sdk_topology_observation_t *observation) {
  if (observation == NULL || observation->cross_pair_count != 4u ||
      observation->max_hops == 0u) {
    return 0;
  }
  return emit_v2_observationf(
      "topology", proof_kind,
      "{\"cross_join_pairs\":[[\"%s\",\"%s\"],[\"%s\",\"%s\"],[\"%s\","
      "\"%s\"],[\"%s\",\"%s\"]],\"reachable\":[{\"from\":\"%s\","
      "\"max_hops\":%zu,\"to\":\"%s\"}]}",
      observation->cross_pairs[0][0], observation->cross_pairs[0][1],
      observation->cross_pairs[1][0], observation->cross_pairs[1][1],
      observation->cross_pairs[2][0], observation->cross_pairs[2][1],
      observation->cross_pairs[3][0], observation->cross_pairs[3][1],
      observation->reachable_from, observation->max_hops,
      observation->reachable_to);
}

static int emit_terminals_observation(
    const char *proof_kind, uint64_t count, int exists,
    const char *first_key, const char *one_key,
    const sdk_person_rows_observation_t *page,
    const sdk_person_rows_observation_t *rows, uint64_t offset,
    uint64_t limit, uint64_t total) {
  if (page == NULL || page->count != 1u || rows == NULL || rows->count != 2u) {
    return 0;
  }
  return emit_v2_observationf(
      "terminals", proof_kind,
      "{\"count\":%" PRIu64 ",\"exists\":%s,\"first\":\"%s\",\"one\":"
      "\"%s\",\"page\":{\"items\":[\"%s\"],\"limit\":%" PRIu64
      ",\"offset\":%" PRIu64 ",\"total\":%" PRIu64 "},\"rows\":[\"%s\","
      "\"%s\"]}",
      count, exists ? "true" : "false", first_key, one_key, page->keys[0],
      limit, offset, total, rows->keys[0], rows->keys[1]);
}

static int emit_structured_query_diagnostic_observation(
    const sdk_not_unique_observation_t *observation) {
  if (observation == NULL ||
      observation->category != TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT ||
      observation->path_kind !=
          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_RESULT ||
      observation->detail_kind != TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COUNT ||
      !observation->redacted) {
    return 0;
  }
  return emit_v2_observationf(
      "structured_query_diagnostic", "diagnostic",
      "{\"category\":\"invalid_input\",\"code\":\"%s\",\"details\":{"
      "\"%s\":{\"kind\":\"count\",\"value\":\"%" PRIu64
      "\"}},\"message\":\"%s\",\"path\":[{\"kind\":\"result\"}],"
      "\"query_category\":\"cardinality\",\"redacted\":true}",
      observation->code, observation->detail_key, observation->actual,
      observation->message);
}

static int emit_query_lifecycle_observation(
    const sdk_query_lifecycle_observation_t *observation) {
  if (observation == NULL || !observation->direct_lane_observed ||
      !observation->remote_lane_observed) {
    return 0;
  }
  return emit_v2_observationf(
      "query_resource_lifecycle", "lifecycle",
      "{\"lanes\":[\"direct\",\"remote\"],\"query\":{"
      "\"ancestor_usable_after_descendant_close\":%s,\"close_idempotent\":%s,"
      "\"descendant_usable_after_ancestor_close\":%s,\"handle_invalidated\":"
      "%s,\"post_close_io_count\":%zu,\"post_close_rejected\":%s,"
      "\"session_usable_after_query_close\":%s,\"sibling_usable\":%s},"
      "\"result_usable_after_query_close\":%s}",
      observation->ancestor_usable_after_descendant_close ? "true" : "false",
      observation->close_idempotent ? "true" : "false",
      observation->descendant_usable_after_ancestor_close ? "true" : "false",
      observation->handle_invalidated ? "true" : "false",
      observation->post_close_io_count,
      observation->post_close_rejected ? "true" : "false",
      observation->session_usable_after_query_close ? "true" : "false",
      observation->sibling_usable ? "true" : "false",
      observation->result_usable_after_query_close ? "true" : "false");
}

static int emit_resource_limits_observation(
    const char *proof_kind,
    const sdk_resource_limit_observation_t *observation) {
  if (observation == NULL ||
      observation->category != TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT ||
      observation->code[0] == '\0' || observation->dimension[0] == '\0') {
    return 0;
  }
  return emit_v2_observationf(
      "resource_limits", proof_kind,
      "{\"enforced\":{\"category\":\"resource_limit\",\"code\":\"%s\","
      "\"dimension\":\"%s\","
      "\"no_partial_result\":%s},\"hard_maxima\":{\"attribute_values\":%" PRIu64
      ",\"bytes\":%" PRIu64 ",\"collection_members\":%" PRIu64
      ",\"graph_nodes\":%" PRIu64 ",\"items\":%" PRIu64
      ",\"role_players\":%" PRIu64 ",\"statements\":%u,"
      "\"timeout_milliseconds\":%" PRIu64 "},\"plus_one_clamped_all\":%s,"
      "\"zero_tightening_all\":%s}",
      observation->code, observation->dimension,
      observation->no_partial_result ? "true" : "false",
      TYPE_BRIDGE_QUERY_DEFAULT_ATTRIBUTE_VALUES,
      TYPE_BRIDGE_QUERY_DEFAULT_BYTES,
      TYPE_BRIDGE_QUERY_DEFAULT_COLLECTION_MEMBERS,
      TYPE_BRIDGE_QUERY_DEFAULT_GRAPH_NODES, TYPE_BRIDGE_QUERY_DEFAULT_ITEMS,
      TYPE_BRIDGE_QUERY_DEFAULT_ROLE_PLAYERS,
      TYPE_BRIDGE_QUERY_DEFAULT_STATEMENTS,
      TYPE_BRIDGE_QUERY_DEFAULT_TIMEOUT_MILLISECONDS,
      observation->plus_one_dimensions == 8u ? "true" : "false",
      observation->zero_dimensions == 8u ? "true" : "false");
}

static int execution_code_is(
    const type_bridge_execution_diagnostics_t *diagnostics,
    const char *expected) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  const size_t expected_length = strlen(expected);
  return type_bridge_execution_diagnostics_get_v1(
             diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK &&
         diagnostic.code.length == expected_length &&
         memcmp(diagnostic.code.data, expected, expected_length) == 0;
}

static int check_structured_execution_diagnostic(
    const type_bridge_execution_diagnostics_t *diagnostics,
    type_bridge_execution_diagnostic_category_t expected_category,
    const char *expected_code) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  size_t count = 0u;
  size_t index;
  CHECK(diagnostics != NULL);
  CHECK(type_bridge_execution_diagnostics_count(diagnostics, &count) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 1u);
  CHECK(type_bridge_execution_diagnostics_get_v1(diagnostics, 0u,
                                                  &diagnostic) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostic.category == expected_category);
  if (expected_code != NULL) {
    CHECK(same_text(diagnostic.code, expected_code));
  } else {
    CHECK(diagnostic.code.data != NULL && diagnostic.code.length != 0u);
  }
  CHECK(diagnostic.message.data != NULL && diagnostic.message.length != 0u);
  CHECK(diagnostic.path_count != 0u);
  for (index = 0u; index < diagnostic.path_count; ++index) {
    type_bridge_execution_diagnostic_path_view_v1_t path = {0};
    CHECK(type_bridge_execution_diagnostics_path_get_v1(
              diagnostics, 0u, index, &path) == TYPE_BRIDGE_STATUS_OK);
    CHECK(path.kind != TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_UNKNOWN);
  }
  for (index = 0u; index < diagnostic.detail_count; ++index) {
    type_bridge_execution_diagnostic_detail_view_v1_t detail = {0};
    CHECK(type_bridge_execution_diagnostics_detail_get_v1(
              diagnostics, 0u, index, &detail) == TYPE_BRIDGE_STATUS_OK);
    CHECK(detail.kind != TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_UNKNOWN);
    CHECK(detail.key.data != NULL && detail.key.length != 0u);
  }
  return 0;
}

static int observe_resource_limit_diagnostic(
    const type_bridge_execution_diagnostics_t *diagnostics,
    const char *dimension, sdk_resource_limit_observation_t *observation) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  CHECK(diagnostics != NULL && dimension != NULL && observation != NULL);
  memset(observation, 0, sizeof(*observation));
  CHECK(type_bridge_execution_diagnostics_get_v1(
            diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostic.category == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT);
  observation->category = diagnostic.category;
  CHECK(copy_stable_text(diagnostic.code, observation->code,
                         sizeof(observation->code)));
  CHECK(snprintf(observation->dimension, sizeof(observation->dimension), "%s",
                 dimension) > 0);
  return 0;
}

static int check_not_unique_diagnostic(
    const type_bridge_execution_diagnostics_t *diagnostics,
    sdk_not_unique_observation_t *observation) {
  static const char expected_message[] =
      "The typed query result does not satisfy the requested cardinality";
  type_bridge_execution_diagnostic_view_v1_t diagnostic = {0};
  type_bridge_execution_diagnostic_path_view_v1_t path = {0};
  type_bridge_execution_diagnostic_detail_view_v1_t detail = {0};
  type_bridge_execution_diagnostic_detail_view_v1_t category_detail = {0};
  CHECK(diagnostics != NULL && observation != NULL);
  memset(observation, 0, sizeof(*observation));
  CHECK(type_bridge_execution_diagnostics_get_v1(
            diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostic.category == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT);
  CHECK(same_text(diagnostic.code, "not_unique"));
  CHECK(same_text(diagnostic.message, expected_message));
  CHECK(diagnostic.path_count == 1u && diagnostic.detail_count == 2u);
  CHECK(type_bridge_execution_diagnostics_path_get_v1(
            diagnostics, 0u, 0u, &path) == TYPE_BRIDGE_STATUS_OK);
  CHECK(path.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_QUERY_RESULT);
  CHECK(type_bridge_execution_diagnostics_detail_get_v1(
            diagnostics, 0u, 0u, &detail) == TYPE_BRIDGE_STATUS_OK);
  CHECK(detail.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_COUNT);
  CHECK(same_text(detail.key, "actual") && detail.unsigned_value == 2u);
  CHECK(type_bridge_execution_diagnostics_detail_get_v1(
            diagnostics, 0u, 1u, &category_detail) == TYPE_BRIDGE_STATUS_OK);
  CHECK(category_detail.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_TEXT);
  CHECK(same_text(category_detail.key, "query_category") &&
        same_text(category_detail.primary, "cardinality"));
  CHECK(strstr(expected_message, "query-ada") == NULL &&
        strstr(expected_message, "query-dana") == NULL);
  observation->category = diagnostic.category;
  observation->path_kind = path.kind;
  observation->detail_kind = detail.kind;
  observation->actual = detail.unsigned_value;
  CHECK(copy_stable_text(diagnostic.code, observation->code,
                         sizeof(observation->code)));
  CHECK(copy_stable_text(diagnostic.message, observation->message,
                         sizeof(observation->message)));
  CHECK(copy_stable_text(detail.key, observation->detail_key,
                         sizeof(observation->detail_key)));
  observation->redacted = strstr(observation->message, "query-ada") == NULL &&
                          strstr(observation->message, "query-dana") == NULL;
  return 0;
}

static uint32_t required_http_port(void) {
  const char *text = getenv("TYPEDB_HTTP_PORT");
  char *end = NULL;
  unsigned long value;
  if (text == NULL || text[0] == '\0') {
    return 0u;
  }
  errno = 0;
  value = strtoul(text, &end, 10);
  if (errno != 0 || end == text || *end != '\0' || value == 0ul ||
      value > 65535ul) {
    return 0u;
  }
  return (uint32_t)value;
}

typedef struct caller_http_body {
  uint8_t *data;
  size_t length;
} caller_http_body_t;

static uint16_t required_remote_port(void) {
  const char *text = getenv("TYPE_BRIDGE_C_REMOTE_PORT");
  char *end = NULL;
  unsigned long value;
  if (text == NULL || text[0] == '\0') {
    return 0u;
  }
  errno = 0;
  value = strtoul(text, &end, 10);
  if (errno != 0 || end == text || *end != '\0' || value == 0ul ||
      value > 65535ul) {
    return 0u;
  }
  return (uint16_t)value;
}

static int send_all(int socket_fd, const uint8_t *data, size_t length) {
  size_t offset = 0u;
  while (offset < length) {
    const ssize_t written =
        send(socket_fd, data + offset, length - offset, 0);
    if (written <= 0) {
      return 0;
    }
    offset += (size_t)written;
  }
  return 1;
}

/* The acceptance server returns bounded identity bodies with Content-Length.
 * This deliberately tiny client performs transport in ordinary C: the native
 * query ABI only prepares/authenticates bytes and never opens a socket. */
static int caller_http_exchange(uint16_t port, const char *method,
                                const char *path, const uint8_t *request_body,
                                size_t request_length, size_t body_limit,
                                caller_http_body_t *out_body) {
  struct sockaddr_in address;
  char header[512];
  int header_length;
  int socket_fd = -1;
  uint8_t *response = NULL;
  size_t response_length = 0u;
  size_t response_capacity = 0u;
  size_t header_end = 0u;
  size_t index;

  out_body->data = NULL;
  out_body->length = 0u;
  socket_fd = socket(AF_INET, SOCK_STREAM, 0);
  if (socket_fd < 0) {
    return 0;
  }
  memset(&address, 0, sizeof(address));
  address.sin_family = AF_INET;
  address.sin_port = htons(port);
  if (inet_pton(AF_INET, "127.0.0.1", &address.sin_addr) != 1 ||
      connect(socket_fd, (const struct sockaddr *)&address,
              sizeof(address)) != 0) {
    (void)close(socket_fd);
    return 0;
  }
  header_length = snprintf(
      header, sizeof(header),
      "%s %s HTTP/1.1\r\nHost: 127.0.0.1:%u\r\nConnection: close\r\n"
      "Content-Type: application/json\r\nContent-Length: %zu\r\n\r\n",
      method, path, (unsigned int)port, request_length);
  if (header_length <= 0 || (size_t)header_length >= sizeof(header) ||
      !send_all(socket_fd, (const uint8_t *)header, (size_t)header_length) ||
      (request_length != 0u &&
       !send_all(socket_fd, request_body, request_length))) {
    (void)close(socket_fd);
    return 0;
  }
  for (;;) {
    uint8_t block[4096];
    const ssize_t received = recv(socket_fd, block, sizeof(block), 0);
    if (received < 0) {
      free(response);
      (void)close(socket_fd);
      return 0;
    }
    if (received == 0) {
      break;
    }
    if (response_length + (size_t)received > body_limit + 16384u) {
      free(response);
      (void)close(socket_fd);
      return 0;
    }
    if (response_length + (size_t)received > response_capacity) {
      size_t capacity = response_capacity == 0u ? 8192u : response_capacity;
      uint8_t *grown;
      while (capacity < response_length + (size_t)received) {
        capacity *= 2u;
      }
      grown = (uint8_t *)realloc(response, capacity);
      if (grown == NULL) {
        free(response);
        (void)close(socket_fd);
        return 0;
      }
      response = grown;
      response_capacity = capacity;
    }
    memcpy(response + response_length, block, (size_t)received);
    response_length += (size_t)received;
  }
  (void)close(socket_fd);
  if (response_length < 12u ||
      (memcmp(response, "HTTP/1.1 200", 12u) != 0 &&
       memcmp(response, "HTTP/1.0 200", 12u) != 0)) {
    free(response);
    return 0;
  }
  for (index = 0u; index + 3u < response_length; ++index) {
    if (response[index] == '\r' && response[index + 1u] == '\n' &&
        response[index + 2u] == '\r' && response[index + 3u] == '\n') {
      header_end = index + 4u;
      break;
    }
  }
  if (header_end == 0u || response_length - header_end > body_limit) {
    free(response);
    return 0;
  }
  out_body->length = response_length - header_end;
  out_body->data = (uint8_t *)malloc(out_body->length == 0u ? 1u
                                                            : out_body->length);
  if (out_body->data == NULL) {
    free(response);
    return 0;
  }
  memcpy(out_body->data, response + header_end, out_body->length);
  free(response);
  return 1;
}

static int open_person_create(
    const type_bridge_schema_package_t *package, const char *identifier_text,
    int64_t score_number, fixture_person_create **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  char alias_text[96];
  fixture_aliases *alias = NULL;
  const fixture_aliases *aliases[1];
  fixture_person_create_field_aliases_chunks_v1_t aliases_chunk = {0};
  fixture_person_create_args_v1_t args = {0};
  fixture_person_create *empty_optional_create = NULL;
  fixture_person_create *missing_required_create =
      (fixture_person_create *)(uintptr_t)1u;
  fixture_foozuzubar *foo = NULL;
  fixture_identifier *identifier = NULL;
  fixture_nickname *nickname = NULL;
  fixture_score *score = NULL;
  fixture_scorezuzugte *score_gte = NULL;
  fixture_valzubool *boolean_value = NULL;
  fixture_valzuconstrained *constrained = NULL;
  fixture_valzudate *date = NULL;
  fixture_valzudatetime *datetime = NULL;
  fixture_valzudatetimezutzz *datetime_tz = NULL;
  fixture_valzudecimal *decimal = NULL;
  fixture_valzudouble *double_value = NULL;
  fixture_valzuduration *duration = NULL;
  const int is_query_ada = strcmp(identifier_text, "query-ada") == 0;
  const int is_query_dana = strcmp(identifier_text, "query-dana") == 0;
  const int is_v5_live = strcmp(identifier_text, "v5-live-person") == 0;
  const int is_sdk = is_query_ada || is_query_dana || is_v5_live;
  const char *alias_value =
      is_query_ada ? "analyst" : (is_query_dana ? "engineer" : alias_text);
  const char *date_value = is_query_dana ? "2026-08-13" : "2026-08-12";
  const char *datetime_value =
      is_query_dana ? "2026-08-13T10:45:00" : "2026-08-12T09:30:00";
  const char *datetime_tz_value =
      is_query_dana ? "2026-08-13T10:45:00Z" : "2026-08-12T09:30:00Z";
  const char *decimal_value_text = is_query_dana ? "45.5" : "38.5";
  const char *duration_value = is_query_dana ? "PT45S" : "PT38S";
  const uint64_t double_bits = is_query_dana
                                   ? UINT64_C(0x4046800000000000)
                                   : UINT64_C(0x4043000000000000);

  CHECK(snprintf(alias_text, sizeof(alias_text), "%s-alias", identifier_text) >
        0);
  if (!is_v5_live) {
    CHECK(fixture_aliases_open(package, view_of(alias_value), &alias,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    aliases[0] = alias;
    CHECK(fixture_foozuzubar_open(package, score_number + 100, &foo,
                                  out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_identifier_open(package, view_of(identifier_text), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  if (!is_query_dana && !is_v5_live) {
    CHECK(fixture_nickname_open(
              package, view_of(is_query_ada ? "Ada" : identifier_text),
              &nickname, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_score_open(package, score_number, &score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  if (!is_v5_live) {
    CHECK(fixture_scorezuzugte_open(package,
                                    is_sdk ? 40 : score_number - 1,
                                    &score_gte,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_valzubool_open(package, is_query_ada ? 0u : 1u,
                               &boolean_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_open(package,
                                      is_v5_live
                                          ? 55
                                          : is_sdk
                                          ? (is_query_dana ? 45 : 38)
                                          : 40,
                                      &constrained,
                                      out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudate_open(
            package,
            view_of(is_v5_live ? "2026-08-03"
                               : (is_sdk ? date_value : "2026-08-11")),
            &date,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetime_open(
            package,
            view_of(is_v5_live
                        ? "2026-08-03T03:55:00"
                        : (is_sdk ? datetime_value
                                        : "2026-08-11T07:30:00.5")),
            &datetime,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetimezutzz_open(
            package,
            view_of(is_v5_live
                        ? "2026-08-03T03:55:00Z"
                        : (is_sdk ? datetime_tz_value
                                        : "2026-08-11T07:30:00Z")),
            &datetime_tz,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudecimal_open(
            package,
            view_of(is_v5_live ? "128.45"
                               : (is_sdk ? decimal_value_text : "12.5")),
            &decimal,
                                  out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudouble_open(
            package,
            is_v5_live ? UINT64_C(0x402047ae147ae148)
                       : (is_sdk ? double_bits
                                       : UINT64_C(0x3ff8000000000000)),
            &double_value,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuduration_open(
            package,
            view_of(is_v5_live ? "P6D"
                               : (is_sdk ? duration_value : "P1D")),
            &duration,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);

  aliases_chunk.struct_size = sizeof(aliases_chunk);
  aliases_chunk.version = FIXTURE_CREATE_ARGS_VERSION;
  aliases_chunk.values = is_v5_live ? NULL : aliases;
  aliases_chunk.count = is_v5_live ? 0u : 1u;
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_aliases_chunks = is_v5_live ? NULL : &aliases_chunk;
  args.field_foozuzubar = foo;
  args.field_identifier = identifier;
  args.field_nickname = nickname;
  args.field_score = score;
  args.field_scorezuzugte = score_gte;
  args.field_valzubool = boolean_value;
  args.field_valzuconstrained = constrained;
  args.field_valzudate = date;
  args.field_valzudatetime = datetime;
  args.field_valzudatetimezutzz = datetime_tz;
  args.field_valzudecimal = decimal;
  args.field_valzudouble = double_value;
  args.field_valzuduration = duration;

  CHECK(fixture_person_create_open(package, &args, out_create,
                                   out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_create != NULL && *out_diagnostics == NULL);

  /* NULL optional scalar and sequence inputs are omitted from the streaming
     builder and normalized by the projected-create contract. */
  args.field_aliases_chunks = NULL;
  args.field_foozuzubar = NULL;
  args.field_nickname = NULL;
  args.field_scorezuzugte = NULL;
  CHECK(fixture_person_create_open(package, &args, &empty_optional_create,
                                   out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(empty_optional_create != NULL && *out_diagnostics == NULL);
  CHECK(fixture_person_create_close(&empty_optional_create) ==
        TYPE_BRIDGE_STATUS_OK);

  args.field_score = NULL;
  CHECK(fixture_person_create_open(package, &args, &missing_required_create,
                                   out_diagnostics) ==
        TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(missing_required_create == NULL && *out_diagnostics != NULL);
  CHECK(execution_code_is(*out_diagnostics, "missing_required_field"));
  CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_foozuzubar_close(&foo) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_scorezuzugte_close(&score_gte) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_close(&constrained) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudate_close(&date) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetime_close(&datetime) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetimezutzz_close(&datetime_tz) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudecimal_close(&decimal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudouble_close(&double_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuduration_close(&duration) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int open_employee_create(
    const type_bridge_schema_package_t *package, const char *identifier_text,
    int64_t rank_number, fixture_employee_create **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_employee_create_args_v1_t args = {0};
  fixture_identifier *identifier = NULL;
  fixture_partyzuname *party_name = NULL;
  fixture_rank *rank = NULL;

  CHECK(fixture_identifier_open(package, view_of(identifier_text), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_partyzuname_open(package, view_of("Query Employee"), &party_name,
                                 out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_rank_open(package, rank_number, &rank, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_identifier = identifier;
  args.field_partyzuname = party_name;
  args.field_rank = rank;
  CHECK(fixture_employee_create_open(package, &args, out_create,
                                     out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_create != NULL && *out_diagnostics == NULL);
  CHECK(fixture_rank_close(&rank) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_partyzuname_close(&party_name) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int open_manager_create(
    const type_bridge_schema_package_t *package, const char *identifier_text,
    int64_t rank_number, fixture_manager_create **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_manager_create_args_v1_t args = {0};
  fixture_identifier *identifier = NULL;
  fixture_managerzunote *manager_note = NULL;
  fixture_partyzuname *party_name = NULL;
  fixture_rank *rank = NULL;

  CHECK(fixture_identifier_open(package, view_of(identifier_text), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_managerzunote_open(package, view_of("query lead"), &manager_note,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_partyzuname_open(package, view_of("Query Manager"), &party_name,
                                 out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_rank_open(package, rank_number, &rank, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_identifier = identifier;
  args.field_managerzunote = manager_note;
  args.field_partyzuname = party_name;
  args.field_rank = rank;
  CHECK(fixture_manager_create_open(package, &args, out_create,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_create != NULL && *out_diagnostics == NULL);
  CHECK(fixture_rank_close(&rank) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_partyzuname_close(&party_name) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_managerzunote_close(&manager_note) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int open_network_link_create(
    const type_bridge_schema_package_t *package,
    type_bridge_byte_view_t origin_iid,
    type_bridge_byte_view_t destination_iid,
    fixture_networkzhlink_create **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_networkzhlink_create_args_v1_t args = {0};
  fixture_networkzhlink_create_role_participant_chunks_v1_t participants_chunk =
      {0};
  fixture_identifier *identifier = NULL;
  fixture_nickname *nickname = NULL;
  fixture_person_ref *origin_reference = NULL;
  fixture_person_ref *destination_reference = NULL;
  fixture_networkzhlink_origin_player *origin = NULL;
  fixture_networkzhlink_destination_player *destination = NULL;
  fixture_networkzhlink_participant_player *origin_participant = NULL;
  fixture_networkzhlink_participant_player *destination_participant = NULL;
  const fixture_networkzhlink_participant_player *participants[2];

  CHECK(fixture_identifier_open(package, view_of("query-link"), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_nickname_open(package, view_of("query route"), &nickname,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_from_iid(package, origin_iid, &origin_reference,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_from_iid(package, destination_iid,
                                    &destination_reference,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_origin_player_from_person(
            origin_reference, &origin, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_destination_player_from_person(
            destination_reference, &destination, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_participant_player_from_person(
            origin_reference, &origin_participant, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_participant_player_from_person(
            destination_reference, &destination_participant,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  participants[0] = origin_participant;
  participants[1] = destination_participant;
  participants_chunk.struct_size = sizeof(participants_chunk);
  participants_chunk.version = FIXTURE_CREATE_ARGS_VERSION;
  participants_chunk.values = participants;
  participants_chunk.count = 2u;
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_identifier = identifier;
  args.field_nickname = nickname;
  args.role_destination = destination;
  args.role_origin = origin;
  args.role_participant_chunks = &participants_chunk;
  CHECK(fixture_networkzhlink_create_open(package, &args, out_create,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_create != NULL && *out_diagnostics == NULL);
  CHECK(fixture_networkzhlink_participant_player_close(
            &destination_participant) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_participant_player_close(&origin_participant) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_destination_player_close(&destination) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_origin_player_close(&origin) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&destination_reference) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&origin_reference) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_person(
    const fixture_person *person, const char *expected_identifier,
    int64_t expected_score,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  char expected_alias[96];
  fixture_aliases *alias = NULL;
  fixture_foozuzubar *foo = NULL;
  fixture_identifier *identifier = NULL;
  fixture_nickname *nickname = NULL;
  fixture_score *score = NULL;
  fixture_scorezuzugte *score_gte = NULL;
  fixture_valzubool *boolean_value = NULL;
  fixture_valzuconstrained *constrained = NULL;
  fixture_valzudate *date = NULL;
  fixture_valzudatetime *datetime = NULL;
  fixture_valzudatetimezutzz *datetime_tz = NULL;
  fixture_valzudecimal *decimal = NULL;
  fixture_valzudouble *double_value = NULL;
  fixture_valzuduration *duration = NULL;
  fixture_person_ref *reference = NULL;
  type_bridge_byte_view_t text = {NULL, 0u};
  int64_t long_value = 0;
  uint64_t bits = 0u;
  uint8_t bool_value = 0u;
  size_t aliases_count = 0u;
  const int is_query_ada = strcmp(expected_identifier, "query-ada") == 0;
  const int is_query_dana = strcmp(expected_identifier, "query-dana") == 0;
  const int is_sdk = is_query_ada || is_query_dana;
  const char *expected_alias_value =
      is_query_ada ? "analyst" : (is_query_dana ? "engineer" : expected_alias);

  CHECK(snprintf(expected_alias, sizeof(expected_alias), "%s-alias",
                 expected_identifier) > 0);
  CHECK(fixture_person_aliases_count(person, &aliases_count,
                                     out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(aliases_count == 1u);
  CHECK(fixture_person_aliases_at(person, 0u, &alias, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_aliases_value(alias, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, expected_alias_value));
  CHECK(fixture_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_person_foozuzubar(person, &foo, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_foozuzubar_value(foo, &long_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(long_value == expected_score + 100);
  CHECK(fixture_foozuzubar_close(&foo) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier(person, &identifier, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, expected_identifier));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_nickname(person, &nickname, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  if (is_query_dana) {
    CHECK(nickname == NULL);
  } else {
    CHECK(fixture_nickname_value(nickname, &text, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_text(text, is_query_ada ? "Ada" : expected_identifier));
    CHECK(fixture_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_person_score(person, &score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_value(score, &long_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(long_value == expected_score);
  CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_scorezuzugte(person, &score_gte, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_scorezuzugte_value(score_gte, &long_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(long_value == (is_sdk ? 40 : expected_score - 1));
  CHECK(fixture_scorezuzugte_close(&score_gte) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzubool(person, &boolean_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzubool_value(boolean_value, &bool_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(bool_value == (is_query_ada ? 0u : 1u));
  CHECK(fixture_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzuconstrained(person, &constrained,
                                        out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_value(constrained, &long_value,
                                       out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(long_value == (is_sdk ? (is_query_dana ? 45 : 38) : 40));
  CHECK(fixture_valzuconstrained_close(&constrained) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudate(person, &date, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudate_value(date, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, is_sdk
                            ? (is_query_dana ? "2026-08-13" : "2026-08-12")
                            : "2026-08-11"));
  CHECK(fixture_valzudate_close(&date) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudatetime(person, &datetime, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetime_value(datetime, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text,
                  is_sdk
                      ? (is_query_dana ? "2026-08-13T10:45:00"
                                       : "2026-08-12T09:30:00")
                      : "2026-08-11T07:30:00.5"));
  CHECK(fixture_valzudatetime_close(&datetime) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudatetimezutzz(person, &datetime_tz,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetimezutzz_value(datetime_tz, &text,
                                         out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text,
                  is_sdk
                      ? (is_query_dana ? "2026-08-13T10:45:00Z"
                                       : "2026-08-12T09:30:00Z")
                      : "2026-08-11T07:30:00Z"));
  CHECK(fixture_valzudatetimezutzz_close(&datetime_tz) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudecimal(person, &decimal, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudecimal_value(decimal, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, is_sdk
                            ? (is_query_dana ? "45.5" : "38.5")
                            : "12.5"));
  CHECK(fixture_valzudecimal_close(&decimal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudouble(person, &double_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudouble_value(double_value, &bits, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(bits == (is_sdk
                     ? (is_query_dana ? UINT64_C(0x4046800000000000)
                                      : UINT64_C(0x4043000000000000))
                     : UINT64_C(0x3ff8000000000000)));
  CHECK(fixture_valzudouble_close(&double_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzuduration(person, &duration, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuduration_value(duration, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, is_sdk
                            ? (is_query_dana ? "PT45S" : "PT38S")
                            : "P1D"));
  CHECK(fixture_valzuduration_close(&duration) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_person_reference(person, &reference, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_identifier_key(reference, &identifier,
                                           out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, expected_identifier));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_diagnostics == NULL);
  return 0;
}

static int copy_person_iid(const fixture_person *person, uint8_t *storage,
                           size_t capacity, size_t *out_length,
                           type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_byte_view_t iid = {NULL, 0u};
  CHECK(fixture_person_iid(person, &iid, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(iid.data != NULL && iid.length != 0u && iid.length <= capacity);
  memcpy(storage, iid.data, iid.length);
  *out_length = iid.length;
  return 0;
}

static int check_reference_constructors(
    const type_bridge_schema_package_t *package, type_bridge_byte_view_t iid,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_identifier *identifier = NULL;
  fixture_identifier *key = NULL;
  fixture_person_ref *reference = NULL;
  type_bridge_byte_view_t actual = {NULL, 0u};

  CHECK(fixture_person_ref_from_iid(package, iid, &reference,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_iid(reference, &actual, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(actual.length == iid.length &&
        memcmp(actual.data, iid.data, iid.length) == 0);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_identifier_open(package, view_of("query-ada"), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_from_key(package, identifier, &reference,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_identifier_key(reference, &key, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(key, &actual, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(actual, "query-ada"));
  CHECK(fixture_identifier_close(&key) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_diagnostics == NULL);
  return 0;
}

static int open_membership_create(
    const type_bridge_schema_package_t *package, type_bridge_byte_view_t person_iid,
    fixture_membership_create **out_create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_person_ref *person_reference = NULL;
  fixture_person_ref *extracted_reference = NULL;
  fixture_membership_member_player *member = NULL;
  fixture_membership_create_args_v1_t args = {0};
  fixture_membership_member_player_kind_t kind =
      fixture_membership_member_player_kind_unknown;
  type_bridge_byte_view_t extracted_iid = {NULL, 0u};

  CHECK(fixture_person_ref_from_iid(package, person_iid, &person_reference,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_member_player_from_person(
            person_reference, &member, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(member != NULL && *out_diagnostics == NULL);
  CHECK(fixture_membership_member_player_kind(member, &kind,
                                               out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(kind == fixture_membership_member_player_kind_person);
  CHECK(fixture_membership_member_player_as_person(
            member, &extracted_reference, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(extracted_reference != NULL);
  CHECK(fixture_person_ref_iid(extracted_reference, &extracted_iid,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(extracted_iid.length == person_iid.length &&
        memcmp(extracted_iid.data, person_iid.data, person_iid.length) == 0);

  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.role_member = member;
  CHECK(fixture_membership_create_open(package, &args, out_create,
                                        out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_create != NULL && *out_diagnostics == NULL);
  /* The create snapshots the union; both the union and its extracted reference
     remain independently owned and reusable. */
  kind = fixture_membership_member_player_kind_unknown;
  CHECK(fixture_membership_member_player_kind(member, &kind,
                                               out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(kind == fixture_membership_member_player_kind_person);
  CHECK(fixture_person_ref_close(&extracted_reference) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_member_player_close(&member) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&person_reference) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_membership(
    const fixture_membership *membership,
    type_bridge_byte_view_t expected_person_iid,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_membership_member_player *member = NULL;
  fixture_membership_member_player_kind_t kind =
      fixture_membership_member_player_kind_unknown;
  fixture_person_ref *person_reference = NULL;
  fixture_membership_ref *membership_reference = NULL;
  type_bridge_byte_view_t actual_iid = {NULL, 0u};

  CHECK(fixture_membership_reference(membership, &membership_reference,
                                      out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(membership_reference != NULL);
  CHECK(fixture_membership_ref_close(&membership_reference) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_member(membership, &member, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(member != NULL);
  CHECK(fixture_membership_member_player_kind(member, &kind,
                                               out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(kind == fixture_membership_member_player_kind_person);
  CHECK(fixture_membership_member_player_as_person(
            member, &person_reference, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(person_reference != NULL);
  CHECK(fixture_person_ref_iid(person_reference, &actual_iid,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(actual_iid.length == expected_person_iid.length &&
        memcmp(actual_iid.data, expected_person_iid.data,
               expected_person_iid.length) == 0);
  CHECK(fixture_person_ref_close(&person_reference) == TYPE_BRIDGE_STATUS_OK);
  /* Extracted references are clones; closing one does not consume the union. */
  kind = fixture_membership_member_player_kind_unknown;
  CHECK(fixture_membership_member_player_kind(member, &kind,
                                               out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(kind == fixture_membership_member_player_kind_person);
  CHECK(fixture_membership_member_player_close(&member) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(*out_diagnostics == NULL);
  return 0;
}

static int copy_membership_iid(
    const fixture_membership *membership, uint8_t *storage, size_t capacity,
    size_t *out_length,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_byte_view_t iid = {NULL, 0u};
  CHECK(fixture_membership_iid(membership, &iid, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(iid.data != NULL && iid.length != 0u && iid.length <= capacity);
  memcpy(storage, iid.data, iid.length);
  *out_length = iid.length;
  return 0;
}

static int check_query_person_score(
    const fixture_person *person, int64_t minimum,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_score *score = NULL;
  int64_t value = 0;
  CHECK(fixture_person_score(person, &score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(score != NULL);
  CHECK(fixture_score_value(score, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(value >= minimum);
  CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_query_person_identifier(
    const fixture_person *person, const char *expected,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_identifier *identifier = NULL;
  type_bridge_byte_view_t value = {NULL, 0u};
  CHECK(fixture_person_identifier(person, &identifier, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(value, expected));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int read_query_person_identifier(
    const fixture_person *person, char *output, size_t capacity,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_identifier *identifier = NULL;
  type_bridge_byte_view_t value = {NULL, 0u};
  CHECK(fixture_person_identifier(person, &identifier, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_stable_text(value, output, capacity));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int read_query_person_score(
    const fixture_person *person, int64_t *out_score,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_score *score = NULL;
  CHECK(out_score != NULL);
  CHECK(fixture_person_score(person, &score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_value(score, out_score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int observe_query_person_model_values(
    const fixture_person *person,
    sdk_person_rows_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_aliases *alias = NULL;
  fixture_valzubool *boolean_value = NULL;
  fixture_valzudate *date = NULL;
  fixture_valzudatetime *datetime = NULL;
  fixture_valzudatetimezutzz *datetime_tz = NULL;
  fixture_valzudecimal *decimal = NULL;
  fixture_valzudouble *double_value = NULL;
  fixture_valzuduration *duration = NULL;
  fixture_identifier *identifier = NULL;
  fixture_nickname *nickname = NULL;
  fixture_person_ref *reference = NULL;
  fixture_score *score = NULL;
  type_bridge_byte_view_t value = {NULL, 0u};
  uint8_t bool_value = 0u;
  uint64_t double_bits = 0u;
  int64_t long_value = 0;
  size_t alias_count = 0u;

  CHECK(person != NULL && observation != NULL);
  CHECK(fixture_person_aliases_count(person, &alias_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(alias_count != 0u);
  CHECK(fixture_person_aliases_at(person, 0u, &alias, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_aliases_value(alias, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_stable_text(value, observation->first_alias,
                         sizeof(observation->first_alias)));
  CHECK(fixture_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_person_nickname(person, &nickname, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  if (nickname != NULL) {
    CHECK(fixture_nickname_value(nickname, &value, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_stable_text(value, observation->first_nickname,
                           sizeof(observation->first_nickname)));
    CHECK(fixture_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  }

  CHECK(fixture_person_reference(person, &reference, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_identifier_key(reference, &identifier,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_stable_text(value, observation->first_reference_key,
                         sizeof(observation->first_reference_key)));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_person_valzubool(person, &boolean_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzubool_value(boolean_value, &bool_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 0;
  CHECK(fixture_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudate(person, &date, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudate_value(date, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 1;
  CHECK(fixture_valzudate_close(&date) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudatetime(person, &datetime, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetime_value(datetime, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 2;
  CHECK(fixture_valzudatetime_close(&datetime) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudatetimezutzz(person, &datetime_tz,
                                         out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetimezutzz_value(datetime_tz, &value,
                                         out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 3;
  CHECK(fixture_valzudatetimezutzz_close(&datetime_tz) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudecimal(person, &decimal, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudecimal_value(decimal, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 4;
  CHECK(fixture_valzudecimal_close(&decimal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzudouble(person, &double_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudouble_value(double_value, &double_bits, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 5;
  CHECK(fixture_valzudouble_close(&double_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_valzuduration(person, &duration, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuduration_value(duration, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 6;
  CHECK(fixture_valzuduration_close(&duration) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_score(person, &score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_value(score, &long_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 7;
  CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier(person, &identifier, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  observation->first_scalar_domain_mask |= UINT32_C(1) << 8;
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int observe_person_rows(
    fixture_query_rows_result *result,
    sdk_person_rows_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  size_t row_count = 0u;
  size_t row_index;
  CHECK(observation != NULL);
  memset(observation, 0, sizeof(*observation));
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count <= 4u);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_person *person = NULL;
    CHECK(fixture_person_query_exact_one_at(
              fixture_person_query_exact_one_result_slot_v1_t_rows(result,
                                                                    0u),
              row_index, &person, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(read_query_person_identifier(person, observation->keys[row_index],
                                       sizeof(observation->keys[row_index]),
                                       out_diagnostics) == 0);
    CHECK(read_query_person_score(person, &observation->scores[row_index],
                                  out_diagnostics) == 0);
    CHECK(check_person(person, observation->keys[row_index],
                       observation->scores[row_index], out_diagnostics) == 0);
    if (row_index == 0u) {
      CHECK(observe_query_person_model_values(person, observation,
                                              out_diagnostics) == 0);
    }
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  }
  observation->count = row_count;
  return 0;
}

static int check_rows_identifiers(
    fixture_query_rows_result *result, const char *const *expected,
    size_t expected_count,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_query_row_result_ref_v1_t result_ref =
      fixture_query_rows_result_ref(result);
  size_t row_count = 0u;
  size_t row_index;
  CHECK(fixture_query_result_row_count(result_ref, &row_count,
                                       out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == expected_count);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_person_query_exact_one_result_slot_v1_t slot =
        fixture_person_query_exact_one_result_slot_v1_t_rows(result, 0u);
    fixture_person *person = NULL;
    CHECK(fixture_person_query_exact_one_at(slot, row_index, &person,
                                            out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_query_person_identifier(person, expected[row_index],
                                        out_diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  }
  return 0;
}

static int check_rows_result(
    fixture_query_rows_result *result, size_t expected_count,
    int64_t minimum,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_query_row_result_ref_v1_t result_ref =
      fixture_query_rows_result_ref(result);
  size_t row_count = 0u;
  size_t row_index;
  int seen_38 = 0;
  int seen_45 = 0;
  CHECK(fixture_query_result_row_count(result_ref, &row_count,
                                       out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == expected_count);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_person_query_exact_one_result_slot_v1_t slot =
        fixture_person_query_exact_one_result_slot_v1_t_rows(result, 0u);
    fixture_person *person = NULL;
    fixture_score *score = NULL;
    int64_t score_value = 0;
    CHECK(fixture_person_query_exact_one_at(slot, row_index, &person,
                                            out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(person != NULL);
    CHECK(check_query_person_score(person, minimum, out_diagnostics) == 0);
    CHECK(fixture_person_score(person, &score, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_score_value(score, &score_value, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    if (expected_count == 2u && minimum == 30 &&
        (score_value == 38 || score_value == 45)) {
      CHECK(check_person(person,
                         score_value == 38 ? "query-ada" : "query-dana",
                         score_value, out_diagnostics) == 0);
    }
    seen_38 += score_value == 38;
    seen_45 += score_value == 45;
    CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  }
  if (expected_count == 2u && minimum == 30) {
    CHECK(seen_38 == 1 && seen_45 == 1);
  }
  return 0;
}

static int check_employee_exact_result(
    fixture_query_rows_result *result, char *out_key, size_t key_capacity,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_employee *value = NULL;
  fixture_identifier *identifier = NULL;
  type_bridge_byte_view_t text = {NULL, 0u};
  size_t row_count = 0u;
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 1u);
  CHECK(fixture_employee_query_exact_one_at(
            fixture_employee_query_exact_one_result_slot_v1_t_rows(result,
                                                                    0u),
            0u, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_identifier(value, &identifier, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, "query-employee"));
  CHECK(copy_stable_text(text, out_key, key_capacity));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_close(&value) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_employee_subtype_result(
    fixture_query_rows_result *result, char *out_employee_key,
    size_t employee_key_capacity, char *out_manager_key,
    size_t manager_key_capacity,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_employee_query_subtypes_result *first = NULL;
  fixture_employee_query_subtypes_result *second = NULL;
  fixture_employee_query_subtypes_result_kind_t first_kind =
      fixture_employee_query_subtypes_result_kind_unknown;
  fixture_employee_query_subtypes_result_kind_t second_kind =
      fixture_employee_query_subtypes_result_kind_unknown;
  fixture_employee *employee_value = NULL;
  fixture_manager *manager_value = NULL;
  fixture_identifier *identifier = NULL;
  type_bridge_byte_view_t text = {NULL, 0u};
  size_t row_count = 0u;
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 2u);
  CHECK(fixture_employee_query_subtypes_one_at(
            fixture_employee_query_subtypes_one_result_slot_v1_t_rows(result,
                                                                       0u),
            0u, &first, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_query_subtypes_one_at(
            fixture_employee_query_subtypes_one_result_slot_v1_t_rows(result,
                                                                       0u),
            1u, &second, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_query_subtypes_result_kind(
            first, &first_kind, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_query_subtypes_result_kind(
            second, &second_kind, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(first_kind == fixture_employee_query_subtypes_result_kind_employee);
  CHECK(second_kind == fixture_employee_query_subtypes_result_kind_manager);
  CHECK(fixture_employee_query_subtypes_result_as_employee(
            first, &employee_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_identifier(employee_value, &identifier,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, "query-employee"));
  CHECK(copy_stable_text(text, out_employee_key, employee_key_capacity));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_query_subtypes_result_as_manager(
            second, &manager_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_manager_identifier(manager_value, &identifier,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, "query-manager"));
  CHECK(copy_stable_text(text, out_manager_key, manager_key_capacity));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_close(&employee_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_manager_close(&manager_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_query_subtypes_result_close(&second) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employee_query_subtypes_result_close(&first) ==
        TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_membership_query_result(
    fixture_query_rows_result *result, type_bridge_byte_view_t ada_iid,
    char *out_member_key, size_t member_key_capacity,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_membership *membership = NULL;
  fixture_person *member = NULL;
  size_t row_count = 0u;
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 1u);
  CHECK(fixture_membership_query_exact_one_at(
            fixture_membership_query_exact_one_result_slot_v1_t_rows(result,
                                                                      0u),
            0u, &membership, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_membership(membership, ada_iid, out_diagnostics) == 0);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(result, 1u),
            0u, &member, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_query_person_identifier(member, "query-ada", out_diagnostics) ==
        0);
  CHECK(read_query_person_identifier(member, out_member_key,
                                     member_key_capacity, out_diagnostics) ==
        0);
  CHECK(fixture_person_close(&member) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_network_topology_result(
    fixture_query_rows_result *result, char *out_source_key,
    size_t source_key_capacity, char *out_target_key,
    size_t target_key_capacity,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_networkzhlink *link = NULL;
  fixture_identifier *identifier = NULL;
  fixture_person *source = NULL;
  fixture_person *target = NULL;
  type_bridge_byte_view_t text = {NULL, 0u};
  size_t row_count = 0u;
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 1u);
  CHECK(fixture_networkzhlink_query_exact_one_at(
            fixture_networkzhlink_query_exact_one_result_slot_v1_t_rows(result,
                                                                         0u),
            0u, &link, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_identifier(link, &identifier, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, "query-link"));
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(result, 1u),
            0u, &source, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(result, 2u),
            0u, &target, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_query_person_identifier(source, "query-ada", out_diagnostics) ==
        0);
  CHECK(check_query_person_identifier(target, "query-dana", out_diagnostics) ==
        0);
  CHECK(read_query_person_identifier(source, out_source_key,
                                     source_key_capacity, out_diagnostics) ==
        0);
  CHECK(read_query_person_identifier(target, out_target_key,
                                     target_key_capacity, out_diagnostics) ==
        0);
  CHECK(fixture_person_close(&target) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_close(&source) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_reachable_result(
    fixture_query_rows_result *result,
    sdk_topology_observation_t *observation, size_t max_hops,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_person *source = NULL;
  fixture_person *target = NULL;
  size_t row_count = 0u;
  CHECK(observation != NULL && max_hops != 0u);
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 1u);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(result, 0u),
            0u, &source, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(result, 1u),
            0u, &target, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_query_person_identifier(source, "query-ada", out_diagnostics) ==
        0);
  CHECK(check_query_person_identifier(target, "query-dana", out_diagnostics) ==
        0);
  CHECK(read_query_person_identifier(source, observation->reachable_from,
                                     sizeof(observation->reachable_from),
                                     out_diagnostics) == 0);
  CHECK(read_query_person_identifier(target, observation->reachable_to,
                                     sizeof(observation->reachable_to),
                                     out_diagnostics) == 0);
  observation->max_hops = max_hops;
  CHECK(fixture_person_close(&target) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_close(&source) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_cross_join_result(
    fixture_query_rows_result *result,
    sdk_topology_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  int seen[4] = {0, 0, 0, 0};
  size_t row_count = 0u;
  size_t row_index;
  CHECK(observation != NULL);
  CHECK(fixture_query_result_row_count(fixture_query_rows_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 4u);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_person *left = NULL;
    fixture_person *right = NULL;
    fixture_identifier *left_identifier = NULL;
    fixture_identifier *right_identifier = NULL;
    type_bridge_byte_view_t left_text = {NULL, 0u};
    type_bridge_byte_view_t right_text = {NULL, 0u};
    int left_index;
    int right_index;
    CHECK(fixture_person_query_exact_one_at(
              fixture_person_query_exact_one_result_slot_v1_t_rows(result,
                                                                    0u),
              row_index, &left, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_at(
              fixture_person_query_exact_one_result_slot_v1_t_rows(result,
                                                                    1u),
              row_index, &right, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier(left, &left_identifier,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier(right, &right_identifier,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_identifier_value(left_identifier, &left_text,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_identifier_value(right_identifier, &right_text,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    left_index = same_text(left_text, "query-ada")
                     ? 0
                     : (same_text(left_text, "query-dana") ? 1 : -1);
    right_index = same_text(right_text, "query-ada")
                      ? 0
                      : (same_text(right_text, "query-dana") ? 1 : -1);
    CHECK(left_index >= 0 && right_index >= 0);
    CHECK(seen[left_index * 2 + right_index] == 0);
    seen[left_index * 2 + right_index] = 1;
    CHECK(copy_stable_text(
        left_text, observation->cross_pairs[left_index * 2 + right_index][0],
        sizeof(observation->cross_pairs[left_index * 2 + right_index][0])));
    CHECK(copy_stable_text(
        right_text, observation->cross_pairs[left_index * 2 + right_index][1],
        sizeof(observation->cross_pairs[left_index * 2 + right_index][1])));
    CHECK(fixture_identifier_close(&right_identifier) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_identifier_close(&left_identifier) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_close(&right) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_close(&left) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(seen[0] == 1 && seen[1] == 1 && seen[2] == 1 && seen[3] == 1);
  observation->cross_pair_count = row_count;
  return 0;
}

static int check_selection_shape_result(
    fixture_query_page_result *result,
    char *out_origin, size_t origin_capacity,
    char out_participants[2][64], size_t *out_participant_count,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  const char *const expected_participants[2] = {"query-ada", "query-dana"};
  fixture_person *origin = NULL;
  fixture_person_query_exact_collect_result_slot_v1_t participants =
      fixture_person_query_exact_collect_result_slot_v1_t_page(result, 1u);
  size_t participant_count = 0u;
  size_t participant_index;
  size_t row_count = 0u;
  CHECK(out_origin != NULL && out_participants != NULL &&
        out_participant_count != NULL);
  *out_participant_count = 0u;
  CHECK(fixture_query_result_row_count(fixture_query_page_result_ref(result),
                                       &row_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 1u);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_page(result, 0u),
            0u, &origin, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_query_person_identifier(origin, "query-ada", out_diagnostics) ==
        0);
  CHECK(read_query_person_identifier(origin, out_origin, origin_capacity,
                                     out_diagnostics) == 0);
  CHECK(fixture_person_close(&origin) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_collect_count(
            participants, 0u, &participant_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(participant_count == 2u);
  for (participant_index = 0u; participant_index < participant_count;
       ++participant_index) {
    fixture_person *participant = NULL;
    CHECK(fixture_person_query_exact_collect_at(
              participants, 0u, participant_index, &participant,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_query_person_identifier(participant,
                                        expected_participants[participant_index],
                                        out_diagnostics) == 0);
    CHECK(read_query_person_identifier(
              participant, out_participants[participant_index],
              sizeof(out_participants[participant_index]), out_diagnostics) ==
          0);
    CHECK(fixture_person_close(&participant) == TYPE_BRIDGE_STATUS_OK);
  }
  *out_participant_count = participant_count;
  return 0;
}

static int check_sdk_reducers(
    fixture_query_reduction_result_ref_v1_t result,
    sdk_reducer_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  const int64_t expected_longs[3] = {83, 38, 45};
  const uint64_t expected_doubles[3] = {
      UINT64_C(0x4044c00000000000), UINT64_C(0x4044c00000000000),
      UINT64_C(0x4013cc8a99af5453)};
  type_bridge_query_reduction_group_kind_t group_kind = 0u;
  type_bridge_query_reduced_value_metadata_v1_t metadata = {0};
  uint64_t count = 0u;
  size_t row_count = 0u;
  size_t index;

  CHECK(observation != NULL);
  CHECK(fixture_query_reduction_result_row_count(
            result, &row_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 1u);
  CHECK(fixture_query_reduction_group_kind(
            result, 0u, &group_kind, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(group_kind == TYPE_BRIDGE_QUERY_REDUCTION_GROUP_NONE);
  CHECK(fixture_query_reduced_count_value(
            fixture_query_reduced_count_slot(result, 0u), 0u, &count,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 2u);
  observation->count = count;
  for (index = 0u; index < 3u; ++index) {
    int64_t value = 0;
    fixture_query_reduced_long_slot_v1_t slot =
        fixture_query_reduced_long_slot(result, index + 1u);
    CHECK(fixture_query_reduced_long_metadata(
              slot, 0u, &metadata, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(metadata.kind == TYPE_BRIDGE_QUERY_REDUCED_LONG &&
          metadata.present == 1u);
    CHECK(fixture_query_reduced_long_value(
              slot, 0u, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(value == expected_longs[index]);
    if (index == 0u) {
      observation->sum = value;
    } else if (index == 1u) {
      observation->minimum = value;
    } else {
      observation->maximum = value;
    }
  }
  for (index = 0u; index < 3u; ++index) {
    uint64_t bits = 0u;
    fixture_query_reduced_double_slot_v1_t slot =
        fixture_query_reduced_double_slot(result, index + 4u);
    CHECK(fixture_query_reduced_double_metadata(
              slot, 0u, &metadata, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(metadata.kind == TYPE_BRIDGE_QUERY_REDUCED_DOUBLE &&
          metadata.present == 1u);
    CHECK(fixture_query_reduced_double_bits(
              slot, 0u, &bits, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(bits == expected_doubles[index]);
    if (index == 0u) {
      observation->mean_bits = bits;
    } else if (index == 1u) {
      observation->median_bits = bits;
    } else {
      observation->standard_deviation_bits = bits;
    }
  }
  return 0;
}

static int check_sdk_binding_groups(
    fixture_query_reduction_result_ref_v1_t result,
    sdk_reducer_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  size_t row_count = 0u;
  size_t row_index;
  int seen_ada = 0;
  int seen_dana = 0;

  CHECK(observation != NULL);
  CHECK(fixture_query_reduction_result_row_count(
            result, &row_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 2u);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_person *person = NULL;
    fixture_identifier *identifier = NULL;
    type_bridge_byte_view_t text = {NULL, 0u};
    type_bridge_query_reduction_group_kind_t group_kind = 0u;
    uint64_t count = 0u;
    CHECK(fixture_query_reduction_group_kind(
              result, row_index, &group_kind, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(group_kind == TYPE_BRIDGE_QUERY_REDUCTION_GROUP_THING);
    CHECK(fixture_person_query_exact_reduction_group_slot_v1_t_thing_at(
              fixture_person_query_exact_reduction_group_slot_v1_t_thing(
                  result),
              row_index, &person, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier(person, &identifier, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    seen_ada += same_text(text, "query-ada");
    seen_dana += same_text(text, "query-dana");
    CHECK(fixture_query_reduced_count_value(
              fixture_query_reduced_count_slot(result, 0u), row_index, &count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 1u);
    if (same_text(text, "query-ada")) {
      CHECK(copy_stable_text(text, observation->binding_keys[0],
                             sizeof(observation->binding_keys[0])));
      observation->binding_counts[0] = count;
    } else {
      CHECK(same_text(text, "query-dana"));
      CHECK(copy_stable_text(text, observation->binding_keys[1],
                             sizeof(observation->binding_keys[1])));
      observation->binding_counts[1] = count;
    }
    CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(seen_ada == 1 && seen_dana == 1);
  return 0;
}

static int check_sdk_field_groups(
    fixture_query_reduction_result_ref_v1_t result,
    sdk_reducer_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  size_t row_count = 0u;
  size_t row_index;
  int seen_38 = 0;
  int seen_45 = 0;

  CHECK(observation != NULL);
  CHECK(fixture_query_reduction_result_row_count(
            result, &row_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 2u);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_score *score = NULL;
    type_bridge_query_reduction_group_kind_t group_kind = 0u;
    int64_t value = 0;
    uint64_t count = 0u;
    CHECK(fixture_query_reduction_group_kind(
              result, row_index, &group_kind, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(group_kind == TYPE_BRIDGE_QUERY_REDUCTION_GROUP_FIELD);
    CHECK(fixture_person_score_query_field_reduction_group_slot_v1_t_value(
              fixture_person_score_query_field_reduction_group_slot_v1_t_from_result(
                  result, 0u),
              row_index, &score, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_score_value(score, &value, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    seen_38 += value == 38;
    seen_45 += value == 45;
    CHECK(fixture_query_reduced_count_value(
              fixture_query_reduced_count_slot(result, 0u), row_index, &count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 1u);
    if (value == 38) {
      observation->field_keys[0] = value;
      observation->field_counts[0] = count;
    } else {
      CHECK(value == 45);
      observation->field_keys[1] = value;
      observation->field_counts[1] = count;
    }
    CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(seen_38 == 1 && seen_45 == 1);
  return 0;
}

static int check_sdk_field_tuple_groups(
    fixture_query_reduction_result_ref_v1_t result,
    sdk_reducer_observation_t *observation,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  size_t row_count = 0u;
  size_t row_index;
  int seen_38 = 0;
  int seen_45 = 0;

  CHECK(observation != NULL);
  CHECK(fixture_query_reduction_result_row_count(
            result, &row_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(row_count == 2u);
  for (row_index = 0u; row_index < row_count; ++row_index) {
    fixture_score *score = NULL;
    fixture_scorezuzugte *score_gte = NULL;
    type_bridge_query_reduction_group_kind_t group_kind = 0u;
    size_t field_count = 0u;
    int64_t score_value = 0;
    int64_t score_gte_value = 0;
    uint64_t count = 0u;
    CHECK(fixture_query_reduction_group_kind(
              result, row_index, &group_kind, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(group_kind == TYPE_BRIDGE_QUERY_REDUCTION_GROUP_FIELDS);
    CHECK(fixture_query_reduction_group_field_count(
              result, row_index, &field_count, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(field_count == 2u);
    CHECK(fixture_person_score_query_field_reduction_group_slot_v1_t_value(
              fixture_person_score_query_field_reduction_group_slot_v1_t_from_result(
                  result, 0u),
              row_index, &score, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_scorezuzugte_query_field_reduction_group_slot_v1_t_value(
              fixture_person_scorezuzugte_query_field_reduction_group_slot_v1_t_from_result(
                  result, 1u),
              row_index, &score_gte, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_score_value(score, &score_value, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_scorezuzugte_value(score_gte, &score_gte_value,
                                     out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(score_gte_value == 40);
    seen_38 += score_value == 38;
    seen_45 += score_value == 45;
    CHECK(fixture_query_reduced_count_value(
              fixture_query_reduced_count_slot(result, 0u), row_index, &count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 1u);
    if (score_value == 38) {
      observation->tuple_keys[0][0] = score_value;
      observation->tuple_keys[0][1] = score_gte_value;
      observation->tuple_counts[0] = count;
    } else {
      CHECK(score_value == 45);
      observation->tuple_keys[1][0] = score_value;
      observation->tuple_keys[1][1] = score_gte_value;
      observation->tuple_counts[1] = count;
    }
    CHECK(fixture_scorezuzugte_close(&score_gte) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(seen_38 == 1 && seen_45 == 1);
  return 0;
}

static int run_typed_query_function_and_remote(
    const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database,
    type_bridge_byte_view_t ada_iid, type_bridge_byte_view_t dana_iid,
    type_bridge_byte_view_t membership_iid,
    type_bridge_byte_view_t network_link_iid,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  const int64_t minimum_number = 30;
  const int64_t threshold_number = 40;
  const type_bridge_query_comparison_t threshold_comparison =
      TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL;
  const uint16_t remote_port = required_remote_port();
  fixture_query_session *session = NULL;
  fixture_person_query_exact_binding *person_binding = NULL;
  fixture_person_identifier_query_field *identifier_field = NULL;
  fixture_person_nickname_query_field *nickname_field = NULL;
  fixture_person_score_query_field *score_field = NULL;
  fixture_person_query_exact_one_selection *selection = NULL;
  fixture_qualifyingzhscore *function = NULL;
  fixture_qualifyingzhscore_query_call *call = NULL;
  fixture_query_function_integer_input *minimum_input = NULL;
  fixture_query_predicate *predicate = NULL;
  fixture_query_predicate *iid_predicate = NULL;
  fixture_query_predicate *filtered_predicate = NULL;
  fixture_query_predicate *threshold_predicate = NULL;
  fixture_query_predicate *dana_predicate = NULL;
  fixture_query_predicate *nickname_present = NULL;
  fixture_query_predicate *nickname_scoped_predicate = NULL;
  fixture_query_predicate *score_present = NULL;
  fixture_query_predicate *owner_iid_predicate = NULL;
  fixture_query *query = NULL;
  fixture_query *filtered = NULL;
  fixture_query *sdk_query = NULL;
  fixture_query *dana_query = NULL;
  fixture_query *nickname_query = NULL;
  fixture_query_rows_terminal *terminal = NULL;
  fixture_query_rows_terminal *one_terminal = NULL;
  fixture_query_first_terminal *first_terminal = NULL;
  fixture_query_rows_terminal *rows_terminal = NULL;
  fixture_query_rows_terminal *nickname_terminal = NULL;
  fixture_query_page_terminal *page_terminal = NULL;
  fixture_query_count_terminal *count_terminal = NULL;
  fixture_query_exists_terminal *exists_terminal = NULL;
  fixture_query_rows_result *direct_result = NULL;
  fixture_query_rows_result *remote_result = NULL;
  fixture_query_rows_result *one_result = NULL;
  fixture_query_rows_result *first_result = NULL;
  fixture_query_rows_result *rows_result = NULL;
  fixture_query_rows_result *nickname_result = NULL;
  fixture_query_rows_result *read_rows_result = NULL;
  fixture_query_page_result *page_result = NULL;
  fixture_query_count_result *count_result = NULL;
  fixture_query_exists_result *exists_result = NULL;
  fixture_query_remote_context *remote_context = NULL;
  fixture_query_rows_remote_pending *pending = NULL;
  fixture_query_rows_remote_claim *claim = NULL;
  fixture_score *minimum = NULL;
  fixture_score *threshold = NULL;
  fixture_query_order *identifier_order = NULL;
  const fixture_query_order *orders[1];
  const type_bridge_byte_view_t sdk_iids[2] = {ada_iid, dana_iid};
  const char *const expected_rows[2] = {"query-ada", "query-dana"};
  const char *const expected_dana[1] = {"query-dana"};
  const char *const expected_ada[1] = {"query-ada"};
  caller_http_body_t advertisement = {NULL, 0u};
  caller_http_body_t response = {NULL, 0u};
  type_bridge_byte_view_t request = {NULL, 0u};
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  fixture_qualifyingzhscore_query_arguments_v1_t arguments = {0};
  size_t response_limit = 0u;
  size_t remote_exchange_count = 0u;
  size_t remote_one_exchange_count = 0u;
  size_t zero_limit_dimension_count = 0u;
  size_t plus_one_limit_dimension_count = 0u;
  uint64_t count_value = 0u;
  uint64_t direct_terminal_count = 0u;
  uint64_t remote_terminal_count = 0u;
  uint8_t exists_value = 0u;
  uint8_t direct_terminal_exists = 0u;
  uint8_t remote_terminal_exists = 0u;
  type_bridge_query_page_metadata_v1_t page_metadata = {0};
  type_bridge_query_page_metadata_v1_t direct_page_metadata = {0};
  type_bridge_query_page_metadata_v1_t remote_page_metadata = {0};
  type_bridge_read_transaction_t *read_transaction = NULL;
  sdk_person_rows_observation_t direct_function_rows = {0};
  sdk_person_rows_observation_t remote_function_rows = {0};
  sdk_person_rows_observation_t direct_nested_function_rows = {0};
  sdk_person_rows_observation_t remote_nested_function_rows = {0};
  sdk_person_rows_observation_t direct_sdk_rows = {0};
  sdk_person_rows_observation_t remote_sdk_rows = {0};
  sdk_person_rows_observation_t direct_nickname_rows = {0};
  sdk_person_rows_observation_t remote_nickname_rows = {0};
  sdk_person_rows_observation_t direct_first_rows = {0};
  sdk_person_rows_observation_t remote_first_rows = {0};
  sdk_person_rows_observation_t direct_one_rows = {0};
  sdk_person_rows_observation_t remote_one_rows = {0};
  sdk_person_rows_observation_t direct_page_rows = {0};
  sdk_person_rows_observation_t remote_page_rows = {0};
  sdk_reducer_observation_t direct_reducers = {0};
  sdk_reducer_observation_t remote_reducers = {0};
  sdk_resource_limit_observation_t resource_limit_observation = {0};
  sdk_topology_observation_t direct_topology = {0};
  sdk_topology_observation_t remote_topology = {0};
  int remote_representative_zero_limit = 0;
  int remote_plus_one_clamped_all = 0;
  char direct_membership_member_key[64] = {0};
  char remote_membership_member_key[64] = {0};
  char direct_topology_source_key[64] = {0};
  char direct_topology_target_key[64] = {0};
  char remote_topology_source_key[64] = {0};
  char remote_topology_target_key[64] = {0};

  (void)network_link_iid;

  CHECK(remote_port != 0u);
  CHECK(fixture_query_session_open(package, &session, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_binding_open(
            session, &person_binding, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_score_query_field_from_exact(
            person_binding, &score_field, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier_query_field_from_exact(
            person_binding, &identifier_field, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_nickname_query_field_from_exact(
            person_binding, &nickname_field, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_nickname_query_field_presence(
            nickname_field, 1u, &nickname_present, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_score_query_field_presence(
            score_field, 1u, &score_present, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier_query_field_order(
            identifier_field, TYPE_BRIDGE_QUERY_SORT_ASCENDING,
            TYPE_BRIDGE_QUERY_MISSING_REJECT, &identifier_order,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  orders[0] = identifier_order;
  CHECK(fixture_person_query_exact_one_selection_open(
            person_binding, &selection, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_open(package, minimum_number, &minimum,
                           out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_query_function_input_open(
            session, minimum, &minimum_input, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_qualifyingzhscore_query_open(session, &function,
                                             out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  arguments.header.struct_size = sizeof(arguments);
  arguments.header.version = TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION;
  arguments.argument_person =
      fixture_person_query_function_argument_v1_t_from_person_exact(
          person_binding);
  arguments.argument_minimum =
      fixture_query_function_integer_argument_from_input(minimum_input);
  CHECK(fixture_qualifyingzhscore_query_call_open(
            function, &arguments, &call, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_qualifyingzhscore_query_call_equal_field(
            call,
            fixture_person_score_query_field_function_field(score_field),
            &predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_binding_iid_in(
            person_binding, sdk_iids, 2u, &iid_predicate,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_and(
            predicate, iid_predicate, &filtered_predicate,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_positional_1(
            session,
            fixture_person_query_exact_one_selection_ref(selection), &query,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_and(
            nickname_present, iid_predicate, &nickname_scoped_predicate,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_where(query, nickname_scoped_predicate,
                            &nickname_query, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows(nickname_query, orders, 1u, 0u, 2u,
                           &nickname_terminal, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_where(query, filtered_predicate, &filtered,
                            out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows(filtered, orders, 1u, 0u, 16u, &terminal,
                           out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_database_query_execute_rows(
            database, terminal, &limits, NULL, &direct_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_rows_result(direct_result, 2u, minimum_number,
                          out_diagnostics) == 0);
  CHECK(observe_person_rows(direct_result, &direct_function_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_database_query_execute_rows(
            database, nickname_terminal, &limits, NULL, &nickname_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_rows_identifiers(nickname_result, expected_ada, 1u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(nickname_result, &direct_nickname_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_query_rows_result_close(&nickname_result) ==
        TYPE_BRIDGE_STATUS_OK);
  puts("C18 direct typed query and G02 schema function: passed");

  CHECK(fixture_score_open(package, threshold_number, &threshold,
                           out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_score_query_field_compare_value(
            score_field, threshold_comparison, threshold, &threshold_predicate,
            out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_and(iid_predicate, threshold_predicate,
                                    &dana_predicate, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_and(iid_predicate, score_present,
                                    &owner_iid_predicate, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_where(query, owner_iid_predicate, &sdk_query,
                            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_where(query, dana_predicate, &dana_query,
                            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_one(dana_query, orders, 1u, &one_terminal,
                          out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_first(sdk_query, orders, 1u, &first_terminal,
                            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows(sdk_query, orders, 1u, 0u, 2u,
                           &rows_terminal, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  {
    fixture_query_root_v1_t root =
        fixture_person_query_exact_binding_root(sdk_query,
                                                person_binding);
    CHECK(fixture_query_page(root, orders, 1u, 0u, 1u, 1u, &page_terminal,
                             out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count(root, &count_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_exists(root, &exists_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_database_query_execute_rows(
            database, one_terminal, &limits, NULL, &one_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_rows_identifiers(one_result, expected_dana, 1u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(one_result, &direct_one_rows, out_diagnostics) ==
        0);
  CHECK(fixture_database_query_execute_first(
            database, first_terminal, &limits, NULL, &first_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_rows_identifiers(first_result, expected_ada, 1u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(first_result, &direct_first_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_database_query_execute_rows(
            database, rows_terminal, &limits, NULL, &rows_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_rows_identifiers(rows_result, expected_rows, 2u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(rows_result, &direct_sdk_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_database_query_execute_page(
            database, page_terminal, &limits, NULL, &page_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_page_metadata(page_result, &page_metadata,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(page_metadata.offset == 0u && page_metadata.limit == 1u &&
        page_metadata.has_total == 1u && page_metadata.total == 2u);
  direct_page_metadata = page_metadata;
  {
    fixture_person_query_exact_one_result_slot_v1_t page_slot =
        fixture_person_query_exact_one_result_slot_v1_t_page(page_result, 0u);
    fixture_person *page_person = NULL;
    CHECK(fixture_person_query_exact_one_at(page_slot, 0u, &page_person,
                                            out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_query_person_identifier(page_person, "query-ada",
                                        out_diagnostics) == 0);
    CHECK(read_query_person_identifier(page_person, direct_page_rows.keys[0],
                                       sizeof(direct_page_rows.keys[0]),
                                       out_diagnostics) == 0);
    CHECK(read_query_person_score(page_person, &direct_page_rows.scores[0],
                                  out_diagnostics) == 0);
    direct_page_rows.count = 1u;
    CHECK(fixture_person_close(&page_person) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_database_query_execute_count(
            database, count_terminal, &limits, NULL, &count_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_count_result_value(count_result, &count_value,
                                         out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count_value == 2u);
  direct_terminal_count = count_value;
  CHECK(fixture_database_query_execute_exists(
            database, exists_terminal, &limits, NULL, &exists_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_exists_result_value(exists_result, &exists_value,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(exists_value == 1u);
  direct_terminal_exists = exists_value;
  puts("C13/C15/C18/C19 direct one/first/rows/page/count/exists: passed");
  CHECK(fixture_query_count_result_close(&count_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_exists_result_close(&exists_result) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(type_bridge_read_transaction_open(database, NULL, &read_transaction,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_read_transaction_query_execute_rows(
            read_transaction, rows_terminal, &limits, NULL,
            &read_rows_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_rows_identifiers(read_rows_result, expected_rows, 2u,
                               out_diagnostics) == 0);
  CHECK(fixture_query_rows_result_close(&read_rows_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_read_transaction_query_execute_count(
            read_transaction, count_terminal, &limits, NULL, &count_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_count_result_value(count_result, &count_value,
                                         out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count_value == 2u);
  CHECK(fixture_query_count_result_close(&count_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_read_transaction_query_execute_exists(
            read_transaction, exists_terminal, &limits, NULL, &exists_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_exists_result_value(exists_result, &exists_value,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(exists_value == 1u);
  CHECK(fixture_query_exists_result_close(&exists_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_read_transaction_close(&read_transaction,
                                           out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  puts("C19 borrowed read transaction query reuse: passed");
  CHECK(fixture_query_page_result_close(&page_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&rows_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&first_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&one_result) == TYPE_BRIDGE_STATUS_OK);

  CHECK(caller_http_exchange(remote_port, "GET", "/v2/capabilities", NULL,
                             0u, 1024u * 1024u, &advertisement));
  CHECK(fixture_query_remote_context_open(
            package,
            (type_bridge_byte_view_t){advertisement.data,
                                      advertisement.length},
            &limits, &remote_context, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  free(advertisement.data);
  advertisement.data = NULL;
  advertisement.length = 0u;
  CHECK(fixture_query_remote_prepare_rows(
            remote_context, terminal, NULL, &pending, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_pending_request_bytes(pending, &request) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(request.data != NULL && request.length != 0u);
  CHECK(fixture_query_rows_remote_pending_response_snapshot_limit(
            pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);
  CHECK(response_limit != 0u);
  /* Exactly one caller-owned terminal request/response exchange. */
  CHECK(caller_http_exchange(remote_port, "POST", "/v2/query", request.data,
                             request.length, response_limit, &response));
  ++remote_exchange_count;
  CHECK(fixture_query_rows_remote_pending_claim(
            pending, NULL, &claim, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_pending_close(&pending) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_claim_decode(
            claim, NULL,
            (type_bridge_byte_view_t){response.data, response.length},
            &remote_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  free(response.data);
  response.data = NULL;
  response.length = 0u;
  CHECK(check_rows_result(remote_result, 2u, minimum_number,
                          out_diagnostics) == 0);
  CHECK(observe_person_rows(remote_result, &remote_function_rows,
                            out_diagnostics) == 0);
  puts("C22 caller-transport remote hydrated rows: passed");

  CHECK(fixture_query_rows_remote_claim_close(&claim) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&remote_result) ==
        TYPE_BRIDGE_STATUS_OK);

#define RUN_REMOTE_QUERY(family, terminal_value, out_result_value)              \
  do {                                                                          \
    fixture_query_##family##_remote_pending *round_pending = NULL;              \
    fixture_query_##family##_remote_claim *round_claim = NULL;                  \
    request.data = NULL;                                                        \
    request.length = 0u;                                                        \
    response_limit = 0u;                                                        \
    CHECK(fixture_query_remote_prepare_##family(                                \
              remote_context, terminal_value, NULL, &round_pending,             \
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);                       \
    CHECK(fixture_query_##family##_remote_pending_request_bytes(                \
              round_pending, &request) == TYPE_BRIDGE_STATUS_OK);               \
    CHECK(request.data != NULL && request.length != 0u);                         \
    CHECK(fixture_query_##family##_remote_pending_response_snapshot_limit(      \
              round_pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);         \
    CHECK(response_limit != 0u);                                                 \
    CHECK(caller_http_exchange(remote_port, "POST", "/v2/query",             \
                               request.data, request.length, response_limit,     \
                               &response));                                      \
    ++remote_exchange_count;                                                     \
    CHECK(fixture_query_##family##_remote_pending_claim(                        \
              round_pending, NULL, &round_claim, out_diagnostics) ==            \
          TYPE_BRIDGE_STATUS_OK);                                                \
    CHECK(fixture_query_##family##_remote_pending_close(&round_pending) ==       \
          TYPE_BRIDGE_STATUS_OK);                                                \
    CHECK(fixture_query_##family##_remote_claim_decode(                         \
              round_claim, NULL,                                                \
              (type_bridge_byte_view_t){response.data, response.length},         \
              &(out_result_value), out_diagnostics) == TYPE_BRIDGE_STATUS_OK);   \
    free(response.data);                                                         \
    response.data = NULL;                                                        \
    response.length = 0u;                                                        \
    CHECK(fixture_query_##family##_remote_claim_close(&round_claim) ==           \
          TYPE_BRIDGE_STATUS_OK);                                                \
  } while (0)

  {
    const size_t before = remote_exchange_count;
    RUN_REMOTE_QUERY(rows, one_terminal, one_result);
    CHECK(remote_exchange_count == before + 1u);
    remote_one_exchange_count = remote_exchange_count - before;
    CHECK(check_rows_identifiers(one_result, expected_dana, 1u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(one_result, &remote_one_rows, out_diagnostics) ==
          0);
    CHECK(fixture_query_rows_result_close(&one_result) == TYPE_BRIDGE_STATUS_OK);
    puts("C21 caller-transport remote one terminal uses one exchange: passed");
  }
  RUN_REMOTE_QUERY(first, first_terminal, first_result);
  CHECK(check_rows_identifiers(first_result, expected_ada, 1u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(first_result, &remote_first_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_query_rows_result_close(&first_result) == TYPE_BRIDGE_STATUS_OK);
  RUN_REMOTE_QUERY(rows, rows_terminal, rows_result);
  CHECK(check_rows_identifiers(rows_result, expected_rows, 2u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(rows_result, &remote_sdk_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_query_rows_result_close(&rows_result) == TYPE_BRIDGE_STATUS_OK);
  RUN_REMOTE_QUERY(page, page_terminal, page_result);
  CHECK(fixture_query_page_metadata(page_result, &page_metadata,
                                    out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(page_metadata.offset == 0u && page_metadata.limit == 1u &&
        page_metadata.has_total == 1u && page_metadata.total == 2u);
  remote_page_metadata = page_metadata;
  {
    fixture_person_query_exact_one_result_slot_v1_t page_slot =
        fixture_person_query_exact_one_result_slot_v1_t_page(page_result, 0u);
    fixture_person *page_person = NULL;
    CHECK(fixture_person_query_exact_one_at(page_slot, 0u, &page_person,
                                            out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_query_person_identifier(page_person, "query-ada",
                                        out_diagnostics) == 0);
    CHECK(read_query_person_identifier(page_person, remote_page_rows.keys[0],
                                       sizeof(remote_page_rows.keys[0]),
                                       out_diagnostics) == 0);
    CHECK(read_query_person_score(page_person, &remote_page_rows.scores[0],
                                  out_diagnostics) == 0);
    remote_page_rows.count = 1u;
    CHECK(fixture_person_close(&page_person) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_query_page_result_close(&page_result) == TYPE_BRIDGE_STATUS_OK);
  RUN_REMOTE_QUERY(count, count_terminal, count_result);
  CHECK(fixture_query_count_result_value(count_result, &count_value,
                                         out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count_value == 2u);
  remote_terminal_count = count_value;
  CHECK(fixture_query_count_result_close(&count_result) ==
        TYPE_BRIDGE_STATUS_OK);
  RUN_REMOTE_QUERY(exists, exists_terminal, exists_result);
  CHECK(fixture_query_exists_result_value(exists_result, &exists_value,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(exists_value == 1u);
  remote_terminal_exists = exists_value;
  CHECK(fixture_query_exists_result_close(&exists_result) ==
        TYPE_BRIDGE_STATUS_OK);
  RUN_REMOTE_QUERY(rows, nickname_terminal, nickname_result);
  CHECK(check_rows_identifiers(nickname_result, expected_ada, 1u,
                               out_diagnostics) == 0);
  CHECK(observe_person_rows(nickname_result, &remote_nickname_rows,
                            out_diagnostics) == 0);
  CHECK(fixture_query_rows_result_close(&nickname_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(emit_model_values_observation("direct_runtime",
                                      &direct_function_rows));
  CHECK(emit_model_values_observation("remote_runtime",
                                      &remote_function_rows));
  CHECK(emit_owner_iid_observation("direct_runtime", &direct_sdk_rows,
                                   &direct_nickname_rows));
  CHECK(emit_owner_iid_observation("remote_runtime", &remote_sdk_rows,
                                   &remote_nickname_rows));
  CHECK(emit_terminals_observation(
      "direct_runtime", direct_terminal_count, direct_terminal_exists,
      direct_first_rows.keys[0],
      direct_one_rows.keys[0], &direct_page_rows, &direct_sdk_rows,
      direct_page_metadata.offset, direct_page_metadata.limit,
      direct_page_metadata.total));
  CHECK(emit_terminals_observation(
      "remote_runtime", remote_terminal_count, remote_terminal_exists,
      remote_first_rows.keys[0],
      remote_one_rows.keys[0], &remote_page_rows, &remote_sdk_rows,
      remote_page_metadata.offset, remote_page_metadata.limit,
      remote_page_metadata.total));
  CHECK(emit_v2_observationf(
      "remote_one_exchange", "remote_runtime",
      "{\"exchange_count\":%zu,\"terminal\":\"one\"}",
      remote_one_exchange_count));
  puts("C13 owner/IID-set predicates direct/remote: passed");
  puts("C19 ordered/windowed terminal parity direct/remote: passed");
  puts("C28 scalar-domain comparison direct/remote: passed");

  {
    fixture_qualifyingzhscore_query_arguments_v1_t nested_arguments = {0};
    fixture_qualifyingzhscore_query_call *nested_call = NULL;
    fixture_query_predicate *nested_predicate = NULL;
    fixture_query_predicate *nested_filtered_predicate = NULL;
    fixture_query *nested_query = NULL;
    fixture_query_rows_terminal *nested_terminal = NULL;
    fixture_query_rows_result *nested_result = NULL;

    nested_arguments.header.struct_size = sizeof(nested_arguments);
    nested_arguments.header.version = TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION;
    nested_arguments.argument_person =
        fixture_person_query_function_argument_v1_t_from_person_exact(
            person_binding);
    nested_arguments.argument_minimum =
        fixture_query_function_integer_argument_from_call(
            fixture_qualifyingzhscore_query_call_ref(call));
    CHECK(fixture_qualifyingzhscore_query_call_open(
              function, &nested_arguments, &nested_call, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_qualifyingzhscore_query_call_equal_field(
              nested_call,
              fixture_person_score_query_field_function_field(score_field),
              &nested_predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              nested_predicate, iid_predicate, &nested_filtered_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(query, nested_filtered_predicate, &nested_query,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(nested_query, orders, 1u, 0u, 16u,
                             &nested_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, nested_terminal, &limits, NULL, &nested_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_rows_result(nested_result, 2u, minimum_number,
                            out_diagnostics) == 0);
    CHECK(observe_person_rows(nested_result, &direct_nested_function_rows,
                              out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&nested_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, nested_terminal, nested_result);
    CHECK(check_rows_result(nested_result, 2u, minimum_number,
                            out_diagnostics) == 0);
    CHECK(observe_person_rows(nested_result, &remote_nested_function_rows,
                              out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&nested_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(emit_schema_function_observation(
        "direct_runtime", minimum_number, &direct_function_rows,
        &direct_nested_function_rows));
    CHECK(emit_schema_function_observation(
        "remote_runtime", minimum_number, &remote_function_rows,
        &remote_nested_function_rows));
    puts("G02 nested schema-function call direct/remote: passed");

    CHECK(fixture_query_rows_terminal_close(&nested_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&nested_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&nested_filtered_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&nested_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_qualifyingzhscore_query_call_close(&nested_call) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  {
    fixture_query_count_remote_pending *count_pending = NULL;
    fixture_query_count_remote_claim *count_claim = NULL;
    fixture_query_rows_result *wrong_rows_result = NULL;
    fixture_query_count_result *reused_count_result = NULL;

    CHECK(fixture_query_remote_prepare_count(
              remote_context, count_terminal, NULL, &count_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_request_bytes(
              count_pending, &request) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_response_snapshot_limit(
              count_pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);
    CHECK(caller_http_exchange(remote_port, "POST", "/v2/query",
                               request.data, request.length, response_limit,
                               &response));
    ++remote_exchange_count;
    CHECK(fixture_query_count_remote_pending_claim(
              count_pending, NULL, &count_claim, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_close(&count_pending) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_remote_claim_decode(
              (const fixture_query_rows_remote_claim *)count_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &wrong_rows_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(wrong_rows_result == NULL);
    CHECK(execution_code_is(*out_diagnostics,
                            "c_query_nominal_contract_mismatch"));
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_decode(
              count_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &reused_count_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_result_value(reused_count_result, &count_value,
                                           out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count_value == 2u);
    free(response.data);
    response.data = NULL;
    response.length = 0u;
    CHECK(fixture_query_count_result_close(&reused_count_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_close(&count_claim) ==
          TYPE_BRIDGE_STATUS_OK);
    puts("C23 remote wrong-family cast rejected and claim reusable: passed");
  }

  {
    fixture_query_rows_terminal *not_unique_terminal = NULL;
    fixture_query_rows_result *not_unique_result = NULL;
    fixture_query_rows_remote_pending *failure_pending = NULL;
    fixture_query_rows_remote_claim *failure_claim = NULL;
    sdk_not_unique_observation_t direct_not_unique = {0};
    sdk_not_unique_observation_t remote_not_unique = {0};

    CHECK(fixture_query_one(sdk_query, orders, 1u,
                            &not_unique_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, not_unique_terminal, &limits, NULL,
              &not_unique_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(not_unique_result == NULL);
    CHECK(check_not_unique_diagnostic(*out_diagnostics,
                                      &direct_not_unique) == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_remote_prepare_rows(
              remote_context, not_unique_terminal, NULL, &failure_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_remote_pending_request_bytes(
              failure_pending, &request) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_remote_pending_response_snapshot_limit(
              failure_pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);
    CHECK(caller_http_exchange(remote_port, "POST", "/v2/query", request.data,
                               request.length, response_limit, &response));
    ++remote_exchange_count;
    CHECK(fixture_query_rows_remote_pending_claim(
              failure_pending, NULL, &failure_claim, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_remote_pending_close(&failure_pending) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_remote_claim_decode(
              failure_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &not_unique_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(not_unique_result == NULL);
    CHECK(check_not_unique_diagnostic(*out_diagnostics,
                                      &remote_not_unique) == 0);
    CHECK(memcmp(&direct_not_unique, &remote_not_unique,
                 sizeof(direct_not_unique)) == 0);
    CHECK(emit_structured_query_diagnostic_observation(&direct_not_unique));
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_remote_claim_decode(
              failure_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &not_unique_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
    CHECK(not_unique_result == NULL);
    CHECK(execution_code_is(*out_diagnostics,
                            "c_query_remote_claim_consumed"));
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    free(response.data);
    response.data = NULL;
    response.length = 0u;
    CHECK(fixture_query_rows_remote_claim_close(&failure_claim) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&not_unique_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    puts("C23 structured cardinality failure direct/remote and one-shot claim: passed");
  }

  {
    type_bridge_cancellation_t *cancellation = NULL;
    fixture_query_rows_result *rejected_result = NULL;
    fixture_query_page_result *rejected_page_result = NULL;
    fixture_query_rows_remote_pending *rejected_pending = NULL;
    fixture_query_remote_context *zero_context = NULL;
    fixture_query_remote_context *clamped_context = NULL;
    fixture_query_count_result *clamped_direct = NULL;
    fixture_query_count_result *clamped_remote = NULL;
    fixture_query_count_remote_pending *clamped_pending = NULL;
    fixture_query_count_remote_claim *clamped_claim = NULL;
    type_bridge_query_execution_limits_v1_t zero_limits[6];
    type_bridge_query_execution_limits_v1_t zero_remote_limits =
        TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
    type_bridge_query_execution_limits_v1_t plus_one_limits[8];
    type_bridge_query_execution_limits_v1_t clamped_limits =
        TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
    size_t limit_index;

    CHECK(limits.timeout_milliseconds ==
              TYPE_BRIDGE_QUERY_DEFAULT_TIMEOUT_MILLISECONDS &&
          limits.items == TYPE_BRIDGE_QUERY_DEFAULT_ITEMS &&
          limits.bytes == TYPE_BRIDGE_QUERY_DEFAULT_BYTES &&
          limits.graph_nodes == TYPE_BRIDGE_QUERY_DEFAULT_GRAPH_NODES &&
          limits.attribute_values ==
              TYPE_BRIDGE_QUERY_DEFAULT_ATTRIBUTE_VALUES &&
          limits.collection_members ==
              TYPE_BRIDGE_QUERY_DEFAULT_COLLECTION_MEMBERS &&
          limits.role_players == TYPE_BRIDGE_QUERY_DEFAULT_ROLE_PLAYERS &&
          limits.statements == TYPE_BRIDGE_QUERY_DEFAULT_STATEMENTS);
    for (limit_index = 0u; limit_index < 6u; ++limit_index) {
      zero_limits[limit_index] = limits;
    }
    zero_limits[0].timeout_milliseconds = 0u;
    zero_limits[1].items = 0u;
    zero_limits[2].bytes = 0u;
    zero_limits[3].graph_nodes = 0u;
    zero_limits[4].attribute_values = 0u;
    zero_limits[5].statements = 0u;
    for (limit_index = 0u; limit_index < 6u; ++limit_index) {
      CHECK(fixture_database_query_execute_rows(
                database, rows_terminal, &zero_limits[limit_index], NULL,
                &rejected_result, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
      CHECK(rejected_result == NULL);
      CHECK(check_structured_execution_diagnostic(
                *out_diagnostics,
                TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
                limit_index == 0u ? "transaction_deadline_exceeded" : NULL) ==
            0);
      CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      ++zero_limit_dimension_count;
    }
    {
      type_bridge_query_execution_limits_v1_t collection_limits = limits;
      collection_limits.collection_members = 0u;
      CHECK(fixture_database_query_execute_page(
                database, page_terminal, &collection_limits, NULL,
                &rejected_page_result, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
      CHECK(rejected_page_result == NULL);
      CHECK(check_structured_execution_diagnostic(
                *out_diagnostics,
                TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT, NULL) == 0);
      CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      ++zero_limit_dimension_count;
    }

    CHECK(type_bridge_cancellation_open(&cancellation) == TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_cancellation_request(cancellation) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, rows_terminal, &limits, cancellation,
              &rejected_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_CANCELLED);
    CHECK(rejected_result == NULL);
    CHECK(check_structured_execution_diagnostic(
              *out_diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_CANCELLED,
              "provider_cancelled") == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_remote_prepare_rows(
              remote_context, rows_terminal, cancellation, &rejected_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_CANCELLED);
    CHECK(rejected_pending == NULL);
    CHECK(check_structured_execution_diagnostic(
              *out_diagnostics, TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_CANCELLED,
              "provider_cancelled") == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_cancellation_close(&cancellation) ==
          TYPE_BRIDGE_STATUS_OK);

    zero_remote_limits.timeout_milliseconds = 0u;
    CHECK(caller_http_exchange(remote_port, "GET", "/v2/capabilities", NULL,
                               0u, 1024u * 1024u, &advertisement));
    CHECK(fixture_query_remote_context_open(
              package,
              (type_bridge_byte_view_t){advertisement.data,
                                        advertisement.length},
              &zero_remote_limits, &zero_context, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    free(advertisement.data);
    advertisement.data = NULL;
    advertisement.length = 0u;
    CHECK(fixture_query_remote_prepare_rows(
              zero_context, rows_terminal, NULL, &rejected_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
    CHECK(rejected_pending == NULL);
    CHECK(check_structured_execution_diagnostic(
              *out_diagnostics,
              TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
              "transaction_deadline_exceeded") == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_remote_context_close(&zero_context) ==
          TYPE_BRIDGE_STATUS_OK);
    remote_representative_zero_limit = 1;

    for (limit_index = 0u; limit_index < 8u; ++limit_index) {
      plus_one_limits[limit_index] = limits;
    }
    plus_one_limits[0].timeout_milliseconds += 1u;
    plus_one_limits[1].items += 1u;
    plus_one_limits[2].bytes += 1u;
    plus_one_limits[3].graph_nodes += 1u;
    plus_one_limits[4].attribute_values += 1u;
    plus_one_limits[5].collection_members += 1u;
    plus_one_limits[6].role_players += 1u;
    plus_one_limits[7].statements += 1u;
    for (limit_index = 0u; limit_index < 8u; ++limit_index) {
      CHECK(fixture_database_query_execute_count(
                database, count_terminal, &plus_one_limits[limit_index], NULL,
                &clamped_direct, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_query_count_result_value(clamped_direct, &count_value,
                                             out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(count_value == 2u);
      CHECK(fixture_query_count_result_close(&clamped_direct) ==
            TYPE_BRIDGE_STATUS_OK);
      ++plus_one_limit_dimension_count;
    }
    clamped_limits.timeout_milliseconds += 1u;
    clamped_limits.items += 1u;
    clamped_limits.bytes += 1u;
    clamped_limits.graph_nodes += 1u;
    clamped_limits.attribute_values += 1u;
    clamped_limits.collection_members += 1u;
    clamped_limits.role_players += 1u;
    clamped_limits.statements += 1u;
    CHECK(fixture_database_query_execute_count(
              database, count_terminal, &clamped_limits, NULL,
              &clamped_direct, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_result_value(clamped_direct, &count_value,
                                           out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count_value == 2u);
    CHECK(fixture_query_count_result_close(&clamped_direct) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(caller_http_exchange(remote_port, "GET", "/v2/capabilities", NULL,
                               0u, 1024u * 1024u, &advertisement));
    CHECK(fixture_query_remote_context_open(
              package,
              (type_bridge_byte_view_t){advertisement.data,
                                        advertisement.length},
              &clamped_limits, &clamped_context, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    free(advertisement.data);
    advertisement.data = NULL;
    advertisement.length = 0u;
    CHECK(fixture_query_remote_prepare_count(
              clamped_context, count_terminal, NULL, &clamped_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_request_bytes(
              clamped_pending, &request) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_response_snapshot_limit(
              clamped_pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);
    CHECK(caller_http_exchange(remote_port, "POST", "/v2/query",
                               request.data, request.length, response_limit,
                               &response));
    ++remote_exchange_count;
    CHECK(fixture_query_count_remote_pending_claim(
              clamped_pending, NULL, &clamped_claim, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_close(&clamped_pending) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_decode(
              clamped_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &clamped_remote, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    free(response.data);
    response.data = NULL;
    response.length = 0u;
    CHECK(fixture_query_count_result_value(clamped_remote, &count_value,
                                           out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count_value == 2u);
    CHECK(fixture_query_count_result_close(&clamped_remote) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_close(&clamped_claim) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_remote_context_close(&clamped_context) ==
          TYPE_BRIDGE_STATUS_OK);
    remote_plus_one_clamped_all = 1;
    CHECK(plus_one_limit_dimension_count == 8u);
    puts("G05 pre-dispatch cancellation direct/remote: passed");
    puts("G06 all-eight plus-one clamp and representative remote limit: passed");
    puts("G08 live structured cancellation/resource diagnostics: passed");
  }

  {
    fixture_query *lineage = NULL;
    fixture_query *derived = NULL;
    fixture_query *sibling = NULL;
    fixture_query *session_probe = NULL;
    fixture_query *closed_query_attempt = NULL;
    fixture_query_rows_terminal *ancestor_terminal = NULL;
    fixture_query_rows_terminal *lifecycle_terminal = NULL;
    fixture_query_rows_result *ancestor_result = NULL;
    fixture_query_rows_result *lifecycle_result = NULL;
    fixture_query_count_remote_pending *closed_pending = NULL;
    fixture_query_count_remote_claim *closed_pending_claim = NULL;
    fixture_query_count_remote_pending *replay_pending = NULL;
    fixture_query_count_remote_claim *replay_claim = NULL;
    fixture_query_count_result *replay_result = NULL;
    fixture_query_count_result *replayed_result = NULL;
    const size_t exchanges_before_closed_pending = remote_exchange_count;
    size_t exchanges_before_closed_query = 0u;
    size_t post_close_query_io_count = 0u;
  sdk_query_lifecycle_observation_t lifecycle_observation = {0};

    CHECK(fixture_query_where(query, iid_predicate, &lineage,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(lineage, threshold_predicate, &derived,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(lineage, threshold_predicate, &sibling,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&derived) == TYPE_BRIDGE_STATUS_OK);
    CHECK(derived == NULL);
    CHECK(fixture_query_close(&derived) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(lineage, orders, 1u, 0u, 2u,
                             &ancestor_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, ancestor_terminal, &limits, NULL, &ancestor_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    lifecycle_observation.ancestor_usable_after_descendant_close = 1;
    CHECK(fixture_query_rows_terminal_close(&ancestor_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&lineage) == TYPE_BRIDGE_STATUS_OK);
    CHECK(lineage == NULL);
    lifecycle_observation.handle_invalidated = lineage == NULL;
    CHECK(fixture_query_close(&lineage) == TYPE_BRIDGE_STATUS_OK);
    lifecycle_observation.close_idempotent = 1;
    exchanges_before_closed_query = remote_exchange_count;
    CHECK(fixture_query_where(lineage, threshold_predicate,
                              &closed_query_attempt, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(closed_query_attempt == NULL && *out_diagnostics == NULL);
    post_close_query_io_count =
        remote_exchange_count - exchanges_before_closed_query;
    CHECK(post_close_query_io_count == 0u);
    lifecycle_observation.post_close_io_count = post_close_query_io_count;
    lifecycle_observation.post_close_rejected = closed_query_attempt == NULL;
    CHECK(check_rows_identifiers(ancestor_result, expected_rows, 2u,
                                 out_diagnostics) == 0);
    lifecycle_observation.result_usable_after_query_close = 1;
    CHECK(fixture_query_rows_result_close(&ancestor_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_result_close(&ancestor_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_1(
              session,
              fixture_person_query_exact_one_selection_ref(selection),
              &session_probe, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&session_probe) == TYPE_BRIDGE_STATUS_OK);
    lifecycle_observation.session_usable_after_query_close = 1;
    CHECK(fixture_query_rows(sibling, orders, 1u, 0u, 2u,
                             &lifecycle_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, lifecycle_terminal, &limits, NULL, &lifecycle_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_rows_identifiers(lifecycle_result, expected_dana, 1u,
                                 out_diagnostics) == 0);
    lifecycle_observation.descendant_usable_after_ancestor_close = 1;
    lifecycle_observation.sibling_usable = 1;
    CHECK(fixture_query_rows_result_close(&lifecycle_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_result_close(&lifecycle_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&lifecycle_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, lifecycle_terminal, &limits, NULL, &lifecycle_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(lifecycle_result == NULL && *out_diagnostics == NULL);
    CHECK(fixture_query_rows_terminal_close(&lifecycle_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&sibling) == TYPE_BRIDGE_STATUS_OK);
    CHECK(sibling == NULL);
    CHECK(fixture_query_close(&sibling) == TYPE_BRIDGE_STATUS_OK);
    lifecycle_observation.direct_lane_observed = 1;

    CHECK(fixture_query_remote_prepare_count(
              remote_context, count_terminal, NULL, &closed_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_close(&closed_pending) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_claim(
              closed_pending, NULL, &closed_pending_claim,
              out_diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    CHECK(closed_pending_claim == NULL && *out_diagnostics == NULL);
    CHECK(remote_exchange_count == exchanges_before_closed_pending);
    CHECK(fixture_query_count_remote_pending_close(&closed_pending) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_query_remote_prepare_count(
              remote_context, count_terminal, NULL, &replay_pending,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_request_bytes(
              replay_pending, &request) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_response_snapshot_limit(
              replay_pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);
    CHECK(caller_http_exchange(remote_port, "POST", "/v2/query",
                               request.data, request.length, response_limit,
                               &response));
    ++remote_exchange_count;
    CHECK(fixture_query_count_remote_pending_claim(
              replay_pending, NULL, &replay_claim, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_pending_close(&replay_pending) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_decode(
              replay_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &replay_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_result_value(replay_result, &count_value,
                                           out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count_value == 2u);
    CHECK(fixture_query_count_result_close(&replay_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_decode(
              replay_claim, NULL,
              (type_bridge_byte_view_t){response.data, response.length},
              &replayed_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
    CHECK(replayed_result == NULL);
    CHECK(execution_code_is(*out_diagnostics,
                            "c_query_remote_claim_consumed"));
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    free(response.data);
    response.data = NULL;
    response.length = 0u;
    CHECK(fixture_query_count_remote_claim_close(&replay_claim) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_count_remote_claim_close(&replay_claim) ==
          TYPE_BRIDGE_STATUS_OK);
    lifecycle_observation.remote_lane_observed = 1;
    CHECK(emit_query_lifecycle_observation(&lifecycle_observation));
    puts("G13 query/result/pending/claim lifecycle directions: passed");
  }

  {
    fixture_employee_query_exact_binding *exact = NULL;
    fixture_employee_query_subtypes_binding *subtypes = NULL;
    fixture_employee_query_exact_one_selection *exact_selection = NULL;
    fixture_employee_query_subtypes_one_selection *subtype_selection = NULL;
    fixture_employee_identifier_query_field *subtype_identifier = NULL;
    fixture_query_order *subtype_order = NULL;
    const fixture_query_order *subtype_orders[1];
    fixture_query *exact_query = NULL;
    fixture_query *subtype_query = NULL;
    fixture_query_rows_terminal *exact_terminal = NULL;
    fixture_query_rows_terminal *subtype_terminal = NULL;
    fixture_query_rows_result *exact_result = NULL;
    fixture_query_rows_result *subtype_result = NULL;
    size_t row_count = 0u;
    char direct_exact_key[64] = {0};
    char direct_subtype_employee_key[64] = {0};
    char direct_subtype_manager_key[64] = {0};
    char remote_exact_key[64] = {0};
    char remote_subtype_employee_key[64] = {0};
    char remote_subtype_manager_key[64] = {0};

    CHECK(fixture_employee_query_exact_binding_open(
              session, &exact, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_subtypes_binding_open(
              session, &subtypes, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_exact_one_selection_open(
              exact, &exact_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_subtypes_one_selection_open(
              subtypes, &subtype_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_identifier_query_field_from_subtypes(
              subtypes, &subtype_identifier, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_identifier_query_field_order(
              subtype_identifier, TYPE_BRIDGE_QUERY_SORT_ASCENDING,
              TYPE_BRIDGE_QUERY_MISSING_REJECT, &subtype_order,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    subtype_orders[0] = subtype_order;
    CHECK(fixture_query_positional_1(
              session,
              fixture_employee_query_exact_one_selection_ref(exact_selection),
              &exact_query, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_1(
              session,
              fixture_employee_query_subtypes_one_selection_ref(
                  subtype_selection),
              &subtype_query, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(exact_query, NULL, 0u, 0u, 4u,
                             &exact_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(subtype_query, subtype_orders, 1u, 0u, 4u,
                             &subtype_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, exact_terminal, &limits, NULL, &exact_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(exact_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(check_employee_exact_result(exact_result, direct_exact_key,
                                      sizeof(direct_exact_key),
                                      out_diagnostics) == 0);
    {
      fixture_employee *value = NULL;
      fixture_identifier *identifier = NULL;
      type_bridge_byte_view_t text = {NULL, 0u};
      CHECK(fixture_employee_query_exact_one_at(
                fixture_employee_query_exact_one_result_slot_v1_t_rows(
                    exact_result, 0u),
                0u, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_identifier(value, &identifier,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(same_text(text, "query-employee"));
      CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_close(&value) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_query_rows_result_close(&exact_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, subtype_terminal, &limits, NULL, &subtype_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(subtype_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 2u);
    CHECK(check_employee_subtype_result(
              subtype_result, direct_subtype_employee_key,
              sizeof(direct_subtype_employee_key), direct_subtype_manager_key,
              sizeof(direct_subtype_manager_key), out_diagnostics) == 0);
    {
      fixture_employee_query_subtypes_result *first = NULL;
      fixture_employee_query_subtypes_result *second = NULL;
      fixture_employee_query_subtypes_result_kind_t first_kind =
          fixture_employee_query_subtypes_result_kind_unknown;
      fixture_employee_query_subtypes_result_kind_t second_kind =
          fixture_employee_query_subtypes_result_kind_unknown;
      fixture_employee *employee_value = NULL;
      fixture_manager *manager_value = NULL;
      CHECK(fixture_employee_query_subtypes_one_at(
                fixture_employee_query_subtypes_one_result_slot_v1_t_rows(
                    subtype_result, 0u),
                0u, &first, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_query_subtypes_one_at(
                fixture_employee_query_subtypes_one_result_slot_v1_t_rows(
                    subtype_result, 0u),
                1u, &second, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_query_subtypes_result_kind(
                first, &first_kind, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_query_subtypes_result_kind(
                second, &second_kind, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(first_kind == fixture_employee_query_subtypes_result_kind_employee);
      CHECK(second_kind == fixture_employee_query_subtypes_result_kind_manager);
      CHECK(fixture_employee_query_subtypes_result_as_employee(
                first, &employee_value, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_query_subtypes_result_as_manager(
                second, &manager_value, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_close(&employee_value) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_manager_close(&manager_value) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_query_subtypes_result_close(&second) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_employee_query_subtypes_result_close(&first) ==
            TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_query_rows_result_close(&subtype_result) ==
          TYPE_BRIDGE_STATUS_OK);

    RUN_REMOTE_QUERY(rows, exact_terminal, exact_result);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(exact_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(check_employee_exact_result(exact_result, remote_exact_key,
                                      sizeof(remote_exact_key),
                                      out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&exact_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, subtype_terminal, subtype_result);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(subtype_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 2u);
    CHECK(check_employee_subtype_result(
              subtype_result, remote_subtype_employee_key,
              sizeof(remote_subtype_employee_key), remote_subtype_manager_key,
              sizeof(remote_subtype_manager_key), out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&subtype_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(emit_exact_subtypes_observation(
        "direct_runtime", direct_exact_key, direct_subtype_employee_key,
        direct_subtype_manager_key));
    CHECK(emit_exact_subtypes_observation(
        "remote_runtime", remote_exact_key, remote_subtype_employee_key,
        remote_subtype_manager_key));
    puts("C14 exact/subtype closed-union direct and remote: passed");

    CHECK(fixture_query_rows_terminal_close(&subtype_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&exact_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&subtype_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&exact_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_order_close(&subtype_order) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_identifier_query_field_close(
              &subtype_identifier) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_subtypes_one_selection_close(
              &subtype_selection) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_exact_one_selection_close(&exact_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_subtypes_binding_close(&subtypes) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_query_exact_binding_close(&exact) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  {
    fixture_person_query_exact_binding *group_binding = NULL;
    fixture_person_identifier_query_field *group_identifier_field = NULL;
    fixture_query_predicate *group_equal_predicate = NULL;
    fixture_query *group_hidden = NULL;
    fixture_query *grouped_query = NULL;
    fixture_person_scorezuzugte_query_field *score_gte_field = NULL;
    fixture_query_reducer_ref_v1_t reducers[7];
    fixture_query_reducer_ref_v1_t count_reducer[1];
    fixture_query_field_group_ref_v1_t tuple_groups[2];
    fixture_query_reduction_terminal *global_terminal = NULL;
    fixture_query_reduction_terminal *binding_terminal = NULL;
    fixture_query_field_reduction_terminal *field_terminal = NULL;
    fixture_query_field_tuple_reduction_terminal *tuple_terminal = NULL;
    fixture_query_reduction_result *global_result = NULL;
    fixture_query_reduction_result *binding_result = NULL;
    fixture_query_field_reduction_result *field_result = NULL;
    fixture_query_field_tuple_reduction_result *tuple_result = NULL;
    fixture_query_root_v1_t root =
        fixture_person_query_exact_binding_root(sdk_query,
                                                person_binding);
    fixture_query_root_v1_t grouped_root;

    CHECK(fixture_person_query_exact_binding_open(
              session, &group_binding, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_from_exact(
              group_binding, &group_identifier_field, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_string_field_compare_field(
              fixture_person_identifier_query_field_string_field_ref(
                  identifier_field),
              TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
              fixture_person_identifier_query_field_string_field_ref(
                  group_identifier_field),
              &group_equal_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_add_hidden(
              sdk_query,
              fixture_person_query_exact_binding_ref(group_binding),
              &group_hidden, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(group_hidden, group_equal_predicate,
                              &grouped_query, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    grouped_root = fixture_person_query_exact_binding_root(grouped_query,
                                                            person_binding);
    CHECK(fixture_person_scorezuzugte_query_field_from_exact(
              person_binding, &score_gte_field, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    reducers[0] = fixture_query_reducer_count();
    reducers[1] = fixture_person_score_query_field_reduce_sum_long(score_field);
    reducers[2] = fixture_person_score_query_field_reduce_min_long(score_field);
    reducers[3] = fixture_person_score_query_field_reduce_max_long(score_field);
    reducers[4] =
        fixture_person_score_query_field_reduce_mean_double(score_field);
    reducers[5] =
        fixture_person_score_query_field_reduce_median_double(score_field);
    reducers[6] =
        fixture_person_score_query_field_reduce_std_double(score_field);
    count_reducer[0] = fixture_query_reducer_count();
    tuple_groups[0] =
        fixture_person_score_query_field_reduction_group_slot_v1_t_from(
            score_field);
    tuple_groups[1] =
        fixture_person_scorezuzugte_query_field_reduction_group_slot_v1_t_from(
            score_gte_field);

    CHECK(fixture_query_reduce(root, reducers, 7u, &global_terminal,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_reduction(
              database, global_terminal, &limits, NULL, &global_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_sdk_reducers(
              fixture_query_reduction_result_ref(global_result),
              &direct_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_reduction_result_close(&global_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(reduction, global_terminal, global_result);
    CHECK(check_sdk_reducers(
              fixture_query_reduction_result_ref(global_result),
              &remote_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_reduction_result_close(&global_result) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_query_reduce_grouped(
              grouped_root,
              fixture_person_query_exact_reduction_group_slot_v1_t_from(
                  group_binding),
              count_reducer, 1u, &binding_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_reduction(
              database, binding_terminal, &limits, NULL, &binding_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_sdk_binding_groups(
              fixture_query_reduction_result_ref(binding_result),
              &direct_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_reduction_result_close(&binding_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(reduction, binding_terminal, binding_result);
    CHECK(check_sdk_binding_groups(
              fixture_query_reduction_result_ref(binding_result),
              &remote_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_reduction_result_close(&binding_result) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_query_reduce_field(
              root,
              fixture_person_score_query_field_reduction_group_slot_v1_t_from(
                  score_field),
              count_reducer, 1u, &field_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_field_reduction(
              database, field_terminal, &limits, NULL, &field_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_sdk_field_groups(
              fixture_query_field_reduction_result_ref(field_result),
              &direct_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_field_reduction_result_close(&field_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(field_reduction, field_terminal, field_result);
    CHECK(check_sdk_field_groups(
              fixture_query_field_reduction_result_ref(field_result),
              &remote_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_field_reduction_result_close(&field_result) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_query_reduce_fields(
              root, tuple_groups, 2u, count_reducer, 1u, &tuple_terminal,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_field_tuple_reduction(
              database, tuple_terminal, &limits, NULL, &tuple_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_sdk_field_tuple_groups(
              fixture_query_field_tuple_reduction_result_ref(tuple_result),
              &direct_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_field_tuple_reduction_result_close(&tuple_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(field_tuple_reduction, tuple_terminal, tuple_result);
    CHECK(check_sdk_field_tuple_groups(
              fixture_query_field_tuple_reduction_result_ref(tuple_result),
              &remote_reducers, out_diagnostics) == 0);
    CHECK(fixture_query_field_tuple_reduction_result_close(&tuple_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(emit_grouped_reducer_observation("direct_runtime",
                                           &direct_reducers));
    CHECK(emit_grouped_reducer_observation("remote_runtime",
                                           &remote_reducers));
    puts("C20/C31 all reducers and binding/field/tuple groups direct/remote: passed");

    CHECK(fixture_query_field_tuple_reduction_terminal_close(&tuple_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_field_reduction_terminal_close(&field_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_reduction_terminal_close(&binding_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_reduction_terminal_close(&global_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_scorezuzugte_query_field_close(&score_gte_field) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&grouped_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&group_hidden) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&group_equal_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_close(
              &group_identifier_field) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&group_binding) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  {
    fixture_membership_query_exact_binding *relation = NULL;
    fixture_person_query_exact_binding *member = NULL;
    fixture_membership_member_query_role *member_role = NULL;
    fixture_membership_query_exact_one_selection *relation_selection = NULL;
    fixture_person_query_exact_one_selection *member_selection = NULL;
    fixture_query_predicate *role_predicate = NULL;
    fixture_query_predicate *relation_iid_predicate = NULL;
    fixture_query_predicate *member_iid_predicate = NULL;
    fixture_query_predicate *relation_and_role = NULL;
    fixture_query_predicate *all_predicates = NULL;
    fixture_query *role_query = NULL;
    fixture_query *filtered_role_query = NULL;
    fixture_query_rows_terminal *role_terminal = NULL;
    fixture_query_rows_result *role_result = NULL;
    fixture_query_rows_result *role_limit_result = NULL;
    fixture_person_score_query_field *member_score_field = NULL;
    fixture_query_reducer_ref_v1_t reducers[2];
    fixture_query_reduction_terminal *reduction_terminal = NULL;
    fixture_query_reduction_result *reduction_result = NULL;
    size_t row_count = 0u;

    CHECK(fixture_membership_query_exact_binding_open(
              session, &relation, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_open(
              session, &member, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_member_query_role_from_exact(
              relation, &member_role, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_member_query_role_connects(
              member_role,
              fixture_membership_member_query_role_player_person_exact(member),
              &role_predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_query_exact_binding_iid(
              relation, membership_iid, &relation_iid_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid(
              member, ada_iid, &member_iid_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              role_predicate, relation_iid_predicate, &relation_and_role,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              relation_and_role, member_iid_predicate, &all_predicates,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_query_exact_one_selection_open(
              relation, &relation_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_open(
              member, &member_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_2(
              session,
              fixture_membership_query_exact_one_selection_ref(
                  relation_selection),
              fixture_person_query_exact_one_selection_ref(member_selection),
              &role_query, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(role_query, all_predicates,
                              &filtered_role_query, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_one(filtered_role_query, NULL, 0u, &role_terminal,
                            out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    {
      type_bridge_query_execution_limits_v1_t role_limits = limits;
      role_limits.role_players = 0u;
      CHECK(fixture_database_query_execute_rows(
                database, role_terminal, &role_limits, NULL,
                &role_limit_result, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
      CHECK(role_limit_result == NULL);
      CHECK(check_structured_execution_diagnostic(
                *out_diagnostics,
                TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_RESOURCE_LIMIT,
                "hydrated_role_player_limit") == 0);
      CHECK(observe_resource_limit_diagnostic(
                *out_diagnostics, "role_players",
                &resource_limit_observation) == 0);
      CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      ++zero_limit_dimension_count;
      CHECK(zero_limit_dimension_count == 8u);
      resource_limit_observation.zero_dimensions =
          zero_limit_dimension_count;
      resource_limit_observation.plus_one_dimensions =
          plus_one_limit_dimension_count;
      resource_limit_observation.no_partial_result =
          role_limit_result == NULL;
      CHECK(emit_resource_limits_observation(
          "direct_runtime", &resource_limit_observation));
      resource_limit_observation.no_partial_result =
          role_limit_result == NULL && remote_representative_zero_limit &&
          remote_plus_one_clamped_all;
      CHECK(emit_resource_limits_observation(
          "remote_runtime", &resource_limit_observation));
      puts("G06 all-eight independent zero ceilings: passed");
    }
    CHECK(fixture_database_query_execute_rows(
              database, role_terminal, &limits, NULL, &role_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(role_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(check_membership_query_result(
              role_result, ada_iid, direct_membership_member_key,
              sizeof(direct_membership_member_key), out_diagnostics) == 0);
    {
      fixture_membership *value = NULL;
      fixture_person *member_value = NULL;
      CHECK(fixture_membership_query_exact_one_at(
                fixture_membership_query_exact_one_result_slot_v1_t_rows(
                    role_result, 0u),
                0u, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(check_membership(value, ada_iid, out_diagnostics) == 0);
      CHECK(fixture_person_query_exact_one_at(
                fixture_person_query_exact_one_result_slot_v1_t_rows(
                    role_result, 1u),
                0u, &member_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(check_query_person_identifier(member_value, "query-ada",
                                          out_diagnostics) == 0);
      CHECK(fixture_person_close(&member_value) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_membership_close(&value) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_query_rows_result_close(&role_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, role_terminal, role_result);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(role_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(check_membership_query_result(
              role_result, ada_iid, remote_membership_member_key,
              sizeof(remote_membership_member_key), out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&role_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(emit_hydrated_result_observation("direct_runtime",
                                           direct_membership_member_key));
    CHECK(emit_hydrated_result_observation("remote_runtime",
                                           remote_membership_member_key));
    puts("C16/C22 role traversal and hydrated relation direct/remote: passed");

    CHECK(fixture_person_score_query_field_from_exact(
              member, &member_score_field, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    reducers[0] = fixture_query_reducer_count();
    reducers[1] =
        fixture_person_score_query_field_reduce_sum_long(member_score_field);
    CHECK(fixture_query_reduce_grouped(
              fixture_membership_query_exact_binding_root(
                  filtered_role_query, relation),
              fixture_person_query_exact_reduction_group_slot_v1_t_from(
                  member),
              reducers, 2u, &reduction_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_reduction(
              database, reduction_terminal, &limits, NULL, &reduction_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    {
      fixture_query_reduction_result_ref_v1_t reduction_ref =
          fixture_query_reduction_result_ref(reduction_result);
      fixture_person *group_person = NULL;
      type_bridge_query_reduced_value_metadata_v1_t sum_metadata = {0};
      uint64_t reduced_count = 0u;
      int64_t reduced_sum = 0;
      CHECK(fixture_query_reduction_result_row_count(
                reduction_ref, &row_count, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(row_count == 1u);
      CHECK(fixture_person_query_exact_reduction_group_slot_v1_t_thing_at(
                fixture_person_query_exact_reduction_group_slot_v1_t_thing(
                    reduction_ref),
                0u, &group_person, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(check_query_person_identifier(group_person, "query-ada",
                                          out_diagnostics) == 0);
      CHECK(fixture_person_close(&group_person) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_query_reduced_count_value(
                fixture_query_reduced_count_slot(reduction_ref, 0u), 0u,
                &reduced_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(reduced_count == 1u);
      CHECK(fixture_query_reduced_long_metadata(
                fixture_query_reduced_long_slot(reduction_ref, 1u), 0u,
                &sum_metadata, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(sum_metadata.kind == TYPE_BRIDGE_QUERY_REDUCED_LONG &&
            sum_metadata.present == 1u);
      CHECK(fixture_query_reduced_long_value(
                fixture_query_reduced_long_slot(reduction_ref, 1u), 0u,
                &reduced_sum, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(reduced_sum == 38);
    }
    CHECK(fixture_query_reduction_result_close(&reduction_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(reduction, reduction_terminal, reduction_result);
    {
      fixture_query_reduction_result_ref_v1_t reduction_ref =
          fixture_query_reduction_result_ref(reduction_result);
      uint64_t reduced_count = 0u;
      int64_t reduced_sum = 0;
      CHECK(fixture_query_reduction_result_row_count(
                reduction_ref, &row_count, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(row_count == 1u);
      CHECK(fixture_query_reduced_count_value(
                fixture_query_reduced_count_slot(reduction_ref, 0u), 0u,
                &reduced_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_query_reduced_long_value(
                fixture_query_reduced_long_slot(reduction_ref, 1u), 0u,
                &reduced_sum, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(reduced_count == 1u && reduced_sum == 38);
    }
    CHECK(fixture_query_reduction_result_close(&reduction_result) ==
          TYPE_BRIDGE_STATUS_OK);
    puts("C20/C31 hydrated relation-member group direct/remote: passed");

    CHECK(fixture_query_reduction_terminal_close(&reduction_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_score_query_field_close(&member_score_field) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&role_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&filtered_role_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&role_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&all_predicates) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&relation_and_role) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&member_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&relation_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&role_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_close(&member_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_query_exact_one_selection_close(
              &relation_selection) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_member_query_role_close(&member_role) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&member) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_query_exact_binding_close(&relation) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  {
    fixture_networkzhlink_query_exact_binding *relation = NULL;
    fixture_person_query_exact_binding *source = NULL;
    fixture_person_query_exact_binding *target = NULL;
    fixture_networkzhlink_origin_query_role *origin_role = NULL;
    fixture_networkzhlink_destination_query_role *destination_role = NULL;
    fixture_networkzhlink_query_exact_one_selection *relation_selection = NULL;
    fixture_person_query_exact_one_selection *source_selection = NULL;
    fixture_person_query_exact_one_selection *target_selection = NULL;
    fixture_query_predicate *relation_iid_predicate = NULL;
    fixture_query_predicate *source_iid_predicate = NULL;
    fixture_query_predicate *target_iid_predicate = NULL;
    fixture_query_predicate *origin_predicate = NULL;
    fixture_query_predicate *destination_predicate = NULL;
    fixture_query_predicate *topology_predicate_1 = NULL;
    fixture_query_predicate *topology_predicate_2 = NULL;
    fixture_query_predicate *topology_predicate_3 = NULL;
    fixture_query_predicate *topology_predicate = NULL;
    fixture_query_predicate *reachable_predicate = NULL;
    fixture_query_predicate *reachable_endpoints = NULL;
    fixture_query_predicate *reachable_all = NULL;
    fixture_query *topology_shape = NULL;
    fixture_query *topology_query = NULL;
    fixture_query *reachable_shape = NULL;
    fixture_query *reachable_query = NULL;
    fixture_query_rows_terminal *topology_terminal = NULL;
    fixture_query_rows_terminal *reachable_terminal = NULL;
    fixture_query_rows_result *topology_result = NULL;
    fixture_query_rows_result *reachable_result = NULL;
    fixture_networkzhlink_origin_query_role_player_binding_v1_t source_origin;
    fixture_networkzhlink_destination_query_role_player_binding_v1_t
        target_destination;
    const size_t reachable_max_hops = 1u;
    size_t row_count = 0u;

    CHECK(fixture_networkzhlink_query_exact_binding_open(
              session, &relation, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_open(
              session, &source, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_open(
              session, &target, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_origin_query_role_from_exact(
              relation, &origin_role, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_destination_query_role_from_exact(
              relation, &destination_role, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    source_origin =
        fixture_networkzhlink_origin_query_role_player_person_exact(source);
    target_destination =
        fixture_networkzhlink_destination_query_role_player_person_exact(
            target);
    CHECK(fixture_networkzhlink_origin_query_role_connects(
              origin_role, source_origin, &origin_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_destination_query_role_connects(
              destination_role, target_destination, &destination_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_query_exact_binding_iid(
              relation, network_link_iid, &relation_iid_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid(
              source, ada_iid, &source_iid_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid(
              target, dana_iid, &target_iid_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              relation_iid_predicate, origin_predicate,
              &topology_predicate_1, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              topology_predicate_1, destination_predicate,
              &topology_predicate_2, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              topology_predicate_2, source_iid_predicate,
              &topology_predicate_3, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              topology_predicate_3, target_iid_predicate,
              &topology_predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_query_exact_one_selection_open(
              relation, &relation_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_open(
              source, &source_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_open(
              target, &target_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_3(
              session,
              fixture_networkzhlink_query_exact_one_selection_ref(
                  relation_selection),
              fixture_person_query_exact_one_selection_ref(source_selection),
              fixture_person_query_exact_one_selection_ref(target_selection),
              &topology_shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(topology_shape, topology_predicate,
                              &topology_query, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(topology_query, NULL, 0u, 0u, 2u,
                             &topology_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, topology_terminal, &limits, NULL, &topology_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_network_topology_result(
              topology_result, direct_topology_source_key,
              sizeof(direct_topology_source_key), direct_topology_target_key,
              sizeof(direct_topology_target_key), out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(topology_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    {
      fixture_networkzhlink *link = NULL;
      fixture_identifier *identifier = NULL;
      type_bridge_byte_view_t text = {NULL, 0u};
      fixture_person *source_value = NULL;
      fixture_person *target_value = NULL;
      CHECK(fixture_networkzhlink_query_exact_one_at(
                fixture_networkzhlink_query_exact_one_result_slot_v1_t_rows(
                    topology_result, 0u),
                0u, &link, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_networkzhlink_identifier(link, &identifier,
                                             out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_identifier_value(identifier, &text, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(same_text(text, "query-link"));
      CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_person_query_exact_one_at(
                fixture_person_query_exact_one_result_slot_v1_t_rows(
                    topology_result, 1u),
                0u, &source_value, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_person_query_exact_one_at(
                fixture_person_query_exact_one_result_slot_v1_t_rows(
                    topology_result, 2u),
                0u, &target_value, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(check_query_person_identifier(source_value, "query-ada",
                                          out_diagnostics) == 0);
      CHECK(check_query_person_identifier(target_value, "query-dana",
                                          out_diagnostics) == 0);
      CHECK(fixture_person_close(&target_value) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_person_close(&source_value) == TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_query_rows_result_close(&topology_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, topology_terminal, topology_result);
    CHECK(check_network_topology_result(
              topology_result, remote_topology_source_key,
              sizeof(remote_topology_source_key), remote_topology_target_key,
              sizeof(remote_topology_target_key), out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(topology_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(fixture_query_rows_result_close(&topology_result) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_networkzhlink_query_reachable(
              session,
              fixture_networkzhlink_query_reachable_endpoint_v1_t_from_origin(
                  source_origin),
              fixture_networkzhlink_query_reachable_endpoint_v1_t_from_destination(
                  target_destination),
              1u, reachable_max_hops, &reachable_predicate,
              out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              source_iid_predicate, target_iid_predicate,
              &reachable_endpoints, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              reachable_endpoints, reachable_predicate, &reachable_all,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_2(
              session,
              fixture_person_query_exact_one_selection_ref(source_selection),
              fixture_person_query_exact_one_selection_ref(target_selection),
              &reachable_shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(reachable_shape, reachable_all,
                              &reachable_query, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(reachable_query, NULL, 0u, 0u, 2u,
                             &reachable_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, reachable_terminal, &limits, NULL, &reachable_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_reachable_result(reachable_result, &direct_topology,
                                 reachable_max_hops, out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(reachable_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(fixture_query_rows_result_close(&reachable_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, reachable_terminal, reachable_result);
    CHECK(check_reachable_result(reachable_result, &remote_topology,
                                 reachable_max_hops, out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(reachable_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(fixture_query_rows_result_close(&reachable_result) ==
          TYPE_BRIDGE_STATUS_OK);
    puts("C16/C17 role-pair topology and reachability direct/remote: passed");

    CHECK(fixture_query_rows_terminal_close(&reachable_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&reachable_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&reachable_shape) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&reachable_all) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&reachable_endpoints) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&reachable_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&topology_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&topology_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&topology_shape) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&topology_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&topology_predicate_3) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&topology_predicate_2) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&topology_predicate_1) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&destination_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&origin_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&target_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&source_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&relation_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_close(&target_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_close(&source_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_query_exact_one_selection_close(
              &relation_selection) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_destination_query_role_close(
              &destination_role) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_origin_query_role_close(&origin_role) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&target) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&source) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_query_exact_binding_close(&relation) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  {
    fixture_person_query_exact_binding *left = NULL;
    fixture_person_query_exact_binding *right = NULL;
    fixture_person_identifier_query_field *left_identifier_field = NULL;
    fixture_person_score_query_field *left_score_field = NULL;
    fixture_person_scorezuzugte_query_field *left_score_gte_field = NULL;
    fixture_query_order *left_identifier_order = NULL;
    const fixture_query_order *left_orders[1];
    fixture_person_query_exact_one_selection *left_selection = NULL;
    fixture_person_query_exact_one_selection *right_selection = NULL;
    fixture_query_predicate *left_ada = NULL;
    fixture_query_predicate *left_dana = NULL;
    fixture_query_predicate *left_or = NULL;
    fixture_query_predicate *left_not = NULL;
    fixture_query_predicate *left_double_not = NULL;
    fixture_query_predicate *left_high = NULL;
    fixture_query_predicate *left_not_high = NULL;
    fixture_query_predicate *left_not_high_scoped = NULL;
    fixture_query_predicate *field_comparison_predicate = NULL;
    fixture_query_predicate *field_comparison_scoped = NULL;
    fixture_query_predicate *right_in = NULL;
    fixture_query_predicate *cross_predicate = NULL;
    fixture_query *boolean_shape = NULL;
    fixture_query *or_query = NULL;
    fixture_query *not_query = NULL;
    fixture_query *field_comparison_query = NULL;
    fixture_query *cross_shape = NULL;
    fixture_query *cross_filtered = NULL;
    fixture_query *cross_query = NULL;
    fixture_query_rows_terminal *or_terminal = NULL;
    fixture_query_rows_terminal *not_terminal = NULL;
    fixture_query_rows_terminal *field_comparison_terminal = NULL;
    fixture_query_rows_terminal *cross_terminal = NULL;
    fixture_query_rows_result *or_result = NULL;
    fixture_query_rows_result *not_result = NULL;
    fixture_query_rows_result *field_comparison_result = NULL;
    fixture_query_rows_result *cross_result = NULL;
    const type_bridge_byte_view_t iids[2] = {ada_iid, dana_iid};
    size_t row_count = 0u;
    sdk_person_rows_observation_t direct_or_rows = {0};
    sdk_person_rows_observation_t remote_or_rows = {0};
    sdk_person_rows_observation_t direct_not_rows = {0};
    sdk_person_rows_observation_t remote_not_rows = {0};
    sdk_person_rows_observation_t direct_field_comparison_rows = {0};
    sdk_person_rows_observation_t remote_field_comparison_rows = {0};

    CHECK(fixture_person_query_exact_binding_open(
              session, &left, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_open(
              session, &right, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid(
              left, ada_iid, &left_ada, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid(
              left, dana_iid, &left_dana, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_or(left_ada, left_dana, &left_or,
                                     out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_not(left_or, &left_not, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_not(left_not, &left_double_not,
                                      out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_score_query_field_from_exact(
              left, &left_score_field, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_scorezuzugte_query_field_from_exact(
              left, &left_score_gte_field, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_integer_field_compare_field(
              fixture_person_score_query_field_integer_field_ref(
                  left_score_field),
              TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL,
              fixture_person_scorezuzugte_query_field_integer_field_ref(
                  left_score_gte_field),
              &field_comparison_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_score_query_field_compare_value(
              left_score_field,
              TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL, threshold,
              &left_high, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_not(left_high, &left_not_high,
                                      out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(left_or, left_not_high,
                                      &left_not_high_scoped,
                                      out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(left_or, field_comparison_predicate,
                                      &field_comparison_scoped,
                                      out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_from_exact(
              left, &left_identifier_field, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_order(
              left_identifier_field, TYPE_BRIDGE_QUERY_SORT_ASCENDING,
              TYPE_BRIDGE_QUERY_MISSING_REJECT, &left_identifier_order,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    left_orders[0] = left_identifier_order;
    CHECK(fixture_person_query_exact_binding_iid_in(
              right, iids, 2u, &right_in, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              left_double_not, right_in, &cross_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_open(
              left, &left_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_open(
              right, &right_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_1(
              session,
              fixture_person_query_exact_one_selection_ref(left_selection),
              &boolean_shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(boolean_shape, left_or, &or_query,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(boolean_shape, left_not_high_scoped, &not_query,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(boolean_shape, field_comparison_scoped,
                              &field_comparison_query, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(or_query, left_orders, 1u, 0u, 2u,
                             &or_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(not_query, left_orders, 1u, 0u, 2u,
                             &not_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(field_comparison_query, left_orders, 1u, 0u, 2u,
                             &field_comparison_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, or_terminal, &limits, NULL, &or_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_rows_identifiers(or_result, expected_rows, 2u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(or_result, &direct_or_rows, out_diagnostics) ==
          0);
    CHECK(fixture_query_rows_result_close(&or_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, or_terminal, or_result);
    CHECK(check_rows_identifiers(or_result, expected_rows, 2u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(or_result, &remote_or_rows, out_diagnostics) ==
          0);
    CHECK(fixture_query_rows_result_close(&or_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, not_terminal, &limits, NULL, &not_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_rows_identifiers(not_result, expected_ada, 1u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(not_result, &direct_not_rows, out_diagnostics) ==
          0);
    CHECK(fixture_query_rows_result_close(&not_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, field_comparison_terminal, &limits, NULL,
              &field_comparison_result, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_rows_identifiers(field_comparison_result, expected_dana, 1u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(field_comparison_result,
                              &direct_field_comparison_rows,
                              out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&field_comparison_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, field_comparison_terminal, field_comparison_result);
    CHECK(check_rows_identifiers(field_comparison_result, expected_dana, 1u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(field_comparison_result,
                              &remote_field_comparison_rows,
                              out_diagnostics) == 0);
    CHECK(fixture_query_rows_result_close(&field_comparison_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, not_terminal, not_result);
    CHECK(check_rows_identifiers(not_result, expected_ada, 1u,
                                 out_diagnostics) == 0);
    CHECK(observe_person_rows(not_result, &remote_not_rows, out_diagnostics) ==
          0);
    CHECK(fixture_query_rows_result_close(&not_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_positional_2(
              session,
              fixture_person_query_exact_one_selection_ref(left_selection),
              fixture_person_query_exact_one_selection_ref(right_selection),
              &cross_shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(cross_shape, cross_predicate, &cross_filtered,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_allow_cross_join(
              cross_filtered, fixture_person_query_exact_binding_ref(left),
              fixture_person_query_exact_binding_ref(right), &cross_query,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows(cross_query, NULL, 0u, 0u, 8u,
                             &cross_terminal, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_database_query_execute_rows(
              database, cross_terminal, &limits, NULL, &cross_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_cross_join_result(cross_result, &direct_topology,
                                  out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(cross_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 4u);
    CHECK(fixture_query_rows_result_close(&cross_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(rows, cross_terminal, cross_result);
    CHECK(check_cross_join_result(cross_result, &remote_topology,
                                  out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_rows_result_ref(cross_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 4u);
    CHECK(fixture_query_rows_result_close(&cross_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(emit_scalar_boolean_observation(
        "direct_runtime", &direct_one_rows, &direct_field_comparison_rows,
        &direct_not_rows, &direct_or_rows));
    CHECK(emit_scalar_boolean_observation(
        "remote_runtime", &remote_one_rows, &remote_field_comparison_rows,
        &remote_not_rows, &remote_or_rows));
    CHECK(emit_topology_observation("direct_runtime", &direct_topology));
    CHECK(emit_topology_observation("remote_runtime", &remote_topology));
    puts("C15/C17 boolean topology and explicit cross join: passed");

    CHECK(fixture_query_rows_terminal_close(&cross_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&cross_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&cross_filtered) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&cross_shape) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&not_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&field_comparison_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_rows_terminal_close(&or_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&not_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&field_comparison_query) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&or_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&boolean_shape) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_close(&right_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_close(&left_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&cross_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&right_in) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&field_comparison_scoped) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_not_high_scoped) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_not_high) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&field_comparison_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_high) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_double_not) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_not) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_or) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_dana) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&left_ada) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_order_close(&left_identifier_order) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_close(
              &left_identifier_field) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_score_query_field_close(&left_score_field) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_scorezuzugte_query_field_close(
              &left_score_gte_field) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&right) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&left) ==
          TYPE_BRIDGE_STATUS_OK);
  }

  {
    fixture_networkzhlink_query_exact_binding *relation = NULL;
    fixture_person_query_exact_binding *origin = NULL;
    fixture_person_query_exact_binding *participant = NULL;
    fixture_networkzhlink_origin_query_role *origin_role = NULL;
    fixture_networkzhlink_participant_query_role *participant_role = NULL;
    fixture_person_identifier_query_field *origin_identifier = NULL;
    fixture_person_identifier_query_field *participant_identifier = NULL;
    fixture_query_order *origin_order = NULL;
    fixture_query_order *participant_order = NULL;
    const fixture_query_order *origin_orders[1];
    const fixture_query_order *participant_orders[1];
    fixture_person_query_exact_one_selection *origin_selection = NULL;
    fixture_person_query_exact_collect_selection *participant_selection = NULL;
    fixture_query_predicate *relation_iid_predicate = NULL;
    fixture_query_predicate *origin_iid_predicate = NULL;
    fixture_query_predicate *participant_iid_predicate = NULL;
    fixture_query_predicate *origin_role_predicate = NULL;
    fixture_query_predicate *participant_role_predicate = NULL;
    fixture_query_predicate *shape_predicate_1 = NULL;
    fixture_query_predicate *shape_predicate_2 = NULL;
    fixture_query_predicate *shape_predicate_3 = NULL;
    fixture_query_predicate *shape_predicate = NULL;
    fixture_query *named_shape = NULL;
    fixture_query *named_hidden = NULL;
    fixture_query *named_query = NULL;
    fixture_query *positional_shape = NULL;
    fixture_query *positional_hidden = NULL;
    fixture_query *positional_query = NULL;
    fixture_query_page_terminal *named_terminal = NULL;
    fixture_query_page_terminal *positional_terminal = NULL;
    fixture_query_page_result *named_result = NULL;
    fixture_query_page_result *positional_result = NULL;
    const type_bridge_byte_view_t participant_iids[2] = {ada_iid, dana_iid};
    const int collected_distinct = 1;
    const type_bridge_query_sort_direction_t collection_order =
        TYPE_BRIDGE_QUERY_SORT_ASCENDING;
    size_t row_count = 0u;
    sdk_selection_shape_observation_t direct_selection_shape = {0};
    sdk_selection_shape_observation_t remote_selection_shape = {0};

    direct_selection_shape.collected_distinct = collected_distinct;
    direct_selection_shape.collection_order = collection_order;
    remote_selection_shape.collected_distinct = collected_distinct;
    remote_selection_shape.collection_order = collection_order;

    CHECK(fixture_networkzhlink_query_exact_binding_open(
              session, &relation, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_open(
              session, &origin, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_open(
              session, &participant, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_origin_query_role_from_exact(
              relation, &origin_role, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_participant_query_role_from_exact(
              relation, &participant_role, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_origin_query_role_connects(
              origin_role,
              fixture_networkzhlink_origin_query_role_player_person_exact(
                  origin),
              &origin_role_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_participant_query_role_connects(
              participant_role,
              fixture_networkzhlink_participant_query_role_player_person_exact(
                  participant),
              &participant_role_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_query_exact_binding_iid(
              relation, network_link_iid, &relation_iid_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid(
              origin, ada_iid, &origin_iid_predicate, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_iid_in(
              participant, participant_iids, 2u, &participant_iid_predicate,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              relation_iid_predicate, origin_iid_predicate,
              &shape_predicate_1, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              shape_predicate_1, participant_iid_predicate,
              &shape_predicate_2, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              shape_predicate_2, origin_role_predicate, &shape_predicate_3,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_and(
              shape_predicate_3, participant_role_predicate,
              &shape_predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_from_exact(
              participant, &participant_identifier, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_from_exact(
              origin, &origin_identifier, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_order(
              origin_identifier, TYPE_BRIDGE_QUERY_SORT_ASCENDING,
              TYPE_BRIDGE_QUERY_MISSING_REJECT, &origin_order,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    origin_orders[0] = origin_order;
    CHECK(fixture_person_identifier_query_field_order(
              participant_identifier, collection_order,
              TYPE_BRIDGE_QUERY_MISSING_REJECT, &participant_order,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    participant_orders[0] = participant_order;
    CHECK(fixture_person_query_exact_one_selection_open(
              origin, &origin_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_collect_selection_open(
              participant, (uint8_t)collected_distinct, participant_orders, 1u,
              &participant_selection, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_named_2(
              session, view_of("origin"),
              fixture_person_query_exact_one_selection_ref(origin_selection),
              view_of("participants"),
              fixture_person_query_exact_collect_selection_ref(
                  participant_selection),
              &named_shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_add_hidden(
              named_shape,
              fixture_networkzhlink_query_exact_binding_ref(relation),
              &named_hidden, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(named_hidden, shape_predicate, &named_query,
                              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    {
      fixture_query_root_v1_t named_root =
          fixture_person_query_exact_binding_root(named_query, origin);
      CHECK(fixture_query_page(named_root, origin_orders, 1u, 0u, 1u, 1u,
                               &named_terminal, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_database_query_execute_page(
              database, named_terminal, &limits, NULL, &named_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_selection_shape_result(
              named_result, direct_selection_shape.named_origin,
              sizeof(direct_selection_shape.named_origin),
              direct_selection_shape.named_participants,
              &direct_selection_shape.named_participant_count,
              out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_page_result_ref(named_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(fixture_query_page_result_close(&named_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(page, named_terminal, named_result);
    CHECK(check_selection_shape_result(
              named_result, remote_selection_shape.named_origin,
              sizeof(remote_selection_shape.named_origin),
              remote_selection_shape.named_participants,
              &remote_selection_shape.named_participant_count,
              out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_page_result_ref(named_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(fixture_query_page_result_close(&named_result) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(fixture_query_positional_2(
              session,
              fixture_person_query_exact_one_selection_ref(origin_selection),
              fixture_person_query_exact_collect_selection_ref(
                  participant_selection),
              &positional_shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_add_hidden(
              positional_shape,
              fixture_networkzhlink_query_exact_binding_ref(relation),
              &positional_hidden, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_where(positional_hidden, shape_predicate,
                              &positional_query, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    {
      fixture_query_root_v1_t positional_root =
          fixture_person_query_exact_binding_root(positional_query, origin);
      CHECK(fixture_query_page(positional_root, origin_orders, 1u, 0u, 1u, 1u,
                               &positional_terminal, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_database_query_execute_page(
              database, positional_terminal, &limits, NULL,
              &positional_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_selection_shape_result(
              positional_result, direct_selection_shape.positional_origin,
              sizeof(direct_selection_shape.positional_origin),
              direct_selection_shape.positional_participants,
              &direct_selection_shape.positional_participant_count,
              out_diagnostics) == 0);
    CHECK(fixture_query_result_row_count(
              fixture_query_page_result_ref(positional_result), &row_count,
              out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(row_count == 1u);
    CHECK(fixture_query_page_result_close(&positional_result) ==
          TYPE_BRIDGE_STATUS_OK);
    RUN_REMOTE_QUERY(page, positional_terminal, positional_result);
    CHECK(check_selection_shape_result(
              positional_result, remote_selection_shape.positional_origin,
              sizeof(remote_selection_shape.positional_origin),
              remote_selection_shape.positional_participants,
              &remote_selection_shape.positional_participant_count,
              out_diagnostics) == 0);
    CHECK(fixture_query_page_result_close(&positional_result) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(emit_selection_shapes_observation("direct_runtime",
                                            &direct_selection_shape));
    CHECK(emit_selection_shapes_observation("remote_runtime",
                                            &remote_selection_shape));
    CHECK(emit_roles_observation(
        "direct_runtime", direct_membership_member_key,
        direct_topology_source_key, direct_topology_target_key,
        direct_selection_shape.named_participants[0],
        direct_selection_shape.named_participants[1]));
    CHECK(emit_roles_observation(
        "remote_runtime", remote_membership_member_key,
        remote_topology_source_key, remote_topology_target_key,
        remote_selection_shape.named_participants[0],
        remote_selection_shape.named_participants[1]));
    puts("C18 positional/named one+collect shapes direct/remote: passed");

    CHECK(fixture_query_page_terminal_close(&positional_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&positional_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&positional_hidden) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&positional_shape) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_page_terminal_close(&named_terminal) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&named_query) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&named_hidden) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_close(&named_shape) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_collect_selection_close(
              &participant_selection) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_one_selection_close(&origin_selection) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_order_close(&participant_order) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_order_close(&origin_order) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_close(
              &participant_identifier) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_identifier_query_field_close(&origin_identifier) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&shape_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&shape_predicate_3) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&shape_predicate_2) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&shape_predicate_1) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&participant_role_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&origin_role_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&participant_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&origin_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_query_predicate_close(&relation_iid_predicate) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_participant_query_role_close(
              &participant_role) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_origin_query_role_close(&origin_role) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&participant) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_query_exact_binding_close(&origin) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_query_exact_binding_close(&relation) ==
          TYPE_BRIDGE_STATUS_OK);
  }

#undef RUN_REMOTE_QUERY

  CHECK(fixture_query_remote_context_close(&remote_context) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&direct_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_exists_terminal_close(&exists_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_count_terminal_close(&count_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_page_terminal_close(&page_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_terminal_close(&rows_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_terminal_close(&nickname_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_first_terminal_close(&first_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_terminal_close(&one_terminal) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_terminal_close(&terminal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&dana_query) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&sdk_query) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&nickname_query) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&filtered) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&query) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&dana_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&owner_iid_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&score_present) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&nickname_scoped_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&nickname_present) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&threshold_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&filtered_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&iid_predicate) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&predicate) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_qualifyingzhscore_query_call_close(&call) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_qualifyingzhscore_query_close(&function) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_function_integer_input_close(&minimum_input) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_close(&threshold) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_close(&minimum) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_selection_close(&selection) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_score_query_field_close(&score_field) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_nickname_query_field_close(&nickname_field) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier_query_field_close(&identifier_field) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_order_close(&identifier_order) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_binding_close(&person_binding) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_session_close(&session) == TYPE_BRIDGE_STATUS_OK);
  CHECK(emit_scalar_domain_observation("direct_runtime", &direct_one_rows,
                                       threshold_comparison,
                                       threshold_number));
  CHECK(emit_scalar_domain_observation("remote_runtime", &remote_one_rows,
                                       threshold_comparison,
                                       threshold_number));
  CHECK(*out_diagnostics == NULL);
  return 0;
}

#ifdef TYPE_BRIDGE_SDK_V5_C_CODEC
static int write_canonical_view(const char *directory, const char *name,
                                type_bridge_byte_view_t value) {
  char path[4096];
  FILE *stream;
  if (snprintf(path, sizeof(path), "%s/%s", directory, name) <= 0) return 0;
  stream = fopen(path, "wbx");
  if (stream == NULL) return 0;
  if (fwrite(value.data, 1u, value.length, stream) != value.length ||
      fflush(stream) != 0 || fclose(stream) != 0) {
    return 0;
  }
  return 1;
}

static int run_sdk_v5_live_codec(
    const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  const char *directory = getenv("TYPE_BRIDGE_SDK_V5_C_EVIDENCE_DIR");
  const uint16_t remote_port = required_remote_port();
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  fixture_person_create *person_create = NULL;
  fixture_person *inserted_person = NULL;
  fixture_person_ref *inserted_person_ref = NULL;
  fixture_employment_employee_player *employee_player = NULL;
  fixture_employment_create_args_v1_t employment_args = {0};
  fixture_employment_create *employment_create = NULL;
  fixture_employment *inserted_employment = NULL;
  fixture_query_session *session = NULL;
  fixture_employment_query_exact_binding *employment_binding = NULL;
  fixture_person_query_exact_binding *person_binding = NULL;
  fixture_employment_employee_query_role *employee_role = NULL;
  fixture_person_identifier_query_field *identifier_field = NULL;
  fixture_identifier *identifier = NULL;
  fixture_query_predicate *role_predicate = NULL;
  fixture_query_predicate *identifier_predicate = NULL;
  fixture_query_predicate *predicate = NULL;
  fixture_employment_query_exact_one_selection *employment_selection = NULL;
  fixture_person_query_exact_one_selection *person_selection = NULL;
  fixture_query *shape = NULL;
  fixture_query *query = NULL;
  fixture_query_rows_terminal *terminal = NULL;
  fixture_query_rows_result *direct_result = NULL;
  fixture_query_rows_result *remote_result = NULL;
  fixture_employment *direct_employment = NULL;
  fixture_person *direct_person = NULL;
  fixture_employment *remote_employment = NULL;
  fixture_person *remote_person = NULL;
  type_bridge_canonical_bytes_t *direct_entity_bytes = NULL;
  type_bridge_canonical_bytes_t *direct_relation_bytes = NULL;
  type_bridge_canonical_bytes_t *remote_entity_bytes = NULL;
  type_bridge_canonical_bytes_t *remote_relation_bytes = NULL;
  type_bridge_byte_view_t direct_entity_view = {0};
  type_bridge_byte_view_t direct_relation_view = {0};
  type_bridge_byte_view_t remote_entity_view = {0};
  type_bridge_byte_view_t remote_relation_view = {0};
  fixture_person *detached_person = NULL;
  fixture_person_ref *detached_person_ref = NULL;
  fixture_employment_employee_player *rebound_player = NULL;
  fixture_person_ref *rebound_person_ref = NULL;
  fixture_employment_create *rebound_create = NULL;
  fixture_employment *updated_employment = NULL;
  type_bridge_byte_view_t employment_iid = {0};
  uint8_t employment_iid_storage[256];
  uint64_t count_before = 0u;
  uint64_t count_after = 0u;
  caller_http_body_t advertisement = {0};
  caller_http_body_t response = {0};
  fixture_query_remote_context *remote_context = NULL;
  fixture_query_rows_remote_pending *pending = NULL;
  fixture_query_rows_remote_claim *claim = NULL;
  type_bridge_byte_view_t request = {0};
  size_t response_limit = 0u;
  size_t row_count = 0u;

  if (directory == NULL) return 0;
  CHECK(remote_port != 0u);
  CHECK(open_person_create(package, "v5-live-person", 70, &person_create,
                           out_diagnostics) == 0);
  CHECK(fixture_person_database_insert(database, person_create, NULL,
                                       &inserted_person, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_reference(inserted_person, &inserted_person_ref,
                                 out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_player_from_person(
            inserted_person_ref, &employee_player, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  employment_args.struct_size = sizeof(employment_args);
  employment_args.version = FIXTURE_CREATE_ARGS_VERSION;
  employment_args.role_employee = employee_player;
  CHECK(fixture_employment_create_open(package, &employment_args,
                                       &employment_create, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_database_insert(
            database, employment_create, NULL, &inserted_employment,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_iid(inserted_employment, &employment_iid,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(employment_iid.length != 0u &&
        employment_iid.length <= sizeof(employment_iid_storage));
  memcpy(employment_iid_storage, employment_iid.data, employment_iid.length);
  employment_iid.data = employment_iid_storage;

  CHECK(fixture_query_session_open(package, &session, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_query_exact_binding_open(
            session, &employment_binding, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_binding_open(
            session, &person_binding, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_query_role_from_exact(
            employment_binding, &employee_role, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_query_role_connects(
            employee_role,
            fixture_employment_employee_query_role_player_person_exact(
                person_binding),
            &role_predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier_query_field_from_exact(
            person_binding, &identifier_field, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_open(package, view_of("v5-live-person"), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier_query_field_compare_value(
            identifier_field, TYPE_BRIDGE_QUERY_COMPARE_EQUAL, identifier,
            &identifier_predicate, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_and(role_predicate, identifier_predicate,
                                    &predicate, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_query_exact_one_selection_open(
            employment_binding, &employment_selection, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_selection_open(
            person_binding, &person_selection, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_positional_2(
            session,
            fixture_employment_query_exact_one_selection_ref(
                employment_selection),
            fixture_person_query_exact_one_selection_ref(person_selection),
            &shape, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_where(shape, predicate, &query, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_one(query, NULL, 0u, &terminal, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_database_query_execute_rows(
            database, terminal, &limits, NULL, &direct_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_result_row_count(
            fixture_query_rows_result_ref(direct_result), &row_count,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        row_count == 1u);
  CHECK(fixture_employment_query_exact_one_at(
            fixture_employment_query_exact_one_result_slot_v1_t_rows(
                direct_result, 0u),
            0u, &direct_employment, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(direct_result,
                                                                  1u),
            0u, &direct_person, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_canonical_encode(direct_person, &direct_entity_bytes,
                                        out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_canonical_encode(
            direct_employment, &direct_relation_bytes, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_view(direct_entity_bytes,
                                         &direct_entity_view) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_view(direct_relation_bytes,
                                         &direct_relation_view) ==
        TYPE_BRIDGE_STATUS_OK);

  CHECK(caller_http_exchange(remote_port, "GET", "/v2/capabilities", NULL, 0u,
                             1024u * 1024u, &advertisement));
  CHECK(fixture_query_remote_context_open(
            package,
            (type_bridge_byte_view_t){advertisement.data,
                                      advertisement.length},
            &limits, &remote_context, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  free(advertisement.data);
  CHECK(fixture_query_remote_prepare_rows(remote_context, terminal, NULL,
                                          &pending, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_pending_request_bytes(pending, &request) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_pending_response_snapshot_limit(
            pending, &response_limit) == TYPE_BRIDGE_STATUS_OK);
  CHECK(caller_http_exchange(remote_port, "POST", "/v2/query", request.data,
                             request.length, response_limit, &response));
  CHECK(fixture_query_rows_remote_pending_claim(
            pending, NULL, &claim, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_pending_close(&pending) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_remote_claim_decode(
            claim, NULL,
            (type_bridge_byte_view_t){response.data, response.length},
            &remote_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  free(response.data);
  CHECK(fixture_query_rows_remote_claim_close(&claim) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_query_exact_one_at(
            fixture_employment_query_exact_one_result_slot_v1_t_rows(
                remote_result, 0u),
            0u, &remote_employment, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_at(
            fixture_person_query_exact_one_result_slot_v1_t_rows(remote_result,
                                                                  1u),
            0u, &remote_person, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_canonical_encode(remote_person, &remote_entity_bytes,
                                        out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_canonical_encode(
            remote_employment, &remote_relation_bytes, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_view(remote_entity_bytes,
                                         &remote_entity_view) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_view(remote_relation_bytes,
                                         &remote_relation_view) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(direct_entity_view.length == remote_entity_view.length &&
        memcmp(direct_entity_view.data, remote_entity_view.data,
               direct_entity_view.length) == 0);
  CHECK(direct_relation_view.length == remote_relation_view.length &&
        memcmp(direct_relation_view.data, remote_relation_view.data,
               direct_relation_view.length) == 0);

  CHECK(fixture_person_canonical_decode(package, direct_entity_view,
                                        &detached_person, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_database_count(database, NULL, &count_before,
                                          out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_reference(detached_person, &detached_person_ref,
                                 out_diagnostics) ==
        TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(detached_person_ref == NULL &&
        execution_code_is(*out_diagnostics, "projected_snapshot_detached"));
  CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_database_count(database, NULL, &count_after,
                                          out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count_after == count_before);

  CHECK(fixture_person_reference(direct_person, &rebound_person_ref,
                                 out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_player_from_person(
            rebound_person_ref, &rebound_player, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  employment_args.role_employee = rebound_player;
  CHECK(fixture_employment_create_open(package, &employment_args,
                                       &rebound_create, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_database_update(
            database, employment_iid, rebound_create, NULL,
            &updated_employment, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(write_canonical_view(directory, "entity.bin", direct_entity_view));
  CHECK(write_canonical_view(directory, "relation.bin", direct_relation_view));

  CHECK(fixture_employment_database_delete_by_iid(
            database, employment_iid, NULL, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t person_iid = {0};
    CHECK(fixture_person_iid(inserted_person, &person_iid, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_database_delete_by_iid(database, person_iid, NULL,
                                                out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_employment_close(&updated_employment) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_create_close(&rebound_create) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_player_close(&rebound_player) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&rebound_person_ref) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_close(&detached_person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_close(&remote_relation_bytes) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_close(&remote_entity_bytes) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_close(&direct_relation_bytes) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_canonical_bytes_close(&direct_entity_bytes) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_close(&remote_person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_close(&remote_employment) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&remote_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_remote_context_close(&remote_context) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_close(&direct_person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_close(&direct_employment) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_result_close(&direct_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_rows_terminal_close(&terminal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&query) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&shape) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_one_selection_close(&person_selection) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_query_exact_one_selection_close(
            &employment_selection) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&predicate) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&identifier_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_predicate_close(&role_predicate) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_identifier_query_field_close(&identifier_field) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_query_role_close(&employee_role) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_query_exact_binding_close(&person_binding) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_query_exact_binding_close(&employment_binding) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_session_close(&session) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_close(&inserted_employment) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_create_close(&employment_create) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_employment_employee_player_close(&employee_player) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&inserted_person_ref) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_close(&inserted_person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_create_close(&person_create) == TYPE_BRIDGE_STATUS_OK);
  puts("Sdk V5 C live codec direct/remote parity: passed");
  return 0;
}
#endif

int main(void) {
  const char *address = getenv("TYPEDB_ADDRESS");
  const char *database_name = getenv("TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
  const uint32_t http_port = required_http_port();
  type_bridge_schema_package_t *package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_read_transaction_t *read_tx = NULL;
  type_bridge_write_transaction_t *write_tx = NULL;
  fixture_person_create *ada_create = NULL;
  fixture_person_create *ada_update = NULL;
  fixture_person_create *ada_tx_update = NULL;
  fixture_person_create *bob_create = NULL;
  fixture_person_create *cara_create = NULL;
  fixture_person_create *dana_create = NULL;
  fixture_employee_create *query_employee_create = NULL;
  fixture_manager_create *query_manager_create = NULL;
  fixture_membership_create *query_membership_create = NULL;
  fixture_networkzhlink_create *query_network_link_create = NULL;
  fixture_membership_create *ada_membership_create = NULL;
  fixture_membership_create *cara_membership_create = NULL;
  fixture_robot_create *robot_create = NULL;
  fixture_robot_create_args_v1_t robot_args = {0};
  fixture_robotzuid *robot_id = NULL;
  fixture_valzuconstrained *robot_constrained = NULL;
  fixture_person *person = NULL;
  fixture_employee *query_employee = NULL;
  fixture_manager *query_manager = NULL;
  fixture_membership *membership = NULL;
  fixture_networkzhlink *query_network_link = NULL;
  uint8_t ada_iid_storage[256];
  uint8_t bob_iid_storage[256];
  uint8_t cara_iid_storage[256];
  uint8_t dana_iid_storage[256];
  uint8_t query_employee_iid_storage[256];
  uint8_t query_manager_iid_storage[256];
  uint8_t query_membership_iid_storage[256];
  uint8_t query_network_link_iid_storage[256];
  uint8_t membership_one_iid_storage[256];
  uint8_t membership_two_iid_storage[256];
  uint8_t membership_three_iid_storage[256];
  uint8_t membership_four_iid_storage[256];
  size_t ada_iid_length = 0u;
  size_t bob_iid_length = 0u;
  size_t cara_iid_length = 0u;
  size_t dana_iid_length = 0u;
  size_t query_employee_iid_length = 0u;
  size_t query_manager_iid_length = 0u;
  size_t query_membership_iid_length = 0u;
  size_t query_network_link_iid_length = 0u;
  size_t membership_one_iid_length = 0u;
  size_t membership_two_iid_length = 0u;
  size_t membership_three_iid_length = 0u;
  size_t membership_four_iid_length = 0u;
  char entity_lifecycle_key[64] = {0};
  char relation_lifecycle_player_key[64] = {0};
  int entity_created = 0;
  int entity_read_after_create = 0;
  int entity_deleted = 0;
  int relation_created = 0;
  int relation_deleted = 0;
  uint64_t count = 0u;
  type_bridge_byte_view_t version = {NULL, 0u};
  type_bridge_runtime_config_v1_t runtime_config = {
      sizeof(type_bridge_runtime_config_v1_t),
      TYPE_BRIDGE_RUNTIME_CONFIG_VERSION,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN,
      0u,
      {0u, 0u, 0u, 0u}};
  type_bridge_database_config_v1_t database_config;

  CHECK(address != NULL && address[0] != '\0');
  CHECK(database_name != NULL && database_name[0] != '\0');
  CHECK(username != NULL && username[0] != '\0');
  CHECK(password != NULL);
  CHECK(http_port != 0u);
  memset(&database_config, 0, sizeof(database_config));
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  database_config.address = view_of(address);
  database_config.database = view_of(database_name);
  database_config.username = view_of(username);
  database_config.password = view_of(password);
  database_config.http_port = http_port;
  database_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;

  CHECK(fixture_schema_package_open(&package, &package_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && package_diagnostics == NULL);
  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_open_v1(runtime, package, &database_config, NULL,
                                     &database, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_server_version(database, &version) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(version, "3.12.3"));
#ifdef TYPE_BRIDGE_SDK_V5_C_CODEC
  CHECK(run_sdk_v5_live_codec(package, database, &diagnostics) == 0);
  CHECK(type_bridge_database_close(&database, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  CHECK(database == NULL && runtime == NULL && package == NULL &&
        diagnostics == NULL);
  return 0;
#endif

  CHECK(fixture_robotzuid_open(package, 7, &robot_id, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_open(package, 10, &robot_constrained,
                                      &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  robot_args.struct_size = sizeof(robot_args);
  robot_args.version = FIXTURE_CREATE_ARGS_VERSION;
  robot_args.field_robotzuid = robot_id;
  robot_args.field_valzuconstrained = robot_constrained;
  CHECK(fixture_robot_create_open(package, &robot_args, &robot_create,
                                  &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robotzuid_close(&robot_id) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_close(&robot_constrained) ==
        TYPE_BRIDGE_STATUS_OK);
  person = (fixture_person *)(uintptr_t)1u;
  CHECK(fixture_person_database_insert(
            database, (const fixture_person_create *)robot_create, NULL,
            &person, &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(person == NULL && diagnostics != NULL);
  CHECK(execution_code_is(diagnostics, "c_entity_create_model_mismatch"));
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_database_count(database, NULL, &count, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 0u);
  CHECK(fixture_robot_database_count(database, NULL, &count, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 0u);
  CHECK(fixture_robot_create_close(&robot_create) == TYPE_BRIDGE_STATUS_OK);
  puts("P4 explicit create cast rejected pre-I/O: passed");

  CHECK(open_person_create(package, "query-ada", 36, &ada_create, &diagnostics) ==
        0);
  CHECK(fixture_person_database_insert(database, ada_create, NULL, &person,
                                       &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  entity_created = 1;
  CHECK(check_person(person, "query-ada", 36, &diagnostics) == 0);
  CHECK(read_query_person_identifier(person, entity_lifecycle_key,
                                     sizeof(entity_lifecycle_key),
                                     &diagnostics) == 0);
  CHECK(copy_person_iid(person, ada_iid_storage, sizeof(ada_iid_storage),
                        &ada_iid_length, &diagnostics) == 0);
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_create_close(&ada_create) == TYPE_BRIDGE_STATUS_OK);
  puts("P4 database insert: passed");

  {
    const type_bridge_byte_view_t ada_iid = {ada_iid_storage, ada_iid_length};
    CHECK(check_reference_constructors(package, ada_iid, &diagnostics) == 0);
    puts("P4 nominal references: passed");
    CHECK(fixture_person_database_get_by_iid(database, ada_iid, NULL, &person,
                                             &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_person(person, "query-ada", 36, &diagnostics) == 0);
    entity_read_after_create = 1;
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 database get: passed");
    CHECK(fixture_person_database_count(database, NULL, &count, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 1u);
    puts("P4 database count: passed");

    CHECK(open_person_create(package, "query-ada", 37, &ada_update, &diagnostics) ==
          0);
    CHECK(fixture_person_database_update(database, ada_iid, ada_update, NULL,
                                         &person, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_person(person, "query-ada", 37, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_create_close(&ada_update) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 database update: passed");

    CHECK(open_person_create(package, "query-ada", 38, &ada_tx_update,
                             &diagnostics) == 0);
    CHECK(open_person_create(package, "bob", 20, &bob_create, &diagnostics) ==
          0);
    CHECK(open_person_create(package, "cara", 25, &cara_create,
                             &diagnostics) == 0);
    CHECK(type_bridge_write_transaction_open(database, NULL, &write_tx,
                                             &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_write_transaction_insert(
              write_tx, bob_create, NULL, &person, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_person_iid(person, bob_iid_storage, sizeof(bob_iid_storage),
                          &bob_iid_length, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_create_close(&bob_create) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 write transaction insert: passed");

    CHECK(fixture_person_write_transaction_put(
              write_tx, cara_create, NULL, &person, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_person_iid(person, cara_iid_storage, sizeof(cara_iid_storage),
                          &cara_iid_length, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_create_close(&cara_create) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 write transaction put: passed");

    CHECK(fixture_person_write_transaction_get_by_iid(
              write_tx, ada_iid, NULL, &person, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_person(person, "query-ada", 37, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 write transaction get: passed");

    CHECK(fixture_person_write_transaction_update(
              write_tx, ada_iid, ada_tx_update, NULL, &person,
              &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(check_person(person, "query-ada", 38, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_create_close(&ada_tx_update) ==
          TYPE_BRIDGE_STATUS_OK);
    puts("P4 write transaction update: passed");

    {
      const type_bridge_byte_view_t bob_iid = {bob_iid_storage,
                                               bob_iid_length};
      CHECK(fixture_person_write_transaction_delete_by_iid(
                write_tx, bob_iid, NULL, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
    }
    puts("P4 write transaction delete: passed");
    CHECK(fixture_person_write_transaction_count(write_tx, NULL, &count,
                                                  &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 2u);
    puts("P4 write transaction count: passed");
    CHECK(type_bridge_write_transaction_commit(&write_tx, NULL,
                                               &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(write_tx == NULL && diagnostics == NULL);

    CHECK(type_bridge_read_transaction_open(database, NULL, &read_tx,
                                            &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_read_transaction_get_by_iid(
              read_tx, ada_iid, NULL, &person, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(check_person(person, "query-ada", 38, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 read transaction get: passed");
    CHECK(fixture_person_read_transaction_count(read_tx, NULL, &count,
                                                 &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(count == 2u);
    puts("P4 read transaction count: passed");
    CHECK(type_bridge_read_transaction_close(&read_tx, &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(read_tx == NULL && diagnostics == NULL);

    CHECK(open_person_create(package, "query-dana", 45, &dana_create,
                             &diagnostics) == 0);
    CHECK(fixture_person_database_put(database, dana_create, NULL, &person,
                                      &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_person_iid(person, dana_iid_storage, sizeof(dana_iid_storage),
                          &dana_iid_length, &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_create_close(&dana_create) == TYPE_BRIDGE_STATUS_OK);
    puts("P4 database put: passed");

    CHECK(open_employee_create(package, "query-employee", 3,
                               &query_employee_create, &diagnostics) == 0);
    CHECK(fixture_employee_database_insert(
              database, query_employee_create, NULL, &query_employee,
              &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    {
      type_bridge_byte_view_t iid = {NULL, 0u};
      CHECK(fixture_employee_iid(query_employee, &iid, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(iid.data != NULL && iid.length != 0u &&
            iid.length <= sizeof(query_employee_iid_storage));
      memcpy(query_employee_iid_storage, iid.data, iid.length);
      query_employee_iid_length = iid.length;
    }
    CHECK(fixture_employee_close(&query_employee) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_create_close(&query_employee_create) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(open_manager_create(package, "query-manager", 5,
                              &query_manager_create, &diagnostics) == 0);
    CHECK(fixture_manager_database_insert(
              database, query_manager_create, NULL, &query_manager,
              &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    {
      type_bridge_byte_view_t iid = {NULL, 0u};
      CHECK(fixture_manager_iid(query_manager, &iid, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(iid.data != NULL && iid.length != 0u &&
            iid.length <= sizeof(query_manager_iid_storage));
      memcpy(query_manager_iid_storage, iid.data, iid.length);
      query_manager_iid_length = iid.length;
    }
    CHECK(fixture_manager_close(&query_manager) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_manager_create_close(&query_manager_create) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(open_membership_create(package, ada_iid, &query_membership_create,
                                 &diagnostics) == 0);
    CHECK(fixture_membership_database_insert(
              database, query_membership_create, NULL, &membership,
              &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    relation_created = 1;
    CHECK(fixture_person_database_get_by_iid(database, ada_iid, NULL, &person,
                                             &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(read_query_person_identifier(person, relation_lifecycle_player_key,
                                       sizeof(relation_lifecycle_player_key),
                                       &diagnostics) == 0);
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_membership_iid(membership, query_membership_iid_storage,
                              sizeof(query_membership_iid_storage),
                              &query_membership_iid_length, &diagnostics) == 0);
    CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_create_close(&query_membership_create) ==
          TYPE_BRIDGE_STATUS_OK);

    CHECK(open_network_link_create(package, ada_iid,
                                   (type_bridge_byte_view_t){dana_iid_storage,
                                                             dana_iid_length},
                                   &query_network_link_create,
                                   &diagnostics) == 0);
    CHECK(fixture_networkzhlink_database_insert(
              database, query_network_link_create, NULL, &query_network_link,
              &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    {
      type_bridge_byte_view_t iid = {NULL, 0u};
      CHECK(fixture_networkzhlink_iid(query_network_link, &iid,
                                      &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(iid.data != NULL && iid.length != 0u &&
            iid.length <= sizeof(query_network_link_iid_storage));
      memcpy(query_network_link_iid_storage, iid.data, iid.length);
      query_network_link_iid_length = iid.length;
    }
    CHECK(fixture_networkzhlink_close(&query_network_link) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_create_close(&query_network_link_create) ==
          TYPE_BRIDGE_STATUS_OK);
    puts("C14/C16/C17 query fixture subtype and relation data: passed");

    CHECK(run_typed_query_function_and_remote(
              package, database, ada_iid,
              (type_bridge_byte_view_t){dana_iid_storage, dana_iid_length},
              (type_bridge_byte_view_t){query_membership_iid_storage,
                                        query_membership_iid_length},
              (type_bridge_byte_view_t){query_network_link_iid_storage,
                                        query_network_link_iid_length},
                                              &diagnostics) == 0);

    CHECK(fixture_networkzhlink_database_delete_by_iid(
              database,
              (type_bridge_byte_view_t){query_network_link_iid_storage,
                                        query_network_link_iid_length},
              NULL, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_membership_database_delete_by_iid(
              database,
              (type_bridge_byte_view_t){query_membership_iid_storage,
                                        query_membership_iid_length},
              NULL, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    relation_deleted = 1;
    CHECK(emit_v2_observationf(
        "relation_lifecycle", "direct_runtime",
        "{\"created\":%s,\"deleted\":%s,\"model\":\"membership\","
        "\"player_key\":\"%s\",\"role\":\"member\"}",
        relation_created ? "true" : "false",
        relation_deleted ? "true" : "false", relation_lifecycle_player_key));
    CHECK(fixture_manager_database_delete_by_iid(
              database,
              (type_bridge_byte_view_t){query_manager_iid_storage,
                                        query_manager_iid_length},
              NULL, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_employee_database_delete_by_iid(
              database,
              (type_bridge_byte_view_t){query_employee_iid_storage,
                                        query_employee_iid_length},
              NULL, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    puts("C14/C16/C17 query fixture cleanup: passed");

    {
      const type_bridge_byte_view_t cara_iid = {cara_iid_storage,
                                                cara_iid_length};
      type_bridge_byte_view_t actual_iid = {NULL, 0u};
      fixture_membership_ref *membership_reference = NULL;

      CHECK(open_membership_create(package, ada_iid,
                                   &ada_membership_create, &diagnostics) == 0);
      CHECK(open_membership_create(package, cara_iid,
                                   &cara_membership_create, &diagnostics) == 0);
      puts("P5 generated Person role-player unions: passed");

      CHECK(fixture_membership_database_insert(
                database, ada_membership_create, NULL, &membership,
                &diagnostics) == TYPE_BRIDGE_STATUS_OK);
      CHECK(check_membership(membership, ada_iid, &diagnostics) == 0);
      CHECK(copy_membership_iid(
                membership, membership_one_iid_storage,
                sizeof(membership_one_iid_storage),
                &membership_one_iid_length, &diagnostics) == 0);
      CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
      puts("P5 database relation insert: passed");

      {
        const type_bridge_byte_view_t membership_one_iid = {
            membership_one_iid_storage, membership_one_iid_length};
        CHECK(fixture_membership_ref_from_iid(
                  package, membership_one_iid, &membership_reference,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(fixture_membership_ref_iid(membership_reference, &actual_iid,
                                         &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(actual_iid.length == membership_one_iid.length &&
              memcmp(actual_iid.data, membership_one_iid.data,
                     membership_one_iid.length) == 0);
        CHECK(fixture_membership_ref_close(&membership_reference) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 nominal relation reference: passed");

        CHECK(fixture_membership_database_get_by_iid(
                  database, membership_one_iid, NULL, &membership,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, ada_iid, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 database relation get: passed");
        CHECK(fixture_membership_database_count(database, NULL, &count,
                                                &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(count == 1u);
        puts("P5 database relation count: passed");

        CHECK(fixture_membership_database_update(
                  database, membership_one_iid, cara_membership_create, NULL,
                  &membership, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, cara_iid, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 database relation update: passed");

        CHECK(fixture_membership_database_put(
                  database, ada_membership_create, NULL, &membership,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, ada_iid, &diagnostics) == 0);
        CHECK(copy_membership_iid(
                  membership, membership_two_iid_storage,
                  sizeof(membership_two_iid_storage),
                  &membership_two_iid_length, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 database relation put: passed");

        CHECK(type_bridge_write_transaction_open(database, NULL, &write_tx,
                                                 &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(fixture_membership_write_transaction_insert(
                  write_tx, ada_membership_create, NULL, &membership,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, ada_iid, &diagnostics) == 0);
        CHECK(copy_membership_iid(
                  membership, membership_three_iid_storage,
                  sizeof(membership_three_iid_storage),
                  &membership_three_iid_length, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 write transaction relation insert: passed");

        CHECK(fixture_membership_write_transaction_put(
                  write_tx, cara_membership_create, NULL, &membership,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, cara_iid, &diagnostics) == 0);
        CHECK(copy_membership_iid(
                  membership, membership_four_iid_storage,
                  sizeof(membership_four_iid_storage),
                  &membership_four_iid_length, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 write transaction relation put: passed");

        CHECK(fixture_membership_write_transaction_get_by_iid(
                  write_tx, membership_one_iid, NULL, &membership,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, cara_iid, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 write transaction relation get: passed");

        CHECK(fixture_membership_write_transaction_update(
                  write_tx, membership_one_iid, ada_membership_create, NULL,
                  &membership, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, ada_iid, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 write transaction relation update: passed");

        {
          const type_bridge_byte_view_t membership_three_iid = {
              membership_three_iid_storage, membership_three_iid_length};
          CHECK(fixture_membership_write_transaction_delete_by_iid(
                    write_tx, membership_three_iid, NULL, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
        }
        puts("P5 write transaction relation delete: passed");
        CHECK(fixture_membership_write_transaction_count(
                  write_tx, NULL, &count, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(count == 3u);
        puts("P5 write transaction relation count: passed");
        CHECK(type_bridge_write_transaction_commit(&write_tx, NULL,
                                                   &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(write_tx == NULL && diagnostics == NULL);

        CHECK(type_bridge_read_transaction_open(database, NULL, &read_tx,
                                                &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(fixture_membership_read_transaction_get_by_iid(
                  read_tx, membership_one_iid, NULL, &membership,
                  &diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(check_membership(membership, ada_iid, &diagnostics) == 0);
        CHECK(fixture_membership_close(&membership) ==
              TYPE_BRIDGE_STATUS_OK);
        puts("P5 read transaction relation get: passed");
        CHECK(fixture_membership_read_transaction_count(
                  read_tx, NULL, &count, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(count == 3u);
        puts("P5 read transaction relation count: passed");
        CHECK(type_bridge_read_transaction_close(&read_tx, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        CHECK(read_tx == NULL && diagnostics == NULL);

        CHECK(fixture_membership_database_delete_by_iid(
                  database, membership_one_iid, NULL, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK);
        {
          const type_bridge_byte_view_t membership_two_iid = {
              membership_two_iid_storage, membership_two_iid_length};
          const type_bridge_byte_view_t membership_four_iid = {
              membership_four_iid_storage, membership_four_iid_length};
          CHECK(fixture_membership_database_delete_by_iid(
                    database, membership_two_iid, NULL, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
          CHECK(fixture_membership_database_delete_by_iid(
                    database, membership_four_iid, NULL, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
          CHECK(fixture_membership_database_delete_by_iid(
                    database, membership_four_iid, NULL, &diagnostics) ==
                TYPE_BRIDGE_STATUS_OK);
        }
        puts("P5 database relation delete and repeated delete: passed");
      }

      CHECK(fixture_membership_database_count(database, NULL, &count,
                                              &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(count == 0u);
      puts("P5 final relation count zero: passed");
      CHECK(fixture_membership_create_close(&cara_membership_create) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_membership_create_close(&ada_membership_create) ==
            TYPE_BRIDGE_STATUS_OK);
      puts("P5 generated C relation CRUD journey: passed");
    }

    CHECK(fixture_person_database_delete_by_iid(database, ada_iid, NULL,
                                                &diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    entity_deleted = 1;
    {
      const type_bridge_byte_view_t cara_iid = {cara_iid_storage,
                                                cara_iid_length};
      const type_bridge_byte_view_t dana_iid = {dana_iid_storage,
                                                dana_iid_length};
      CHECK(fixture_person_database_delete_by_iid(database, cara_iid, NULL,
                                                  &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_person_database_delete_by_iid(database, dana_iid, NULL,
                                                  &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
      CHECK(fixture_person_database_delete_by_iid(database, dana_iid, NULL,
                                                  &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
    }
    puts("P4 database delete and repeated delete: passed");
  }

  CHECK(fixture_person_database_count(database, NULL, &count, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(count == 0u);
  CHECK(emit_v2_observationf(
      "entity_lifecycle", "direct_runtime",
      "{\"created\":%s,\"deleted\":%s,\"key\":\"%s\","
      "\"model\":\"person\",\"read_after_create\":%s}",
      entity_created ? "true" : "false", entity_deleted ? "true" : "false",
      entity_lifecycle_key, entity_read_after_create ? "true" : "false"));
  puts("P4 final database count zero: passed");
  CHECK(type_bridge_database_close(&database, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  CHECK(database == NULL && runtime == NULL && package == NULL &&
        diagnostics == NULL);
  puts("P4 generated C entity CRUD journey: passed");
  return 0;
}
