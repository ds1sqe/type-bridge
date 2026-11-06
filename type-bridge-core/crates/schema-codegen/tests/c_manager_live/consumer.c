#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <manager/models.h>
#include <manager_foreign/models.h>

/* These fixtures expect one root. Exercise the result owner, then its item. */
#define TEST_SINGLE_WRITE(MODEL, OP, CALL, OUT, DIAG)                           \
  ((write_status = (CALL)) != TYPE_BRIDGE_STATUS_OK ? write_status :            \
   ((write_status = MODEL##_##OP##_batch_result_item(                           \
         MODEL##_##OP##_result, (OUT), (DIAG))) != TYPE_BRIDGE_STATUS_OK ?       \
    ((void)MODEL##_##OP##_batch_result_close(&MODEL##_##OP##_result),            \
     write_status) :                                                          \
    (MODEL##_##OP##_batch_result_close(&MODEL##_##OP##_result) !=               \
         TYPE_BRIDGE_STATUS_OK ? TYPE_BRIDGE_STATUS_INVALID_ARGUMENT :         \
     (*(OUT) != NULL ? TYPE_BRIDGE_STATUS_OK : TYPE_BRIDGE_STATUS_INVALID_ARGUMENT))))


#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "C Manager live failure at line %d\n",         \
              __LINE__);                                                       \
      return __LINE__;                                                         \
    }                                                                          \
  } while (0)

typedef struct iid_copy {
  type_bridge_iid_t *typed;
  uint8_t bytes[256];
  size_t length;
} iid_copy_t;

typedef struct person_inputs {
  manager_identifier *identifier;
  manager_nickname *nickname;
  manager_score *score;
  manager_foozuzubar *foo_bar;
  manager_scorezuzugte *score_gte;
  manager_valzubool *boolean_value;
  manager_valzuconstrained *constrained;
  manager_valzudate *date;
  manager_valzudatetime *datetime;
  manager_valzudatetimezutzz *datetime_tz;
  manager_valzudecimal *decimal;
  manager_valzudouble *double_value;
  manager_valzuduration *duration;
} person_inputs_t;

typedef type_bridge_status_t(TYPE_BRIDGE_CALL *foo_filter_fn)(
    const manager_person_manager *, manager_person_manager_filter_ref_v1_t,
    const manager_foozuzubar *, manager_person_manager_filter **,
    type_bridge_execution_diagnostics_t **);

static type_bridge_byte_view_t view_of(const char *text) {
  return (type_bridge_byte_view_t){(const uint8_t *)text, strlen(text)};
}

static void plan_mark(const char *name) {
  const char *path = getenv("TYPE_BRIDGE_TEST_PLAN_CAPTURE_LABEL");
  if (path != NULL) {
    FILE *file = fopen(path, "w");
    if (file == NULL || fputs(name, file) < 0 || fclose(file) != 0) abort();
  }
}

static int same_text(type_bridge_byte_view_t value, const char *text) {
  const size_t length = strlen(text);
  return value.data != NULL && value.length == length &&
         memcmp(value.data, text, length) == 0;
}

static uint32_t required_http_port(void) {
  const char *value = getenv("TYPE_BRIDGE_MANAGER_LIVE_HTTP_PORT");
  const unsigned char *cursor;
  char *end = NULL;
  unsigned long parsed;
  if (value == NULL || value[0] == '\0') {
    return 0u;
  }
  for (cursor = (const unsigned char *)value; *cursor != '\0'; ++cursor) {
    if (*cursor < (unsigned char)'0' || *cursor > (unsigned char)'9') {
      return 0u;
    }
  }
  parsed = strtoul(value, &end, 10);
  if (end == NULL || *end != '\0' || parsed == 0ul || parsed > 65535ul) {
    return 0u;
  }
  return (uint32_t)parsed;
}

static int copy_iid(type_bridge_byte_view_t iid, iid_copy_t *output) {
  if (iid.data == NULL || iid.length == 0u || iid.length > sizeof(output->bytes)) {
    return 0;
  }
  memcpy(output->bytes, iid.data, iid.length);
  output->length = iid.length;
  return type_bridge_iid_open(iid, &output->typed) == TYPE_BRIDGE_STATUS_OK;
}



static int close_person_inputs(person_inputs_t *values) {
  return manager_valzuduration_close(&values->duration) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_valzudouble_close(&values->double_value) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_valzudecimal_close(&values->decimal) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_valzudatetimezutzz_close(&values->datetime_tz) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_valzudatetime_close(&values->datetime) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_valzudate_close(&values->date) == TYPE_BRIDGE_STATUS_OK &&
         manager_valzuconstrained_close(&values->constrained) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_valzubool_close(&values->boolean_value) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_scorezuzugte_close(&values->score_gte) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_foozuzubar_close(&values->foo_bar) == TYPE_BRIDGE_STATUS_OK &&
         manager_score_close(&values->score) == TYPE_BRIDGE_STATUS_OK &&
         manager_nickname_close(&values->nickname) == TYPE_BRIDGE_STATUS_OK &&
         manager_identifier_close(&values->identifier) == TYPE_BRIDGE_STATUS_OK;
}

static int open_person_create(
    const type_bridge_schema_package_t *package, const char *identifier,
    const char *nickname, int64_t score, int64_t foo_bar, int64_t score_gte,
    uint8_t boolean_value, int64_t constrained, const char *date,
    const char *datetime, const char *datetime_tz, const char *decimal,
    uint64_t double_bits, const char *duration,
    manager_person_create **out_create,
    type_bridge_execution_diagnostics_t **diagnostics) {
  person_inputs_t values = {0};
  manager_person_create_args_v1_t args = {0};
  int ok = 0;
  if (manager_identifier_open(package, view_of(identifier), &values.identifier,
                             diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_score_open(package, score, &values.score, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      manager_foozuzubar_open(package, foo_bar, &values.foo_bar, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      manager_scorezuzugte_open(package, score_gte, &values.score_gte,
                               diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_valzubool_open(package, boolean_value, &values.boolean_value,
                            diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_valzuconstrained_open(package, constrained, &values.constrained,
                                   diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_valzudate_open(package, view_of(date), &values.date, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      manager_valzudatetime_open(package, view_of(datetime), &values.datetime,
                                diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_valzudatetimezutzz_open(package, view_of(datetime_tz),
                                     &values.datetime_tz, diagnostics) !=
          TYPE_BRIDGE_STATUS_OK ||
      manager_valzudecimal_open(package, view_of(decimal), &values.decimal,
                               diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_valzudouble_open(package, double_bits, &values.double_value,
                              diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      manager_valzuduration_open(package, view_of(duration), &values.duration,
                                diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto close;
  }
  if (nickname != NULL &&
      manager_nickname_open(package, view_of(nickname), &values.nickname,
                           diagnostics) != TYPE_BRIDGE_STATUS_OK) {
    goto close;
  }
  args.struct_size = sizeof(args);
  args.version = MANAGER_CREATE_ARGS_VERSION;
  args.field_identifier = values.identifier;
  args.field_nickname = values.nickname;
  args.field_score = values.score;
  args.field_foozuzubar = values.foo_bar;
  args.field_scorezuzugte = values.score_gte;
  args.field_valzubool = values.boolean_value;
  args.field_valzuconstrained = values.constrained;
  args.field_valzudate = values.date;
  args.field_valzudatetime = values.datetime;
  args.field_valzudatetimezutzz = values.datetime_tz;
  args.field_valzudecimal = values.decimal;
  args.field_valzudouble = values.double_value;
  args.field_valzuduration = values.duration;
  ok = manager_person_create_open(package, &args, out_create, diagnostics) ==
           TYPE_BRIDGE_STATUS_OK &&
       *out_create != NULL && *diagnostics == NULL;
close:
  return close_person_inputs(&values) && ok;
}

static int exact_diagnostic(type_bridge_execution_diagnostics_t **diagnostics,
                            type_bridge_execution_diagnostic_category_t category,
                            const char *code) {
  type_bridge_execution_diagnostic_view_v1_t view = {0};
  size_t count = 0u;
  int ok = *diagnostics != NULL &&
           type_bridge_execution_diagnostics_count(*diagnostics, &count) ==
               TYPE_BRIDGE_STATUS_OK &&
           count == 1u &&
           type_bridge_execution_diagnostics_get_v1(*diagnostics, 0u, &view) ==
               TYPE_BRIDGE_STATUS_OK &&
           view.category == category && same_text(view.code, code);
  if (!ok && *diagnostics != NULL) {
    fprintf(stderr,
            "C Manager diagnostic mismatch: expected %u/%s, got %u/%.*s "
            "(count=%zu)\n",
            (unsigned)category, code, (unsigned)view.category,
            (int)view.code.length,
            view.code.data == NULL ? "" : (const char *)view.code.data, count);
  }
  return type_bridge_execution_diagnostics_close(diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         *diagnostics == NULL && ok;
}

/*
 * The matching source-level call is a strict -Werror compile-negative in the
 * harness. This deliberately hostile cast additionally pins the C ABI
 * preflight: it fails closed with the typed-query layer's exact diagnostic and
 * without mutating its null output. That diagnostic is deliberately not
 * rebranded as the manager contract's normalized wrong-owner row.
 */
static int wrong_owner_hostile_cast_fails_closed(
    const type_bridge_schema_package_t *package,
    type_bridge_execution_diagnostics_t **diagnostics) {
  manager_query_session *session = NULL;
  manager_robot_query_exact_binding *robot = NULL;
  manager_person_foozuzubar_query_field *field = NULL;
  type_bridge_status_t status;
  int ok = manager_query_session_open(package, &session, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           manager_robot_query_exact_binding_open(session, &robot, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK;
  if (ok) {
    status = manager_person_foozuzubar_query_field_from_exact(
        (const manager_person_query_exact_binding *)(const void *)robot, &field,
        diagnostics);
    if (status != TYPE_BRIDGE_STATUS_INVALID_ARGUMENT || field != NULL) {
      fprintf(stderr,
              "C Manager wrong-owner field status mismatch: got %u, field=%p\n",
              (unsigned)status, (void *)field);
    }
    ok = status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT && field == NULL &&
         exact_diagnostic(diagnostics,
                          TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                          "cross_owner_field");
  }
  if (*diagnostics != NULL) {
    (void)type_bridge_execution_diagnostics_close(diagnostics);
  }
  status = manager_person_foozuzubar_query_field_close(&field);
  if (manager_robot_query_exact_binding_close(&robot) != TYPE_BRIDGE_STATUS_OK ||
      manager_query_session_close(&session) != TYPE_BRIDGE_STATUS_OK) {
    ok = 0;
  }
  return status == TYPE_BRIDGE_STATUS_OK && ok;
}

static int person_key(const manager_person *person, char *output,
                      size_t capacity,
                      type_bridge_execution_diagnostics_t **diagnostics) {
  manager_identifier *identifier = NULL;
  type_bridge_byte_view_t value = {0};
  int ok = manager_person_identifier(person, &identifier, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           manager_identifier_value(identifier, &value, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           value.data != NULL && value.length + 1u <= capacity;
  if (ok) {
    memcpy(output, value.data, value.length);
    output[value.length] = '\0';
  }
  return manager_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK && ok;
}

static int result_mask(manager_person_manager_all_result *result,
                       type_bridge_execution_diagnostics_t **diagnostics,
                       unsigned *out_mask) {
  size_t count = 0u;
  size_t index;
  unsigned mask = 0u;
  if (manager_person_manager_all_result_count(result, &count, diagnostics) !=
      TYPE_BRIDGE_STATUS_OK) {
    return 0;
  }
  for (index = 0u; index < count; ++index) {
    manager_person *person = NULL;
    char key[64] = {0};
    if (manager_person_manager_all_result_at(result, index, &person,
                                            diagnostics) !=
            TYPE_BRIDGE_STATUS_OK ||
        !person_key(person, key, sizeof(key), diagnostics)) {
      manager_person_close(&person);
      return 0;
    }
    if (strcmp(key, "data-ada") == 0) {
      mask |= 1u;
    } else if (strcmp(key, "data-dana") == 0) {
      mask |= 2u;
    } else {
      manager_person_close(&person);
      return 0;
    }
    if (manager_person_close(&person) != TYPE_BRIDGE_STATUS_OK) {
      return 0;
    }
  }
  if ((mask == 1u && count != 1u) || (mask == 2u && count != 1u) ||
      (mask == 3u && count != 2u) || (mask == 0u && count != 0u)) {
    return 0;
  }
  *out_mask = mask;
  return 1;
}

static int filter_all(
    const type_bridge_read_transaction_t *read,
    const manager_person_manager *manager,
    manager_person_manager_filter_ref_v1_t filter,
    const type_bridge_query_execution_limits_v1_t *limits,
    type_bridge_execution_diagnostics_t **diagnostics, unsigned *out_mask) {
  manager_person_manager_all_result *result = NULL;
  int ok = manager_person_manager_read_transaction_all(
               read, manager, filter, limits, NULL, &result, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           result_mask(result, diagnostics, out_mask);
  return manager_person_manager_all_result_close(&result) ==
             TYPE_BRIDGE_STATUS_OK &&
         ok;
}

static int sibling_query_count(
    const type_bridge_schema_package_t *package,
    const type_bridge_read_transaction_t *read,
    const type_bridge_query_execution_limits_v1_t *limits,
    type_bridge_execution_diagnostics_t **diagnostics) {
  manager_query_session *session = NULL;
  manager_person_query_exact_binding *person = NULL;
  manager_person_identifier_query_field *identifier_field = NULL;
  manager_person_query_exact_one_selection *selection = NULL;
  manager_identifier *identifier = NULL;
  manager_query_predicate *predicate = NULL;
  manager_query *root = NULL;
  manager_query *filtered = NULL;
  manager_query_count_terminal *terminal = NULL;
  manager_query_count_result *result = NULL;
  uint64_t count = 0u;
  int ok = manager_query_session_open(package, &session, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           manager_person_query_exact_binding_open(session, &person,
                                                  diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           manager_person_identifier_query_field_from_exact(
               person, &identifier_field, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           manager_identifier_open(package, view_of("data-ada"), &identifier,
                                  diagnostics) == TYPE_BRIDGE_STATUS_OK &&
           manager_person_identifier_query_field_compare_value(
               identifier_field, TYPE_BRIDGE_QUERY_COMPARE_EQUAL, identifier,
               &predicate, diagnostics) == TYPE_BRIDGE_STATUS_OK &&
           manager_person_query_exact_one_selection_open(person, &selection,
                                                        diagnostics) ==
               TYPE_BRIDGE_STATUS_OK &&
           manager_query_positional_1(
               session, manager_person_query_exact_one_selection_ref(selection),
               &root, diagnostics) == TYPE_BRIDGE_STATUS_OK &&
           manager_query_where(root, predicate, &filtered, diagnostics) ==
               TYPE_BRIDGE_STATUS_OK;
  if (ok) {
    manager_query_root_v1_t count_root =
        manager_person_query_exact_binding_root(filtered, person);
    ok = manager_query_count(count_root, &terminal, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_read_transaction_query_execute_count(
             read, terminal, limits, NULL, &result, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_query_count_result_value(result, &count, diagnostics) ==
             TYPE_BRIDGE_STATUS_OK &&
         count == 1u;
  }
  return manager_query_count_result_close(&result) == TYPE_BRIDGE_STATUS_OK &&
         manager_query_count_terminal_close(&terminal) == TYPE_BRIDGE_STATUS_OK &&
         manager_query_close(&filtered) == TYPE_BRIDGE_STATUS_OK &&
         manager_query_close(&root) == TYPE_BRIDGE_STATUS_OK &&
         manager_query_predicate_close(&predicate) == TYPE_BRIDGE_STATUS_OK &&
         manager_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK &&
         manager_person_query_exact_one_selection_close(&selection) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_person_identifier_query_field_close(&identifier_field) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_person_query_exact_binding_close(&person) ==
             TYPE_BRIDGE_STATUS_OK &&
         manager_query_session_close(&session) == TYPE_BRIDGE_STATUS_OK && ok;
}

static long manager_trace_offset(void) {
  const char *path = getenv("SMOKE_TRACE_FILE");
  FILE *file;
  long offset;
  if (path == NULL) return -1;
  file = fopen(path, "r");
  if (file == NULL || fseek(file, 0, SEEK_END) != 0) abort();
  offset = ftell(file);
  if (offset < 0 || fclose(file) != 0) abort();
  return offset;
}

static int check_manager_trace(long offset) {
  const char *path = getenv("SMOKE_TRACE_FILE");
  FILE *file;
  char line[1024];
  unsigned events = 0u;
  if (offset < 0) return 0;
  CHECK(path != NULL);
  file = fopen(path, "r");
  CHECK(file != NULL && fseek(file, offset, SEEK_SET) == 0);
  CHECK(fgets(line, sizeof(line), file) != NULL);
  CHECK(strstr(line, "\"event\":\"request\"") != NULL);
  events++;
  CHECK(fgets(line, sizeof(line), file) != NULL);
  CHECK(strstr(line, "\"event\":\"plan\"") != NULL);
  CHECK(strstr(line, "\"index\":0") != NULL);
  CHECK(strstr(line, "\"result_refs\":0") != NULL);
  CHECK(strstr(line, "\"writes\":false") != NULL);
  events++;
  CHECK(fgets(line, sizeof(line), file) != NULL);
  CHECK(strstr(line, "\"event\":\"commit\"") != NULL);
  CHECK(strstr(line, "\"kind\":\"read\"") != NULL);
  CHECK(strstr(line, "\"committed\":true") != NULL);
  CHECK(strstr(line, "\"code\":\"ok\"") != NULL);
  events++;
  CHECK(fgets(line, sizeof(line), file) == NULL && events == 3u);
  CHECK(fclose(file) == 0);
  return 0;
}

typedef struct manager_read_observation {
  uint64_t count;
  uint8_t exists;
  size_t row_count;
  char iids[32][256];
} manager_read_observation_t;

static int compare_manager_iids(const void *left, const void *right) {
  return strcmp((const char *)left, (const char *)right);
}

static int check_manager_person_terminal_twins(
    const type_bridge_database_t *database,
    const type_bridge_proxy_connection_t *proxy,
    const manager_person_manager *manager, manager_person_manager_filter_ref_v1_t filter,
    type_bridge_query_terminal_kind_t kind,
    type_bridge_execution_diagnostics_t **diagnostics) {
  manager_read_observation_t observations[2];
  size_t target;
  memset(observations, 0, sizeof(observations));
  for (target = 0u; target < 2u; ++target) {
    manager_person_manager_all_result *result = NULL;
    manager_person *first = NULL;
    const long offset = target == 1u ? manager_trace_offset() : -1;
    if (kind == TYPE_BRIDGE_QUERY_TERMINAL_COUNT) {
      CHECK((target == 0u
          ? manager_person_manager_database_count(database, manager, filter, NULL, NULL, &observations[target].count, diagnostics)
          : manager_person_manager_proxy_connection_count(proxy, manager, filter, NULL, NULL, &observations[target].count, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
    } else if (kind == TYPE_BRIDGE_QUERY_TERMINAL_EXISTS) {
      CHECK((target == 0u
          ? manager_person_manager_database_exists(database, manager, filter, NULL, NULL, &observations[target].exists, diagnostics)
          : manager_person_manager_proxy_connection_exists(proxy, manager, filter, NULL, NULL, &observations[target].exists, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
    } else {
      size_t row;
      if (kind == TYPE_BRIDGE_QUERY_TERMINAL_FIRST) {
        CHECK((target == 0u
            ? manager_person_manager_database_first(database, manager, filter, NULL, NULL, &first, diagnostics)
            : manager_person_manager_proxy_connection_first(proxy, manager, filter, NULL, NULL, &first, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
        observations[target].row_count = first == NULL ? 0u : 1u;
      } else {
        CHECK((target == 0u
            ? manager_person_manager_database_all(database, manager, filter, NULL, NULL, &result, diagnostics)
            : manager_person_manager_proxy_connection_all(proxy, manager, filter, NULL, NULL, &result, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
        CHECK(manager_person_manager_all_result_count(result, &observations[target].row_count, diagnostics) == TYPE_BRIDGE_STATUS_OK);
      }
      CHECK(observations[target].row_count <= 32u);
      for (row = 0u; row < observations[target].row_count; ++row) {
        manager_person *thing = first;
        type_bridge_byte_view_t iid;
        first = NULL;
        if (result != NULL) {
          CHECK(manager_person_manager_all_result_at(result, row, &thing, diagnostics) == TYPE_BRIDGE_STATUS_OK);
        }
        CHECK(type_bridge_projected_thing_iid((const type_bridge_projected_thing_t *)thing, &iid) == TYPE_BRIDGE_STATUS_OK);
        CHECK(iid.data != NULL && iid.length > 0u && iid.length < 256u);
        memcpy(observations[target].iids[row], iid.data, iid.length);
        CHECK(manager_person_close(&thing) == TYPE_BRIDGE_STATUS_OK);
      }
      qsort(observations[target].iids, observations[target].row_count,
            sizeof(observations[target].iids[0]), compare_manager_iids);
    }
    CHECK(check_manager_trace(offset) == 0);
    CHECK(manager_person_manager_all_result_close(&result) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(memcmp(&observations[0], &observations[1], sizeof(observations[0])) == 0);
  return 0;
}

static int check_manager_networkzhlink_terminal_twins(
    const type_bridge_database_t *database,
    const type_bridge_proxy_connection_t *proxy,
    const manager_networkzhlink_manager *manager, manager_networkzhlink_manager_filter_ref_v1_t filter,
    type_bridge_query_terminal_kind_t kind,
    type_bridge_execution_diagnostics_t **diagnostics) {
  manager_read_observation_t observations[2];
  size_t target;
  memset(observations, 0, sizeof(observations));
  for (target = 0u; target < 2u; ++target) {
    manager_networkzhlink_manager_all_result *result = NULL;
    manager_networkzhlink *first = NULL;
    const long offset = target == 1u ? manager_trace_offset() : -1;
    if (kind == TYPE_BRIDGE_QUERY_TERMINAL_COUNT) {
      CHECK((target == 0u
          ? manager_networkzhlink_manager_database_count(database, manager, filter, NULL, NULL, &observations[target].count, diagnostics)
          : manager_networkzhlink_manager_proxy_connection_count(proxy, manager, filter, NULL, NULL, &observations[target].count, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
    } else if (kind == TYPE_BRIDGE_QUERY_TERMINAL_EXISTS) {
      CHECK((target == 0u
          ? manager_networkzhlink_manager_database_exists(database, manager, filter, NULL, NULL, &observations[target].exists, diagnostics)
          : manager_networkzhlink_manager_proxy_connection_exists(proxy, manager, filter, NULL, NULL, &observations[target].exists, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
    } else {
      size_t row;
      if (kind == TYPE_BRIDGE_QUERY_TERMINAL_FIRST) {
        CHECK((target == 0u
            ? manager_networkzhlink_manager_database_first(database, manager, filter, NULL, NULL, &first, diagnostics)
            : manager_networkzhlink_manager_proxy_connection_first(proxy, manager, filter, NULL, NULL, &first, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
        observations[target].row_count = first == NULL ? 0u : 1u;
      } else {
        CHECK((target == 0u
            ? manager_networkzhlink_manager_database_all(database, manager, filter, NULL, NULL, &result, diagnostics)
            : manager_networkzhlink_manager_proxy_connection_all(proxy, manager, filter, NULL, NULL, &result, diagnostics)) == TYPE_BRIDGE_STATUS_OK);
        CHECK(manager_networkzhlink_manager_all_result_count(result, &observations[target].row_count, diagnostics) == TYPE_BRIDGE_STATUS_OK);
      }
      CHECK(observations[target].row_count <= 32u);
      for (row = 0u; row < observations[target].row_count; ++row) {
        manager_networkzhlink *thing = first;
        type_bridge_byte_view_t iid;
        first = NULL;
        if (result != NULL) {
          CHECK(manager_networkzhlink_manager_all_result_at(result, row, &thing, diagnostics) == TYPE_BRIDGE_STATUS_OK);
        }
        CHECK(type_bridge_projected_thing_iid((const type_bridge_projected_thing_t *)thing, &iid) == TYPE_BRIDGE_STATUS_OK);
        CHECK(iid.data != NULL && iid.length > 0u && iid.length < 256u);
        memcpy(observations[target].iids[row], iid.data, iid.length);
        CHECK(manager_networkzhlink_close(&thing) == TYPE_BRIDGE_STATUS_OK);
      }
      qsort(observations[target].iids, observations[target].row_count,
            sizeof(observations[target].iids[0]), compare_manager_iids);
    }
    CHECK(check_manager_trace(offset) == 0);
    CHECK(manager_networkzhlink_manager_all_result_close(&result) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(memcmp(&observations[0], &observations[1], sizeof(observations[0])) == 0);
  return 0;
}

static int check_manager_iid_filters(
    const type_bridge_database_t *database, const manager_person_manager *manager,
    const type_bridge_iid_t *first_iid, const type_bridge_iid_t *second_iid,
    type_bridge_execution_diagnostics_t **diagnostics) {
  const type_bridge_iid_t *iids[] = {first_iid, second_iid};
  manager_person_manager_filter *filter = NULL;
  manager_person *person = NULL;
  uint64_t count = UINT64_MAX;
  uint8_t exists = 1u;
  CHECK(manager_person_manager_filter_iids(manager, manager_person_manager_filter_root(manager),
            iids, 2u, (manager_person_manager_filter **)(void *)&iids[0], diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(iids[0] == first_iid && iids[1] == second_iid && *diagnostics == NULL);
  CHECK(manager_person_manager_filter_iid(manager, manager_person_manager_filter_root(manager),
            first_iid, &filter, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_database_count(database, manager, manager_person_manager_filter_ref(filter),
            NULL, NULL, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 1u);
  CHECK(manager_person_manager_database_first(database, manager, manager_person_manager_filter_ref(filter),
            NULL, NULL, &person, diagnostics) == TYPE_BRIDGE_STATUS_OK && person != NULL);
  CHECK(manager_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_filter_close(&filter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_filter_iids(manager, manager_person_manager_filter_root(manager),
            iids, 2u, &filter, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_database_count(database, manager, manager_person_manager_filter_ref(filter),
            NULL, NULL, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 2u);
  CHECK(manager_person_manager_filter_close(&filter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_filter_iids(manager, manager_person_manager_filter_root(manager),
            NULL, 0u, &filter, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_database_count(database, manager, manager_person_manager_filter_ref(filter),
            NULL, NULL, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 0u);
  CHECK(manager_person_manager_database_exists(database, manager, manager_person_manager_filter_ref(filter),
            NULL, NULL, &exists, diagnostics) == TYPE_BRIDGE_STATUS_OK && exists == 0u);
  CHECK(manager_person_manager_database_first(database, manager, manager_person_manager_filter_ref(filter),
            NULL, NULL, &person, diagnostics) == TYPE_BRIDGE_STATUS_OK && person == NULL);
  CHECK(manager_person_manager_filter_close(&filter) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_manager_read_twins(
    const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database,
    const type_bridge_proxy_connection_t *proxy,
    type_bridge_execution_diagnostics_t **diagnostics) {
  manager_person_manager people;
  manager_networkzhlink_manager links;
  manager_person_manager_filter *absent_person = NULL;
  manager_networkzhlink_manager_filter *absent_link = NULL;
  manager_identifier *absent = NULL;
  const type_bridge_query_terminal_kind_t kinds[] = {
      TYPE_BRIDGE_QUERY_TERMINAL_ROWS, TYPE_BRIDGE_QUERY_TERMINAL_FIRST,
      TYPE_BRIDGE_QUERY_TERMINAL_COUNT, TYPE_BRIDGE_QUERY_TERMINAL_EXISTS};
  size_t index;
  memset(&people, 0, sizeof(people));
  memset(&links, 0, sizeof(links));
  CHECK(manager_person_manager_open(package, &people, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_networkzhlink_manager_open(package, &links, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_identifier_open(package, view_of("manager-absent"), &absent,
                                diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_filter_identifier_eq(&people,
            manager_person_manager_filter_root(&people), absent, &absent_person,
            diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_networkzhlink_manager_filter_identifier_eq(&links,
            manager_networkzhlink_manager_filter_root(&links), absent, &absent_link,
            diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index = 0u; index < sizeof(kinds) / sizeof(kinds[0]); ++index) {
    const int first = kinds[index] == TYPE_BRIDGE_QUERY_TERMINAL_FIRST;
    CHECK(check_manager_person_terminal_twins(database, proxy, &people,
              first ? manager_person_manager_filter_ref(absent_person) : manager_person_manager_filter_root(&people),
              kinds[index], diagnostics) == 0);
    CHECK(check_manager_networkzhlink_terminal_twins(database, proxy, &links,
              first ? manager_networkzhlink_manager_filter_ref(absent_link) : manager_networkzhlink_manager_filter_root(&links),
              kinds[index], diagnostics) == 0);
  }
  CHECK(manager_networkzhlink_manager_filter_close(&absent_link) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_filter_close(&absent_person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_identifier_close(&absent) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_networkzhlink_manager_close(&links) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_close(&people) == TYPE_BRIDGE_STATUS_OK);
  puts("06.6 C manager entity/relation direct and HTTP read twins: passed");
  return 0;
}

static int check_manager_http_if_requested(
    const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database,
    type_bridge_execution_diagnostics_t **diagnostics) {
  const char *port = getenv("TYPE_BRIDGE_C_MANAGER_REMOTE_PORT");
  type_bridge_proxy_connection_options_v1_t options;
  type_bridge_proxy_connection_t *proxy = NULL;
  char endpoint[64];
  if (port == NULL) return 0;
  CHECK(snprintf(endpoint, sizeof(endpoint), "http://127.0.0.1:%s", port) > 0);
  memset(&options, 0, sizeof(options));
  options.struct_size = (uint32_t)sizeof(options);
  options.version = TYPE_BRIDGE_PROXY_CONNECTION_OPTIONS_VERSION;
  options.endpoint = view_of(endpoint);
  options.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  CHECK(type_bridge_proxy_connection_open_v1(package, &options, NULL, &proxy,
                                             diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(check_manager_read_twins(package, database, proxy, diagnostics) == 0);
  CHECK(type_bridge_proxy_connection_close(&proxy) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

int main(void) {
  type_bridge_status_t write_status;
  manager_person_insert_batch_result *manager_person_insert_result = NULL;
  const char *address = getenv("TYPE_BRIDGE_MANAGER_LIVE_ADDRESS");
  const char *database_name =
      getenv("TYPE_BRIDGE_MANAGER_LIVE_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
  /* The harness selects the server from the SDK profile matrix. */
  const char *expected_server_version =
      getenv("TYPE_BRIDGE_EXPECTED_SERVER_VERSION");
  const uint32_t http_port = required_http_port();
  type_bridge_schema_package_t *package = NULL;
  type_bridge_schema_package_t *foreign_package = NULL;
  type_bridge_diagnostics_t *package_diagnostics = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_read_transaction_t *read = NULL;
  type_bridge_runtime_config_v1_t runtime_config = {
      sizeof(type_bridge_runtime_config_v1_t),
      TYPE_BRIDGE_RUNTIME_CONFIG_VERSION,
      TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN,
      0u,
      {0u, 0u, 0u, 0u}};
  type_bridge_database_config_v1_t database_config = {0};
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  type_bridge_byte_view_t version = {0};
  manager_person_create *ada_create = NULL;
  manager_person_create *dana_create = NULL;
  manager_person *person = NULL;
  iid_copy_t ada_iid = {NULL, {0}, 0u};
  iid_copy_t dana_iid = {NULL, {0}, 0u};
  manager_person_manager manager = {0};
  manager_person_manager foreign_manager = {0};
  manager_foozuzubar *seven = NULL;
  manager_foozuzubar *nine = NULL;
  manager_score *forty = NULL;
  manager_identifier *ada_identifier = NULL;
  manager_valzubool *boolean_value = NULL;
  manager_identifier *wrong_domain = NULL;
  manager_person_manager_filter *filter = NULL;
  manager_person_manager_filter *conjunction = NULL;
  manager_person_manager_filter *identity = NULL;
  manager_person_manager_filter *sibling = NULL;
  manager_person_manager_all_result *root_result = NULL;
  foo_filter_fn operations[6] = {
      manager_person_manager_filter_foozuzubar_eq,
      manager_person_manager_filter_foozuzubar_ne,
      manager_person_manager_filter_foozuzubar_gt,
      manager_person_manager_filter_foozuzubar_gte,
      manager_person_manager_filter_foozuzubar_lt,
      manager_person_manager_filter_foozuzubar_lte};
  const unsigned expected_masks[6] = {1u, 2u, 2u, 3u, 0u, 1u};
  size_t index;
  uint64_t count = 0u;
  uint8_t exists = 0u;
  unsigned mask = 0u;

  CHECK(address != NULL && address[0] != '\0');
  CHECK(expected_server_version != NULL &&
        expected_server_version[0] != '\0');
  CHECK(database_name != NULL && database_name[0] != '\0');
  CHECK(username != NULL && username[0] != '\0');
  CHECK(password != NULL && http_port != 0u);
  CHECK(manager_schema_package_open(&package, &package_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        package != NULL && package_diagnostics == NULL);
  CHECK(manager_foreign_schema_package_open(&foreign_package,
                                           &package_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        foreign_package != NULL && package_diagnostics == NULL);
  CHECK(manager_person_manager_open(foreign_package, &foreign_manager,
                                   &diagnostics) ==
            TYPE_BRIDGE_STATUS_EXECUTION_FAILED &&
        foreign_manager.root_filter == NULL &&
        foreign_manager.reserved[0] == 0u &&
        foreign_manager.reserved[1] == 0u &&
        foreign_manager.reserved[2] == 0u &&
        foreign_manager.reserved[3] == 0u &&
        exact_diagnostic(&diagnostics,
                         TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INTEGRITY,
                         "generated_token_package_mismatch"));
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  database_config.address = view_of(address);
  database_config.database = view_of(database_name);
  database_config.username = view_of(username);
  database_config.password = view_of(password);
  database_config.http_port = http_port;
  database_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        runtime != NULL && diagnostics == NULL);
  CHECK(type_bridge_database_open_v1(runtime, package, &database_config, NULL,
                                     &database, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        database != NULL && diagnostics == NULL);
  CHECK(type_bridge_database_server_version(database, &version) ==
            TYPE_BRIDGE_STATUS_OK &&
        same_text(version, expected_server_version));

  limits.items = UINT64_C(100000);
  limits.bytes = UINT64_C(67108864);
  limits.graph_nodes = UINT64_C(1000000);
  limits.attribute_values = UINT64_C(4000000);
  limits.collection_members = UINT64_C(1000000);
  limits.role_players = UINT64_C(1000000);
  CHECK(open_person_create(
      package, "data-ada", "Ada", 38, 7, 40, 0u, 38, "2026-08-12",
      "2026-08-12T09:30:00", "2026-08-12T09:30:00Z", "38.5",
      UINT64_C(0x4043000000000000), "PT38S", &ada_create, &diagnostics));
  CHECK(TEST_SINGLE_WRITE(manager_person, insert, manager_person_database_insert(database, ada_create, NULL,&manager_person_insert_result,
                                      &diagnostics), &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t iid = {0};
    CHECK(manager_person_iid(person, &iid, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &ada_iid));
  }
  CHECK(manager_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_person_create(
      package, "data-dana", NULL, 45, 9, 40, 1u, 45, "2026-08-13",
      "2026-08-13T10:45:00", "2026-08-13T10:45:00Z", "45.5",
      UINT64_C(0x4046800000000000), "PT45S", &dana_create, &diagnostics));
  CHECK(TEST_SINGLE_WRITE(manager_person, insert, manager_person_database_insert(database, dana_create, NULL,&manager_person_insert_result,
                                      &diagnostics), &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  {
    type_bridge_byte_view_t iid = {0};
    CHECK(manager_person_iid(person, &iid, &diagnostics) ==
              TYPE_BRIDGE_STATUS_OK &&
          copy_iid(iid, &dana_iid));
  }
  CHECK(manager_person_close(&person) == TYPE_BRIDGE_STATUS_OK);

  CHECK(manager_person_manager_open(package, &manager, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        diagnostics == NULL);
  CHECK(manager_foozuzubar_open(package, 7, &seven, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        manager_foozuzubar_open(package, 9, &nine, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        manager_score_open(package, 40, &forty, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        manager_identifier_open(package, view_of("data-ada"), &ada_identifier,
                               &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        manager_valzubool_open(package, 1u, &boolean_value, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        manager_identifier_open(package, view_of("wrong-domain"), &wrong_domain,
                               &diagnostics) == TYPE_BRIDGE_STATUS_OK);

  CHECK(check_manager_iid_filters(database, &manager, ada_iid.typed, dana_iid.typed, &diagnostics) == 0);
  CHECK(wrong_owner_hostile_cast_fails_closed(package, &diagnostics));
  CHECK(manager_person_manager_filter_foozuzubar_eq(
            &manager, manager_person_manager_filter_root(&manager),
            (const manager_foozuzubar *)(const void *)wrong_domain, &filter,
            &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
        filter == NULL &&
        exact_diagnostic(&diagnostics,
                         TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                         "wrong_scalar_domain"));
  CHECK(manager_person_manager_filter_valzubool_gt(
            &manager, manager_person_manager_filter_root(&manager),
            boolean_value, &filter, &diagnostics) == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
        filter == NULL &&
        exact_diagnostic(&diagnostics,
                         TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                         "invalid_operator_for_type"));

  CHECK(type_bridge_read_transaction_open_v2(database, &limits, NULL, &read,
                                             &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        read != NULL && diagnostics == NULL);
  CHECK(manager_person_manager_filter_foozuzubar_gte(
            &manager, manager_person_manager_filter_root(&manager), seven,
            &filter, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_read_transaction_first(
            read, &manager, manager_person_manager_filter_ref(filter), &limits,
            NULL, &person, &diagnostics) ==
            TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
        person == NULL &&
        exact_diagnostic(&diagnostics,
                         TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_INVALID_INPUT,
                         "manager_first_requires_identity"));
  CHECK(manager_person_manager_filter_close(&filter) == TYPE_BRIDGE_STATUS_OK);

  for (index = 0u; index < 6u; ++index) {
    static const char *const names[6] = {"eq", "ne", "gt", "gte", "lt", "lte"};
    char label[96];
    snprintf(label, sizeof(label), "manager_field_token_filter.operator.%s", names[index]);
    plan_mark(label);
    CHECK(operations[index](&manager,
                            manager_person_manager_filter_root(&manager), seven,
                            &filter, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(filter_all(read, &manager, manager_person_manager_filter_ref(filter),
                     &limits, &diagnostics, &mask) &&
          mask == expected_masks[index]);
    CHECK(manager_person_manager_filter_close(&filter) == TYPE_BRIDGE_STATUS_OK);
  }

  CHECK(manager_person_manager_filter_foozuzubar_gte(
            &manager, manager_person_manager_filter_root(&manager), seven,
            &filter, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_manager_filter_score_gt(
            &manager, manager_person_manager_filter_ref(filter), forty,
            &conjunction, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  plan_mark("manager_field_token_filter.conjunction");
  CHECK(filter_all(read, &manager,
                   manager_person_manager_filter_ref(conjunction), &limits,
                   &diagnostics, &mask) &&
        mask == 2u);
  CHECK(manager_person_manager_filter_close(&conjunction) ==
            TYPE_BRIDGE_STATUS_OK &&
        manager_person_manager_filter_close(&filter) == TYPE_BRIDGE_STATUS_OK);

  plan_mark("manager_field_token_filter.all");
  CHECK(manager_person_manager_read_transaction_all(
            read, &manager, manager_person_manager_filter_root(&manager),
            &limits, NULL, &root_result, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_mask(root_result, &diagnostics, &mask) && mask == 3u);
  CHECK(manager_person_manager_all_result_close(&root_result) ==
        TYPE_BRIDGE_STATUS_OK);
  plan_mark("manager_field_token_filter.count");
  CHECK(manager_person_manager_read_transaction_count(
            read, &manager, manager_person_manager_filter_root(&manager),
            &limits, NULL, &count, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        count == 2u);
  plan_mark("manager_field_token_filter.exists");
  CHECK(manager_person_manager_read_transaction_exists(
            read, &manager, manager_person_manager_filter_root(&manager),
            &limits, NULL, &exists, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        exists == 1u);

  CHECK(manager_person_manager_filter_identifier_eq(
            &manager, manager_person_manager_filter_root(&manager),
            ada_identifier, &identity, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  plan_mark("manager_field_token_filter.first");
  CHECK(manager_person_manager_read_transaction_first(
            read, &manager, manager_person_manager_filter_ref(identity), &limits,
            NULL, &person, &diagnostics) == TYPE_BRIDGE_STATUS_OK &&
        person != NULL);
  {
    char key[64] = {0};
    CHECK(person_key(person, key, sizeof(key), &diagnostics) &&
          strcmp(key, "data-ada") == 0);
  }
  CHECK(manager_person_close(&person) == TYPE_BRIDGE_STATUS_OK &&
        manager_person_manager_filter_close(&identity) ==
            TYPE_BRIDGE_STATUS_OK);

  plan_mark("manager_field_token_filter.sibling_query_count");
  CHECK(sibling_query_count(package, read, &limits, &diagnostics));
  CHECK(manager_person_manager_filter_foozuzubar_eq(
            &manager, manager_person_manager_filter_root(&manager), nine,
            &sibling, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  plan_mark("manager_field_token_filter.sibling_filter");
  CHECK(filter_all(read, &manager, manager_person_manager_filter_ref(sibling),
                   &limits, &diagnostics, &mask) &&
        mask == 2u);
  CHECK(manager_person_manager_filter_close(&sibling) == TYPE_BRIDGE_STATUS_OK);

  plan_mark("");
  CHECK(check_manager_http_if_requested(package, database, &diagnostics) == 0);

  plan_mark("");
  /*
   * The normalized wrong_field_owner row below is source-bound for C by the
   * strict nominal compile-negative plus the fail-closed hostile ABI probe
   * above. Its typed-query diagnostic is not rebranded as observed manager
   * diagnostics. The wrong_scalar_domain and boolean_ordering rows now observe
   * exact canonical manager-filter construction refusals. The wrong_package row
   * is an exact manager-open diagnostic observed before database/provider setup.
   */
  puts("TYPE_BRIDGE_C_MANAGER_LIVE_FACT\t{\"borrowed_read\":{\"final_state\":\"active\",\"reusable_after_each_terminal\":true,\"sibling_filter_usable\":true},\"conjunction\":{\"authored_order\":[\"foo__bar:gte:7\",\"score:gt:40\"],\"normalized_keys\":[\"data-dana\"]},\"field_token\":{\"attribute\":\"attribute:foo__bar\",\"binding_name\":\"foo__bar\",\"owner\":\"entity:person\"},\"first\":{\"identity_predicate\":\"identifier:eq:data-ada\",\"nonsingular_rejection\":{\"category\":\"invalid_input\",\"code\":\"manager_first_requires_identity\",\"rejected_before_provider_io\":true},\"result\":\"data-ada\",\"strict_singular\":true},\"model\":\"person\",\"operator_literal\":{\"kind\":\"long\",\"value\":\"7\"},\"operator_outcomes\":[{\"normalized_keys\":[\"data-ada\"],\"operator\":\"eq\"},{\"normalized_keys\":[\"data-dana\"],\"operator\":\"ne\"},{\"normalized_keys\":[\"data-dana\"],\"operator\":\"gt\"},{\"normalized_keys\":[\"data-ada\",\"data-dana\"],\"operator\":\"gte\"},{\"normalized_keys\":[],\"operator\":\"lt\"},{\"normalized_keys\":[\"data-ada\"],\"operator\":\"lte\"}],\"rejections\":[{\"category\":\"integrity\",\"code\":\"field_owner_mismatch\",\"kind\":\"wrong_field_owner\",\"rejected_before_provider_io\":true},{\"category\":\"invalid_input\",\"code\":\"wrong_scalar_domain\",\"kind\":\"wrong_scalar_domain\",\"rejected_before_provider_io\":true},{\"category\":\"integrity\",\"code\":\"generated_token_package_mismatch\",\"kind\":\"wrong_package\",\"rejected_before_provider_io\":true},{\"category\":\"invalid_input\",\"code\":\"invalid_operator_for_type\",\"kind\":\"boolean_ordering\",\"rejected_before_provider_io\":true}],\"terminals\":{\"all_normalization\":\"reference_key_ascending\",\"count\":2,\"exists\":true}}");

  CHECK(type_bridge_read_transaction_close(&read, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        read == NULL && diagnostics == NULL);
  CHECK(manager_person_database_delete_by_iid(database, dana_iid.typed,
                                             NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_database_delete_by_iid(database, ada_iid.typed, NULL,
                                             &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(manager_person_database_count(database, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);

  CHECK(manager_identifier_close(&wrong_domain) == TYPE_BRIDGE_STATUS_OK &&
        manager_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK &&
        manager_identifier_close(&ada_identifier) == TYPE_BRIDGE_STATUS_OK &&
        manager_score_close(&forty) == TYPE_BRIDGE_STATUS_OK &&
        manager_foozuzubar_close(&nine) == TYPE_BRIDGE_STATUS_OK &&
        manager_foozuzubar_close(&seven) == TYPE_BRIDGE_STATUS_OK &&
        manager_person_manager_close(&manager) == TYPE_BRIDGE_STATUS_OK &&
        manager_person_create_close(&dana_create) == TYPE_BRIDGE_STATUS_OK &&
        manager_person_create_close(&ada_create) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_iid_close(&ada_iid.typed) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_iid_close(&dana_iid.typed) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_close(&database, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        database == NULL && diagnostics == NULL);
  CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        runtime == NULL && diagnostics == NULL);
  CHECK(type_bridge_schema_package_close(&foreign_package) ==
            TYPE_BRIDGE_STATUS_OK &&
        type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}
