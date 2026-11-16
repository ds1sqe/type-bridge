/* Twelve typed bindings; the runner owns schema, data, and database cleanup. */
#include <assert.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <tb_complex/models.h>

static type_bridge_execution_diagnostics_t *diagnostics;
static type_bridge_database_t *db;
static type_bridge_schema_package_t *package;
static tb_complex_query *query;
static tb_complex_query *page_query;
static void checked(type_bridge_status_t status, int line) {
  if (status != TYPE_BRIDGE_STATUS_OK) {
    type_bridge_execution_diagnostic_view_v1_t error;
    fprintf(stderr, "C query line %d: status %d\n", line, (int)status);
    if (diagnostics && type_bridge_execution_diagnostics_get_v1(
                           diagnostics, 0, &error) == TYPE_BRIDGE_STATUS_OK)
      fprintf(stderr, "%.*s: %.*s\n", (int)error.code.length, error.code.data,
              (int)error.message.length, error.message.data);
    exit(1);
  }
}
#define OK(call) checked((call), __LINE__)
static type_bridge_byte_view_t view(const char *text) {
  type_bridge_byte_view_t value = {(const uint8_t *)text, strlen(text)};
  return value;
}
static void where_take(tb_complex_query_predicate *predicate) {
  tb_complex_query *next = NULL;
  OK(tb_complex_query_where(query, predicate, &next, &diagnostics));
  OK(tb_complex_query_close(&query));
  query = next;
  if (page_query) {
    OK(tb_complex_query_where(page_query, predicate, &next, &diagnostics));
    OK(tb_complex_query_close(&page_query));
    page_query = next;
  }
  OK(tb_complex_query_predicate_close(&predicate));
}
static void hidden(tb_complex_query_binding_ref_v1_t binding) {
  tb_complex_query *next = NULL;
  OK(tb_complex_query_match(query, binding, &next, &diagnostics));
  OK(tb_complex_query_close(&query));
  query = next;
  if (page_query) {
    OK(tb_complex_query_match(page_query, binding, &next, &diagnostics));
    OK(tb_complex_query_close(&page_query));
    page_query = next;
  }
}
#define BIND(model, mode, var)                                                 \
  tb_complex_##model##_query_##mode##_binding *var = NULL;                     \
  OK(tb_complex_##model##_query_##mode##_binding_open(session, &var,           \
                                                      &diagnostics))
#define CLOSE_BIND(model, mode, var)                                           \
  OK(tb_complex_##model##_query_##mode##_binding_close(&var))
#define HIDDEN(model, mode, var)                                               \
  hidden(tb_complex_##model##_query_##mode##_binding_ref(var))
#define LINK(model, role_name, relation, player_model, player_mode, player)                  \
  do {                                                                                       \
    tb_complex_##model##_##role_name##_query_role *role = NULL;                              \
    tb_complex_query_predicate *predicate = NULL;                                            \
    OK(tb_complex_##model##_##role_name##_query_role_from_exact(                             \
        relation, &role, &diagnostics));                                                     \
    OK(tb_complex_##model##_##role_name##_query_role_links(                                  \
        role,                                                                                \
        tb_complex_##model##_##role_name##_query_role_player_##player_model##_##player_mode( \
            player),                                                                         \
        &predicate, &diagnostics));                                                          \
    OK(tb_complex_##model##_##role_name##_query_role_close(&role));                          \
    where_take(predicate);                                                                   \
  } while (0)
#define COMPARE(model, name, binding, mode, attr, input, op, output)           \
  do {                                                                         \
    tb_complex_##model##_##name##_query_field *field = NULL;                   \
    tb_complex_##attr *value = NULL;                                           \
    OK(tb_complex_##model##_##name##_query_field_from_##mode(binding, &field,  \
                                                             &diagnostics));   \
    OK(tb_complex_##attr##_open(package, input, &value, &diagnostics));        \
    OK(tb_complex_##model##_##name##_query_field_compare_value(                \
        field, op, value, &output, &diagnostics));                             \
    OK(tb_complex_##model##_##name##_query_field_close(&field));               \
    OK(tb_complex_##attr##_close(&value));                                     \
  } while (0)
#define FILTER(model, name, binding, mode, attr, input, op)                    \
  do {                                                                         \
    tb_complex_query_predicate *predicate = NULL;                              \
    COMPARE(model, name, binding, mode, attr, input, op, predicate);           \
    where_take(predicate);                                                     \
  } while (0)

static size_t row_count(const tb_complex_query_rows_result *result) {
  size_t count = 0;
  OK(tb_complex_query_result_row_count(tb_complex_query_rows_result_ref(result),
                                       &count, &diagnostics));
  return count;
}
static tb_complex_identifier *
person_identifier(tb_complex_person_query_subtypes_result *family) {
  tb_complex_identifier *identifier = NULL;
  tb_complex_person_query_subtypes_result_kind_t kind;
  OK(tb_complex_person_query_subtypes_result_kind(family, &kind, &diagnostics));
  if (kind == tb_complex_person_query_subtypes_result_kind_employee) {
    tb_complex_employee *person = NULL;
    OK(tb_complex_person_query_subtypes_result_as_employee(family, &person,
                                                           &diagnostics));
    OK(tb_complex_employee_identifier(person, &identifier, &diagnostics));
    OK(tb_complex_employee_close(&person));
  } else {
    tb_complex_person *person = NULL;
    assert(kind == tb_complex_person_query_subtypes_result_kind_person);
    OK(tb_complex_person_query_subtypes_result_as_person(family, &person,
                                                         &diagnostics));
    OK(tb_complex_person_identifier(person, &identifier, &diagnostics));
    OK(tb_complex_person_close(&person));
  }
  return identifier;
}

static void print_row(const tb_complex_query_rows_result *result, size_t row) {
  tb_complex_person_query_subtypes_result *family = NULL;
  tb_complex_employment *employment = NULL;
  tb_complex_identifier *identifier = NULL;
  tb_complex_note *note = NULL;
  OK(tb_complex_person_query_subtypes_one_at(
      tb_complex_person_query_subtypes_one_result_slot_v1_t_rows(result, 0),
      row, &family, &diagnostics));
  identifier = person_identifier(family);
  OK(tb_complex_employment_query_exact_one_at(
      tb_complex_employment_query_exact_one_result_slot_v1_t_rows(result, 1),
      row, &employment, &diagnostics));
  OK(tb_complex_employment_note(employment, &note, &diagnostics));
  type_bridge_byte_view_t a, b;
  OK(tb_complex_identifier_value(identifier, &a, &diagnostics));
  OK(tb_complex_note_value(note, &b, &diagnostics));
  printf("[\"%.*s\",\"%.*s\"]", (int)a.length, a.data, (int)b.length, b.data);
  OK(tb_complex_identifier_close(&identifier));
  OK(tb_complex_note_close(&note));
  OK(tb_complex_employment_close(&employment));
  OK(tb_complex_person_query_subtypes_result_close(&family));
}

static void incompatible_plan_inputs_fail_before_connect(void) {
  type_bridge_plan_builder_t *builder = NULL;
  type_bridge_plan_t *plan = NULL;
  uint32_t integer = 0, boolean = 0;
  OK(tb_complex_plan_open(NULL, &builder, &diagnostics));
  tb_complex_party_handle_t party;
  OK(tb_complex_party_subtypes(builder, &party));
  OK(type_bridge_plan_builder_input(builder, view("integer"),
                                    TYPE_BRIDGE_PROJECTED_VALUE_LONG, 0u,
                                    &integer));
  OK(type_bridge_plan_builder_input(builder, view("boolean"),
                                    TYPE_BRIDGE_PROJECTED_VALUE_BOOLEAN, 0u,
                                    &boolean));
  OK(type_bridge_plan_builder_compare(builder, TYPE_BRIDGE_PLAN_OPERAND_INPUT,
                                      integer, TYPE_BRIDGE_PLAN_COMPARE_GREATER,
                                      TYPE_BRIDGE_PLAN_OPERAND_INPUT, boolean));
  OK(type_bridge_plan_builder_count(builder));
  assert(type_bridge_plan_builder_build(builder, package, &plan,
                                        &diagnostics) != TYPE_BRIDGE_STATUS_OK);
  assert(plan == NULL);
  type_bridge_execution_diagnostic_view_v1_t error;
  OK(type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &error));
  const char *expected = "query_plan_value_type_mismatch";
  if (error.code.length != strlen(expected) ||
      memcmp(error.code.data, expected, error.code.length) != 0)
    fprintf(stderr, "wrong refusal: %.*s\n", (int)error.code.length,
            error.code.data);
  assert(error.code.length == strlen(expected) &&
         memcmp(error.code.data, expected, error.code.length) == 0);
  OK(type_bridge_execution_diagnostics_close(&diagnostics));
  type_bridge_plan_builder_free(&builder);
}

static void exact_abstract_plan_is_refused_before_connect(void) {
  type_bridge_plan_builder_t *builder = NULL;
  type_bridge_plan_t *plan = NULL;
  uint32_t party_type = 0, party_binding = 0;
  OK(tb_complex_plan_open(NULL, &builder, &diagnostics));
  OK(type_bridge_plan_builder_type(builder, TYPE_BRIDGE_PLAN_THING_ENTITY,
                                   view("party"), &party_type));
  OK(type_bridge_plan_builder_exact(builder, party_type, &party_binding));
  OK(type_bridge_plan_builder_count(builder));
  assert(type_bridge_plan_builder_build(builder, package, &plan,
                                        &diagnostics) != TYPE_BRIDGE_STATUS_OK);
  assert(plan == NULL);
  type_bridge_execution_diagnostic_view_v1_t error;
  OK(type_bridge_execution_diagnostics_get_v1(diagnostics, 0, &error));
  const char *code = "query_plan_abstract_exact_read";
  assert(error.code.length == strlen(code) &&
         memcmp(error.code.data, code, error.code.length) == 0);
  OK(type_bridge_execution_diagnostics_close(&diagnostics));
  type_bridge_plan_builder_free(&builder);
}
static uint64_t abstract_subtypes_plan_count(void) {
  type_bridge_plan_builder_t *builder = NULL;
  type_bridge_plan_t *plan = NULL;
  tb_complex_party_handle_t party;
  OK(tb_complex_plan_open(NULL, &builder, &diagnostics));
  OK(tb_complex_party_subtypes(builder, &party));
  OK(type_bridge_plan_builder_count(builder));
  OK(type_bridge_plan_builder_build(builder, package, &plan, &diagnostics));
  type_bridge_staged_transaction_t *transaction = NULL;
  type_bridge_pending_t *pending = NULL;
  OK(type_bridge_database_transaction(db, TYPE_BRIDGE_TRANSACTION_READ,
                                      &transaction, &diagnostics));
  OK(type_bridge_staged_transaction_stage(transaction, plan, &pending,
                                          &diagnostics));
  OK(type_bridge_staged_transaction_commit(transaction, &diagnostics));
  type_bridge_plan_answer_kind_t kind;
  uint64_t count = 0;
  OK(type_bridge_pending_answer(pending, &kind, &count));
  assert(kind == TYPE_BRIDGE_PLAN_ANSWER_COUNT);
  type_bridge_pending_free(&pending);
  type_bridge_staged_transaction_close(&transaction);
  type_bridge_plan_free(&plan);
  type_bridge_plan_builder_free(&builder);
  return count;
}

static void abstract_query_terminals(
    tb_complex_query *q, tb_complex_query_root_v1_t root,
    const tb_complex_query_order *order,
    tb_complex_query_field_group_ref_v1_t field,
    uint64_t expected) {
  const tb_complex_query_order *sort[] = {order};
  tb_complex_query_rows_terminal *rows_terminal = NULL;
  tb_complex_query_rows_result *rows = NULL;
  OK(tb_complex_query_rows(q, sort, 1, 0, expected + 1, &rows_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_rows(db, rows_terminal, NULL, NULL, &rows, &diagnostics));
  assert(row_count(rows) == expected);
  OK(tb_complex_query_rows_result_close(&rows));
  OK(tb_complex_query_rows_terminal_close(&rows_terminal));
  tb_complex_query_first_terminal *first_terminal = NULL;
  OK(tb_complex_query_first(q, sort, 1, &first_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_first(db, first_terminal, NULL, NULL, &rows, &diagnostics));
  assert(row_count(rows) == 1);
  OK(tb_complex_query_rows_result_close(&rows));
  OK(tb_complex_query_first_terminal_close(&first_terminal));
  tb_complex_query_count_terminal *count_terminal = NULL;
  tb_complex_query_count_result *count_result = NULL;
  uint64_t count = 0;
  OK(tb_complex_query_count_by(root, &count_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_count(db, count_terminal, NULL, NULL, &count_result, &diagnostics));
  OK(tb_complex_query_count_result_value(count_result, &count, &diagnostics));
  assert(count == expected);
  OK(tb_complex_query_count_result_close(&count_result));
  OK(tb_complex_query_count_terminal_close(&count_terminal));
  tb_complex_query_exists_terminal *exists_terminal = NULL;
  tb_complex_query_exists_result *exists_result = NULL;
  uint8_t exists = 0;
  OK(tb_complex_query_exists_by(root, &exists_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_exists(db, exists_terminal, NULL, NULL, &exists_result, &diagnostics));
  OK(tb_complex_query_exists_result_value(exists_result, &exists, &diagnostics));
  assert(exists);
  OK(tb_complex_query_exists_result_close(&exists_result));
  OK(tb_complex_query_exists_terminal_close(&exists_terminal));
  tb_complex_query_page_terminal *page_terminal = NULL;
  tb_complex_query_page_result *page = NULL;
  OK(tb_complex_query_page_by(root, sort, 1, 0, expected + 1, 1, &page_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_page(db, page_terminal, NULL, NULL, &page, &diagnostics));
  size_t page_count = 0;
  type_bridge_query_page_metadata_v1_t metadata;
  OK(tb_complex_query_result_row_count(tb_complex_query_page_result_ref(page), &page_count, &diagnostics));
  OK(tb_complex_query_page_metadata(page, &metadata, &diagnostics));
  assert(page_count == expected && metadata.has_total && metadata.total == expected);
  OK(tb_complex_query_page_result_close(&page));
  OK(tb_complex_query_page_terminal_close(&page_terminal));
  tb_complex_query_reducer_ref_v1_t reducer = tb_complex_query_reducer_count();
  tb_complex_query_field_reduction_terminal *field_terminal = NULL;
  tb_complex_query_field_reduction_result *field_result = NULL;
  OK(tb_complex_query_reduce_field(root, field, &reducer, 1, &field_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_field_reduction(db, field_terminal, NULL, NULL, &field_result, &diagnostics));
  size_t groups = 0;
  OK(tb_complex_query_reduction_result_row_count(tb_complex_query_field_reduction_result_ref(field_result), &groups, &diagnostics));
  assert(groups == expected);
  OK(tb_complex_query_field_reduction_result_close(&field_result));
  OK(tb_complex_query_field_reduction_terminal_close(&field_terminal));

}

static void abstract_model_groups(tb_complex_query_root_v1_t root,
    tb_complex_query_binding_group_ref_v1_t model, uint64_t expected) {
  tb_complex_query_reducer_ref_v1_t reducer = tb_complex_query_reducer_count();
  tb_complex_query_reduction_terminal *model_terminal = NULL;
  tb_complex_query_reduction_result *model_result = NULL;
  OK(tb_complex_query_reduce_grouped(root, model, &reducer, 1, &model_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_reduction(db, model_terminal, NULL, NULL, &model_result, &diagnostics));
  size_t groups = 0;
  OK(tb_complex_query_reduction_result_row_count(tb_complex_query_reduction_result_ref(model_result), &groups, &diagnostics));
  assert(groups == expected);
  OK(tb_complex_query_reduction_result_close(&model_result));
  OK(tb_complex_query_reduction_terminal_close(&model_terminal));
}

#define ABSTRACT_READ_CASE(MODEL, MODE, EDGE, ROLE, PREFIX) \
static void abstract_##MODEL##_##MODE(tb_complex_query_session *session, uint64_t size) { \
  BIND(MODEL, MODE, root); \
  BIND(EDGE, exact, edge); \
  tb_complex_##MODEL##_query_##MODE##_one_selection *selection = NULL; \
  tb_complex_##EDGE##_query_exact_one_selection *edge_selection = NULL; \
  tb_complex_##MODEL##_identifier_query_field *field = NULL; \
  tb_complex_query_order *order = NULL; \
  OK(tb_complex_##MODEL##_query_##MODE##_one_selection_open(root, &selection, &diagnostics)); \
  OK(tb_complex_##EDGE##_query_exact_one_selection_open(edge, &edge_selection, &diagnostics)); \
  OK(tb_complex_##MODEL##_identifier_query_field_from_##MODE(root, &field, &diagnostics)); \
  OK(tb_complex_##MODEL##_identifier_query_field_order(field, TYPE_BRIDGE_QUERY_SORT_ASCENDING, &order, &diagnostics)); \
  tb_complex_query *q = NULL, *next = NULL; \
  OK(tb_complex_query_positional_1(session, tb_complex_##MODEL##_query_##MODE##_one_selection_ref(selection), &q, &diagnostics)); \
  abstract_query_terminals(q, tb_complex_##MODEL##_query_##MODE##_binding_root(q, root), order, \
      tb_complex_##MODEL##_identifier_query_field_reduction_group_slot_v1_t_from(field), \
      size); \
  tb_complex_identifier *identifier = NULL; \
  tb_complex_query_predicate *identity = NULL; \
  OK(tb_complex_identifier_open(package, view(PREFIX "0001"), &identifier, &diagnostics)); \
  OK(tb_complex_##MODEL##_identifier_query_field_compare_value(field, TYPE_BRIDGE_QUERY_COMPARE_EQUAL, identifier, &identity, &diagnostics)); \
  OK(tb_complex_query_where(q, identity, &next, &diagnostics)); \
  tb_complex_query_rows_terminal *one_terminal = NULL; \
  tb_complex_query_rows_result *rows = NULL; \
  OK(tb_complex_query_one(next, NULL, 0, &one_terminal, &diagnostics)); \
  OK(tb_complex_database_query_execute_rows(db, one_terminal, NULL, NULL, &rows, &diagnostics)); \
  assert(row_count(rows) == 1); \
  OK(tb_complex_query_rows_result_close(&rows)); \
  OK(tb_complex_query_rows_terminal_close(&one_terminal)); \
  OK(tb_complex_query_close(&next)); \
  OK(tb_complex_query_predicate_close(&identity)); \
  OK(tb_complex_identifier_close(&identifier)); \
  tb_complex_##EDGE##_##ROLE##_query_role *role = NULL; \
  tb_complex_query_predicate *link = NULL; \
  OK(tb_complex_##EDGE##_##ROLE##_query_role_from_exact(edge, &role, &diagnostics)); \
  OK(tb_complex_##EDGE##_##ROLE##_query_role_links(role, tb_complex_##EDGE##_##ROLE##_query_role_player_##MODEL##_##MODE(root), &link, &diagnostics)); \
  OK(tb_complex_query_match(q, tb_complex_##EDGE##_query_exact_binding_ref(edge), &next, &diagnostics)); \
  OK(tb_complex_query_close(&q)); q = next; next = NULL; \
  OK(tb_complex_query_where(q, link, &next, &diagnostics)); \
  OK(tb_complex_query_close(&q)); q = next; next = NULL; \
  abstract_query_terminals(q, tb_complex_##MODEL##_query_##MODE##_binding_root(q, root), order, \
      tb_complex_##MODEL##_identifier_query_field_reduction_group_slot_v1_t_from(field), \
      size); \
  OK(tb_complex_query_close(&q)); \
  /* Selecting the relation hydrates its nested abstract role player. */ \
  OK(tb_complex_query_positional_1(session, tb_complex_##EDGE##_query_exact_one_selection_ref(edge_selection), &q, &diagnostics)); \
  OK(tb_complex_query_match(q, tb_complex_##MODEL##_query_##MODE##_binding_ref(root), &next, &diagnostics)); \
  OK(tb_complex_query_close(&q)); q = next; next = NULL; \
  OK(tb_complex_query_where(q, link, &next, &diagnostics)); \
  OK(tb_complex_query_close(&q)); q = next; next = NULL; \
  abstract_model_groups(tb_complex_##EDGE##_query_exact_binding_root(q, edge), tb_complex_##MODEL##_query_##MODE##_reduction_group_slot_v1_t_from(root), size); \
  tb_complex_query_rows_terminal *terminal = NULL; \
  OK(tb_complex_query_rows(q, NULL, 0, 0, size + 1, &terminal, &diagnostics)); \
  OK(tb_complex_database_query_execute_rows(db, terminal, NULL, NULL, &rows, &diagnostics)); \
  assert(row_count(rows) == size); \
  OK(tb_complex_query_rows_result_close(&rows)); \
  OK(tb_complex_query_rows_terminal_close(&terminal)); \
  OK(tb_complex_query_close(&q)); \
  tb_complex_##MODEL##_query_##MODE##_collect_selection *collected = NULL; \
  OK(tb_complex_##MODEL##_query_##MODE##_collect_selection_open(root, 1, NULL, 0, &collected, &diagnostics)); \
  OK(tb_complex_query_positional_2(session, tb_complex_##EDGE##_query_exact_one_selection_ref(edge_selection), tb_complex_##MODEL##_query_##MODE##_collect_selection_ref(collected), &q, &diagnostics)); \
  OK(tb_complex_query_where(q, link, &next, &diagnostics)); \
  OK(tb_complex_query_close(&q)); q = next; next = NULL; \
  tb_complex_query_page_terminal *page_terminal = NULL; \
  tb_complex_query_page_result *page = NULL; \
  OK(tb_complex_query_page_by(tb_complex_##EDGE##_query_exact_binding_root(q, edge), NULL, 0, 0, size + 1, 1, &page_terminal, &diagnostics)); \
  OK(tb_complex_database_query_execute_page(db, page_terminal, NULL, NULL, &page, &diagnostics)); \
  size_t count = 0; \
  OK(tb_complex_query_result_row_count(tb_complex_query_page_result_ref(page), &count, &diagnostics)); \
  assert(count == size); \
  for (size_t i = 0; i < count; ++i) { \
    size_t members = 0; \
    OK(tb_complex_##MODEL##_query_##MODE##_collect_count(tb_complex_##MODEL##_query_##MODE##_collect_result_slot_v1_t_page(page, 1), i, &members, &diagnostics)); \
    assert(members == 1); \
  } \
  OK(tb_complex_query_page_result_close(&page)); \
  OK(tb_complex_query_page_terminal_close(&page_terminal)); \
  OK(tb_complex_query_close(&q)); \
  OK(tb_complex_##MODEL##_query_##MODE##_collect_selection_close(&collected)); \
  OK(tb_complex_query_predicate_close(&link)); \
  OK(tb_complex_##EDGE##_##ROLE##_query_role_close(&role)); \
  OK(tb_complex_##MODEL##_identifier_query_field_close(&field)); \
  OK(tb_complex_query_order_close(&order)); \
  OK(tb_complex_##MODEL##_query_##MODE##_one_selection_close(&selection)); \
  OK(tb_complex_##EDGE##_query_exact_one_selection_close(&edge_selection)); \
  CLOSE_BIND(MODEL, MODE, root); CLOSE_BIND(EDGE, exact, edge); \
}
ABSTRACT_READ_CASE(party, exact, friendship, participant, "P")
ABSTRACT_READ_CASE(party, subtypes, friendship, participant, "P")
ABSTRACT_READ_CASE(association, exact, relay, edge, "F")
ABSTRACT_READ_CASE(association, subtypes, relay, edge, "F")

static void abstract_read_matrix(tb_complex_query_session *session, uint64_t size) {
  void (*cases[])(tb_complex_query_session *, uint64_t) = {
      abstract_party_exact, abstract_party_subtypes,
      abstract_association_exact, abstract_association_subtypes};
  for (size_t i = 0; i < sizeof(cases) / sizeof(cases[0]); ++i)
    cases[i](session, size);
}


int main(void) {
  const char *width_env = getenv("TYPE_BRIDGE_COMPLEX_WIDTH");
  unsigned width = width_env ? (unsigned)strtoul(width_env, NULL, 10) : 12u;
  uint64_t size = strtoull(getenv("TYPE_BRIDGE_COMPLEX_SIZE"), NULL, 10);
  uint64_t limit = size + 1;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_runtime_config_v1_t runtime_config = {0};
  runtime_config.struct_size = sizeof(runtime_config);
  runtime_config.version = TYPE_BRIDGE_RUNTIME_CONFIG_VERSION;
  runtime_config.worker_threads = 2;
  OK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics));
  OK(tb_complex_schema_package_open(&package, &diagnostics));
  incompatible_plan_inputs_fail_before_connect();
  exact_abstract_plan_is_refused_before_connect();
  type_bridge_database_config_t config = {0};
  config.struct_size = sizeof(config);
  config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  config.address = view(getenv("TYPEDB_ADDRESS"));
  config.database = view(getenv("TYPE_BRIDGE_COMPLEX_DATABASE"));
  config.username = view("admin");
  config.password = view("password");
  config.http_port = (uint32_t)strtoul(getenv("TYPEDB_HTTP_PORT"), NULL, 10);
  OK(type_bridge_database_open(runtime, package, &config, NULL, &db,
                               &diagnostics));
  const char *raw = getenv("TYPE_BRIDGE_COMPLEX_RAW_QUERY");
  if (raw) {
    type_bridge_raw_result_t *answer = NULL;
    type_bridge_diagnostics_t *raw_diagnostics = NULL;
    type_bridge_status_t status = type_bridge_database_execute_query(
        db, view(raw), TYPE_BRIDGE_TRANSACTION_READ, &answer, &raw_diagnostics);
    type_bridge_byte_view_t json;
    if (status != TYPE_BRIDGE_STATUS_OK) {
      if (raw_diagnostics &&
          type_bridge_diagnostics_json(raw_diagnostics, &json) ==
              TYPE_BRIDGE_STATUS_OK)
        fprintf(stderr, "%.*s\n", (int)json.length, json.data);
      return 1;
    }
    OK(type_bridge_raw_result_json(answer, &json));
    printf("%.*s\n", (int)json.length, json.data);
    OK(type_bridge_raw_result_close(&answer));
    OK(type_bridge_diagnostics_close(&raw_diagnostics));
    OK(type_bridge_database_close(&db, &diagnostics));
    OK(type_bridge_schema_package_close(&package));
    OK(type_bridge_runtime_close(&runtime, &diagnostics));
    return 0;
  }
  uint64_t generic_abstract_count = abstract_subtypes_plan_count();
  tb_complex_query_session *session = NULL;
  OK(tb_complex_query_session_open(package, &session, &diagnostics));
  if (!width_env) abstract_read_matrix(session, size);
  BIND(person, subtypes, p);
  BIND(company, exact, c);
  BIND(department, exact, d);
  BIND(project, exact, j);
  BIND(task, exact, t);
  BIND(office, exact, o);
  BIND(employment, exact, e);
  BIND(membership, exact, m);
  BIND(assignment, exact, a);
  BIND(projectzhtask, exact, x);
  BIND(located, exact, l);
  BIND(companyzhoffice, exact, z);
  tb_complex_person_query_subtypes_one_selection *ps = NULL;
  tb_complex_employment_query_exact_one_selection *es = NULL;
  OK(tb_complex_person_query_subtypes_one_selection_open(p, &ps, &diagnostics));
  OK(tb_complex_employment_query_exact_one_selection_open(e, &es,
                                                          &diagnostics));
  OK(tb_complex_query_positional_2(
      session, tb_complex_person_query_subtypes_one_selection_ref(ps),
      tb_complex_employment_query_exact_one_selection_ref(es), &query,
      &diagnostics));
  OK(tb_complex_query_positional_1(
      session, tb_complex_person_query_subtypes_one_selection_ref(ps),
      &page_query, &diagnostics));
  tb_complex_query *page_with_e = NULL;
  OK(tb_complex_query_match(page_query,
                            tb_complex_employment_query_exact_binding_ref(e),
                            &page_with_e, &diagnostics));
  OK(tb_complex_query_close(&page_query));
  page_query = page_with_e;
  OK(tb_complex_person_query_subtypes_one_selection_close(&ps));
  OK(tb_complex_employment_query_exact_one_selection_close(&es));
  if (width >= 4) {
    HIDDEN(company, exact, c);
  }
  if (width >= 6) {
    HIDDEN(department, exact, d);
  }
  if (width >= 10) {
    HIDDEN(project, exact, j);
  }
  if (width >= 12) {
    HIDDEN(task, exact, t);
  }
  if (width >= 8) {
    HIDDEN(office, exact, o);
  }
  if (width >= 4) {
    HIDDEN(membership, exact, m);
  }
  if (width >= 10) {
    HIDDEN(assignment, exact, a);
  }
  if (width >= 12) {
    HIDDEN(projectzhtask, exact, x);
  }
  if (width >= 6) {
    HIDDEN(located, exact, l);
  }
  if (width >= 8) {
    HIDDEN(companyzhoffice, exact, z);
  }
  LINK(employment, worker, e, person, subtypes, p);
  if (width >= 4) {
    LINK(employment, employer, e, company, exact, c);
  }
  if (width >= 4) {
    LINK(membership, member, m, person, subtypes, p);
  }
  if (width >= 6) {
    LINK(membership, department, m, department, exact, d);
  }
  if (width >= 10) {
    LINK(assignment, worker, a, person, subtypes, p);
  }
  if (width >= 10) {
    LINK(assignment, project, a, project, exact, j);
  }
  if (width >= 12) {
    LINK(projectzhtask, project, x, project, exact, j);
  }
  if (width >= 12) {
    LINK(projectzhtask, task, x, task, exact, t);
  }
  if (width >= 6) {
    LINK(located, department, l, department, exact, d);
  }
  if (width >= 8) {
    LINK(located, office, l, office, exact, o);
  }
  if (width >= 8) {
    LINK(companyzhoffice, company, z, company, exact, c);
  }
  if (width >= 8) {
    LINK(companyzhoffice, office, z, office, exact, o);
  }
  FILTER(person, tag, p, subtypes, tag, view("a"),
         TYPE_BRIDGE_QUERY_COMPARE_EQUAL);
  FILTER(person, tag, p, subtypes, tag, view("z"),
         TYPE_BRIDGE_QUERY_COMPARE_EQUAL);
  FILTER(person, score, p, subtypes, score, 20,
         TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL);
  if (width >= 4) {
    FILTER(company, identifier, c, exact, identifier, view("C"),
           TYPE_BRIDGE_QUERY_COMPARE_STARTS_WITH);
  }
  if (width >= 10) {
    FILTER(project, priority, j, exact, priority, 1,
           TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL);
  }
  tb_complex_query_predicate *done = NULL, *site = NULL, *either = NULL,
                             *blocked = NULL, *unblocked = NULL;
  if (width >= 12) {
    COMPARE(task, done, t, exact, done, 0u, TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
            done);
    COMPARE(task, identifier, t, exact, identifier, view("T0008"),
            TYPE_BRIDGE_QUERY_COMPARE_STARTS_WITH, site);
    OK(tb_complex_query_predicate_or(done, site, &either, &diagnostics));
    OK(tb_complex_query_predicate_close(&done));
    OK(tb_complex_query_predicate_close(&site));
    where_take(either);
  }
  if (width >= 8) {
    FILTER(office, site, o, exact, site, view("north"),
           TYPE_BRIDGE_QUERY_COMPARE_CONTAINS);
  }
  if (width >= 6) {
    COMPARE(department, blocked, d, exact, blocked, 1u,
            TYPE_BRIDGE_QUERY_COMPARE_EQUAL, blocked);
    OK(tb_complex_query_predicate_not(blocked, &unblocked, &diagnostics));
    OK(tb_complex_query_predicate_close(&blocked));
    where_take(unblocked);
  }
  if (width < 12) {
    FILTER(employment, note, e, exact, note, view("job-"),
           TYPE_BRIDGE_QUERY_COMPARE_STARTS_WITH);
    tb_complex_query_predicate *left = NULL, *right = NULL, *either_id = NULL;
    COMPARE(person, identifier, p, subtypes, identifier, view("P0002"),
            TYPE_BRIDGE_QUERY_COMPARE_EQUAL, left);
    COMPARE(person, identifier, p, subtypes, identifier, view("P0008"),
            TYPE_BRIDGE_QUERY_COMPARE_EQUAL, right);
    OK(tb_complex_query_predicate_or(left, right, &either_id, &diagnostics));
    OK(tb_complex_query_predicate_close(&left));
    OK(tb_complex_query_predicate_close(&right));
    tb_complex_query_predicate *third = NULL, *all_ids = NULL;
    COMPARE(person, identifier, p, subtypes, identifier, view("P0004"),
            TYPE_BRIDGE_QUERY_COMPARE_EQUAL, third);
    OK(tb_complex_query_predicate_or(either_id, third, &all_ids, &diagnostics));
    OK(tb_complex_query_predicate_close(&either_id));
    OK(tb_complex_query_predicate_close(&third));
    where_take(all_ids);
  }
  if (width >= 10) {
    tb_complex_person_score_query_field *score = NULL;
    tb_complex_project_priority_query_field *priority = NULL;
    tb_complex_query_predicate *comparison = NULL;
    OK(tb_complex_person_score_query_field_from_subtypes(p, &score,
                                                         &diagnostics));
    OK(tb_complex_project_priority_query_field_from_exact(j, &priority,
                                                          &diagnostics));
    OK(tb_complex_query_integer_field_compare_field(
        tb_complex_person_score_query_field_integer_field_ref(score),
        TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL,
        tb_complex_project_priority_query_field_integer_field_ref(priority),
        &comparison, &diagnostics));
    OK(tb_complex_person_score_query_field_close(&score));
    OK(tb_complex_project_priority_query_field_close(&priority));
    where_take(comparison);
  }
  tb_complex_employment_note_query_field *note_field = NULL;
  tb_complex_person_identifier_query_field *id_field = NULL;
  tb_complex_query_predicate *present = NULL;
  tb_complex_query_order *id_order = NULL, *note_order = NULL;
  OK(tb_complex_employment_note_query_field_from_exact(e, &note_field,
                                                       &diagnostics));
  OK(tb_complex_employment_note_query_field_presence(note_field, 1u, &present,
                                                     &diagnostics));
  where_take(present);
  OK(tb_complex_employment_note_query_field_order(
      note_field, TYPE_BRIDGE_QUERY_SORT_ASCENDING, &note_order, &diagnostics));
  OK(tb_complex_person_identifier_query_field_from_subtypes(p, &id_field,
                                                            &diagnostics));
  OK(tb_complex_person_identifier_query_field_order(
      id_field, TYPE_BRIDGE_QUERY_SORT_ASCENDING, &id_order, &diagnostics));
  const tb_complex_query_order *sorts[] = {id_order, note_order};
  tb_complex_query_rows_terminal *terminal = NULL;
  tb_complex_query_rows_result *result = NULL, *first = NULL;
  tb_complex_query_first_terminal *first_terminal = NULL;
  OK(tb_complex_query_rows(query, sorts, 2, 0, limit, &terminal, &diagnostics));
  OK(tb_complex_database_query_execute_rows(db, terminal, NULL, NULL, &result,
                                            &diagnostics));
  OK(tb_complex_query_first(query, sorts, 2, &first_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_first(db, first_terminal, NULL, NULL,
                                             &first, &diagnostics));
  tb_complex_query_count_terminal *count_terminal = NULL;
  tb_complex_query_exists_terminal *exists_terminal = NULL;
  tb_complex_query_count_result *count_result = NULL;
  tb_complex_query_exists_result *exists_result = NULL;
  tb_complex_query_root_v1_t root =
      tb_complex_person_query_subtypes_binding_root(query, p);
  OK(tb_complex_query_count_by(root, &count_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_count(db, count_terminal, NULL, NULL,
                                             &count_result, &diagnostics));
  OK(tb_complex_query_exists_by(root, &exists_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_exists(db, exists_terminal, NULL, NULL,
                                              &exists_result, &diagnostics));
  uint64_t count = 0;
  uint8_t exists = 0;
  OK(tb_complex_query_count_result_value(count_result, &count, &diagnostics));
  OK(tb_complex_query_exists_result_value(exists_result, &exists,
                                          &diagnostics));
  tb_complex_query_page_terminal *page_terminal = NULL;
  tb_complex_query_page_result *page = NULL;
  OK(tb_complex_query_page_by(
      tb_complex_person_query_subtypes_binding_root(page_query, p), sorts, 1, 1,
      1, 1u, &page_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_page(db, page_terminal, NULL, NULL,
                                            &page, &diagnostics));
  size_t page_count = 0;
  OK(tb_complex_query_result_row_count(tb_complex_query_page_result_ref(page),
                                       &page_count, &diagnostics));
  type_bridge_query_page_metadata_v1_t page_metadata = {0};
  OK(tb_complex_query_page_metadata(page, &page_metadata, &diagnostics));
  assert(page_metadata.has_total && page_metadata.total == count);
  assert(page_count == (count > 1 ? 1u : 0u));
  tb_complex_query_rows_terminal *offset_terminal = NULL;
  tb_complex_query_rows_result *offset_result = NULL;
  OK(tb_complex_query_rows(query, sorts, 2, 1, 1, &offset_terminal,
                           &diagnostics));
  OK(tb_complex_database_query_execute_rows(db, offset_terminal, NULL, NULL,
                                            &offset_result, &diagnostics));
  assert(row_count(offset_result) == page_count);
  tb_complex_query_field_reduction_terminal *group_terminal = NULL;
  tb_complex_query_field_reduction_result *groups = NULL;
  tb_complex_query_reducer_ref_v1_t reducer = tb_complex_query_reducer_count();
  OK(tb_complex_query_reduce_field(
      tb_complex_person_query_subtypes_binding_root(page_query, p),
      tb_complex_person_identifier_query_field_reduction_group_slot_v1_t_from(
          id_field),
      &reducer, 1, &group_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_field_reduction(
      db, group_terminal, NULL, NULL, &groups, &diagnostics));
  tb_complex_query_reduction_result_ref_v1_t group_ref =
      tb_complex_query_field_reduction_result_ref(groups);
  size_t group_count = 0;
  OK(tb_complex_query_reduction_result_row_count(group_ref, &group_count,
                                                 &diagnostics));
  assert(group_count == count);
  printf("{\"rows\":[");
  for (size_t i = 0; i < row_count(result); ++i) {
    if (i)
      putchar(',');
    print_row(result, i);
  }
  printf("],\"first\":");
  if (row_count(first))
    print_row(first, 0);
  else
    printf("null");
  printf(",\"count\":%" PRIu64 ",\"exists\":%s", count,
         exists ? "true" : "false");

  printf(",\"page_id\":");
  if (page_count) {
    tb_complex_person_query_subtypes_result *family = NULL;
    OK(tb_complex_person_query_subtypes_one_at(
        tb_complex_person_query_subtypes_one_result_slot_v1_t_page(page, 0), 0,
        &family, &diagnostics));
    tb_complex_identifier *identifier = person_identifier(family);
    type_bridge_byte_view_t text;
    OK(tb_complex_identifier_value(identifier, &text, &diagnostics));
    printf("\"%.*s\"", (int)text.length, text.data);
    OK(tb_complex_identifier_close(&identifier));
    OK(tb_complex_person_query_subtypes_result_close(&family));
  } else
    printf("null");
  printf(",\"groups\":[");
  for (size_t i = 0; i < group_count; ++i) {
    tb_complex_identifier *identifier = NULL;
    uint64_t members = 0;
    OK(tb_complex_person_identifier_query_field_reduction_group_slot_v1_t_value(
        tb_complex_person_identifier_query_field_reduction_group_slot_v1_t_from_result(
            group_ref, 0),
        i, &identifier, &diagnostics));
    OK(tb_complex_query_reduced_count_value(
        tb_complex_query_reduced_count_slot(group_ref, 0), i, &members,
        &diagnostics));
    type_bridge_byte_view_t text;
    OK(tb_complex_identifier_value(identifier, &text, &diagnostics));
    if (i)
      putchar(',');
    printf("[\"%.*s\",%" PRIu64 "]", (int)text.length, text.data, members);
    OK(tb_complex_identifier_close(&identifier));
  }
  putchar(']');
  OK(tb_complex_query_field_reduction_result_close(&groups));
  OK(tb_complex_query_field_reduction_terminal_close(&group_terminal));
  OK(tb_complex_query_rows_result_close(&offset_result));
  OK(tb_complex_query_rows_terminal_close(&offset_terminal));
  OK(tb_complex_query_page_result_close(&page));
  OK(tb_complex_query_page_terminal_close(&page_terminal));

  /* Pin abstract exact-mode result witnesses and the canonical manager path. */
  BIND(party, exact, party);
  tb_complex_party_query_exact_one_selection *party_selection = NULL;
  tb_complex_query *abstract_query = NULL;
  tb_complex_query_rows_terminal *abstract_terminal = NULL;
  tb_complex_query_rows_result *abstract_result = NULL;
  OK(tb_complex_party_query_exact_one_selection_open(party, &party_selection,
                                                     &diagnostics));
  OK(tb_complex_query_positional_1(
      session, tb_complex_party_query_exact_one_selection_ref(party_selection),
      &abstract_query, &diagnostics));
  OK(tb_complex_query_rows(abstract_query, NULL, 0, 0, limit,
                           &abstract_terminal, &diagnostics));
  OK(tb_complex_database_query_execute_rows(db, abstract_terminal, NULL, NULL,
                                            &abstract_result, &diagnostics));
  size_t abstract_count = row_count(abstract_result);
  assert(generic_abstract_count == abstract_count);
  if (abstract_count) {
    tb_complex_party *value = NULL;
    OK(tb_complex_party_query_exact_one_at(
        tb_complex_party_query_exact_one_result_slot_v1_t_rows(abstract_result,
                                                               0),
        0, &value, &diagnostics));
    OK(tb_complex_party_close(&value));
  }
  type_bridge_manager_filter_t *filter = NULL, *next_filter = NULL;
  const type_bridge_projected_token_v1_t *party_token =
      tb_complex_party_query_exact_binding_ref(party).expected_model;
  OK(type_bridge_manager_filter_open(package, party_token, &filter,
                                     &diagnostics));
  tb_complex_party_tag_query_field *tag_field = NULL;
  OK(tb_complex_party_tag_query_field_from_exact(party, &tag_field,
                                                 &diagnostics));
  const char *tags[] = {"a", "z"};
  for (size_t i = 0; i < 2; ++i) {
    tb_complex_tag *value = NULL;
    OK(tb_complex_tag_open(package, view(tags[i]), &value, &diagnostics));
    OK(type_bridge_manager_filter_compare(
        filter, tb_complex_party_tag_query_field_ref(tag_field).expected_field,
        TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
        (const type_bridge_projected_value_t *)value, &next_filter,
        &diagnostics));
    OK(type_bridge_manager_filter_close(&filter));
    filter = next_filter;
    OK(tb_complex_tag_close(&value));
  }
  uint64_t tagged_count = 0;
  size_t tagged_rows = 0;
  type_bridge_manager_result_t *tagged = NULL;
  OK(type_bridge_database_manager_all(db, filter, NULL, NULL, &tagged,
                                      &diagnostics));
  OK(type_bridge_manager_result_count(tagged, party_token, &tagged_rows,
                                      &diagnostics));
  OK(type_bridge_database_manager_count(db, filter, NULL, NULL, &tagged_count,
                                        &diagnostics));
  assert(tagged_rows == tagged_count);
  uint8_t tagged_exists = 0;
  OK(type_bridge_database_manager_exists(db, filter, NULL, NULL, &tagged_exists,
                                         &diagnostics));
  assert((tagged_exists != 0u) == (tagged_count != 0u));
  tb_complex_party_identifier_query_field *key_field = NULL;
  tb_complex_identifier *key = NULL;
  OK(tb_complex_party_identifier_query_field_from_exact(party, &key_field,
                                                        &diagnostics));
  OK(tb_complex_identifier_open(package, view("P0002"), &key, &diagnostics));
  OK(type_bridge_manager_filter_compare(
      filter,
      tb_complex_party_identifier_query_field_ref(key_field).expected_field,
      TYPE_BRIDGE_QUERY_COMPARE_EQUAL,
      (const type_bridge_projected_value_t *)key, &next_filter, &diagnostics));
  type_bridge_projected_thing_t *first_tagged = NULL;
  OK(type_bridge_database_manager_first(db, next_filter, NULL, NULL,
                                        &first_tagged, &diagnostics));
  assert((first_tagged != NULL) == (size >= 2u));
  OK(type_bridge_projected_thing_close(&first_tagged));
  OK(type_bridge_manager_filter_close(&next_filter));
  OK(tb_complex_identifier_close(&key));
  OK(tb_complex_party_identifier_query_field_close(&key_field));

  printf(",\"abstract_count\":%zu,\"tagged_count\":%" PRIu64 "}\n",
         abstract_count, tagged_count);
  OK(type_bridge_manager_result_close(&tagged));
  OK(type_bridge_manager_filter_close(&filter));
  OK(tb_complex_party_tag_query_field_close(&tag_field));
  OK(tb_complex_query_rows_result_close(&abstract_result));
  OK(tb_complex_query_rows_terminal_close(&abstract_terminal));
  OK(tb_complex_query_close(&abstract_query));
  OK(tb_complex_party_query_exact_one_selection_close(&party_selection));
  CLOSE_BIND(party, exact, party);
  OK(tb_complex_query_count_result_close(&count_result));
  OK(tb_complex_query_exists_result_close(&exists_result));
  OK(tb_complex_query_count_terminal_close(&count_terminal));
  OK(tb_complex_query_exists_terminal_close(&exists_terminal));
  OK(tb_complex_query_rows_result_close(&result));
  OK(tb_complex_query_rows_result_close(&first));
  OK(tb_complex_query_rows_terminal_close(&terminal));
  OK(tb_complex_query_first_terminal_close(&first_terminal));
  OK(tb_complex_employment_note_query_field_close(&note_field));
  OK(tb_complex_person_identifier_query_field_close(&id_field));
  OK(tb_complex_query_order_close(&id_order));
  OK(tb_complex_query_order_close(&note_order));
  OK(tb_complex_query_close(&query));
  OK(tb_complex_query_close(&page_query));
  CLOSE_BIND(person, subtypes, p);
  CLOSE_BIND(company, exact, c);
  CLOSE_BIND(department, exact, d);
  CLOSE_BIND(project, exact, j);
  CLOSE_BIND(task, exact, t);
  CLOSE_BIND(office, exact, o);
  CLOSE_BIND(employment, exact, e);
  CLOSE_BIND(membership, exact, m);
  CLOSE_BIND(assignment, exact, a);
  CLOSE_BIND(projectzhtask, exact, x);
  CLOSE_BIND(located, exact, l);
  CLOSE_BIND(companyzhoffice, exact, z);
  OK(tb_complex_query_session_close(&session));
  OK(type_bridge_database_close(&db, &diagnostics));
  OK(type_bridge_schema_package_close(&package));
  OK(type_bridge_runtime_close(&runtime, &diagnostics));
  return 0;
}
