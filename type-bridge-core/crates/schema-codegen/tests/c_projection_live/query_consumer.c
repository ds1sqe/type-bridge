#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <fixture/models.h>

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


#ifdef __cplusplus
extern "C" {
#endif
type_bridge_status_t TYPE_BRIDGE_CALL query_schema_package_open_flat(
    type_bridge_schema_package_t **out_package,
    type_bridge_execution_diagnostics_t **out_diagnostics);
#ifdef __cplusplus
}
#endif

#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "ABI 1.6 successor check failed at line %d\n",           \
              __LINE__);                                                       \
      return __LINE__;                                                         \
    }                                                                          \
  } while (0)

/* Reuse one caller-owned validated argument between synchronous calls. */
static const type_bridge_iid_t *checked_iid(type_bridge_byte_view_t text,
                                          type_bridge_iid_t **owner) {
  if (type_bridge_iid_close(owner) != TYPE_BRIDGE_STATUS_OK ||
      type_bridge_iid_open(text, owner) != TYPE_BRIDGE_STATUS_OK) {
    fputs("invalid test IID argument\n", stderr);
    abort();
  }
  return *owner;
}

static type_bridge_byte_view_t view_of(const char *text) {
  type_bridge_byte_view_t view;
  view.data = (const uint8_t *)text;
  view.length = strlen(text);
  return view;
}

static int same_text(type_bridge_byte_view_t value, const char *text) {
  const size_t length = strlen(text);
  return value.length == length && value.data != NULL &&
         memcmp(value.data, text, length) == 0;
}

static int check_exact_provider_write_failure(
    const type_bridge_execution_diagnostics_t *diagnostics) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic;
  type_bridge_execution_diagnostic_path_view_v1_t path;
  type_bridge_execution_diagnostic_detail_view_v1_t detail;
  size_t count = 0u;

  memset(&diagnostic, 0, sizeof(diagnostic));
  memset(&path, 0, sizeof(path));
  memset(&detail, 0, sizeof(detail));
  CHECK(diagnostics != NULL);
  CHECK(type_bridge_execution_diagnostics_count(diagnostics, &count) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);
  CHECK(type_bridge_execution_diagnostics_get_v1(
            diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostic.category == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PROVIDER);
  CHECK(same_text(diagnostic.code, "provider_operation_failed"));
  CHECK(same_text(
      diagnostic.message,
      "The database provider could not complete the requested operation"));
  CHECK(diagnostic.path_count == 1u && diagnostic.detail_count == 1u);
  CHECK(type_bridge_execution_diagnostics_path_get_v1(
            diagnostics, 0u, 0u, &path) == TYPE_BRIDGE_STATUS_OK);
  CHECK(path.kind == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_PATH_TYPE);
  CHECK(path.index == 0u && same_text(path.primary, "entity") &&
        same_text(path.secondary, "robot") && path.tertiary.length == 0u);
  CHECK(type_bridge_execution_diagnostics_detail_get_v1(
            diagnostics, 0u, 0u, &detail) == TYPE_BRIDGE_STATUS_OK);
  CHECK(detail.kind ==
        TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_DETAIL_PROVIDER_OPERATION);
  CHECK(same_text(detail.key, "operation") &&
        same_text(detail.primary, "write") && detail.secondary.length == 0u &&
        detail.tertiary.length == 0u && detail.quaternary.length == 0u);
  return 0;
}

static int check_exact_rollback_only(
    const type_bridge_execution_diagnostics_t *diagnostics) {
  type_bridge_execution_diagnostic_view_v1_t diagnostic;
  size_t count = 0u;

  memset(&diagnostic, 0, sizeof(diagnostic));
  CHECK(diagnostics != NULL);
  CHECK(type_bridge_execution_diagnostics_count(diagnostics, &count) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);
  CHECK(type_bridge_execution_diagnostics_get_v1(
            diagnostics, 0u, &diagnostic) == TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostic.category == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_TRANSACTION);
  CHECK(same_text(diagnostic.code, "transaction_rollback_only"));
  CHECK(same_text(diagnostic.message,
                  "The transaction is rollback-only and cannot be committed"));
  CHECK(diagnostic.path_count == 0u && diagnostic.detail_count == 0u);
  return 0;
}

static int copy_iid(type_bridge_byte_view_t iid, uint8_t *storage,
                    size_t capacity, size_t *out_length) {
  if (iid.data == NULL || iid.length == 0u || iid.length > capacity ||
      out_length == NULL) {
    return 0;
  }
  memcpy(storage, iid.data, iid.length);
  *out_length = iid.length;
  return 1;
}

static int
open_robot_create(const type_bridge_schema_package_t *package,
                  int64_t identifier_value, int64_t constrained_value,
                  fixture_robot_create **out_create,
                  type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_robotzuid *identifier = NULL;
  fixture_valzuconstrained *constrained = NULL;
  fixture_robot_create_args_v1_t args;

  memset(&args, 0, sizeof(args));
  CHECK(fixture_robotzuid_open(package, identifier_value, &identifier,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_open(package, constrained_value, &constrained,
                                      out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_robotzuid = identifier;
  args.field_valzuconstrained = constrained;
  CHECK(fixture_robot_create_open(package, &args, out_create,
                                  out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_close(&constrained) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robotzuid_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
open_counter_create(const type_bridge_schema_package_t *package, int64_t value,
                    fixture_counter_create **out_create,
                    type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_counterzuvalue *counter_value = NULL;
  fixture_counter_create_args_v1_t args;

  memset(&args, 0, sizeof(args));
  CHECK(fixture_counterzuvalue_open(package, value, &counter_value,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_counterzuvalue = counter_value;
  CHECK(fixture_counter_create_open(package, &args, out_create,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counterzuvalue_close(&counter_value) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
open_person_create(const type_bridge_schema_package_t *package, const char *key,
                   int nonempty, fixture_person_create **out_create,
                   type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_aliases *alias = NULL;
  const fixture_aliases *aliases[1];
  fixture_person_create_field_aliases_chunks_v1_t aliases_chunk;
  fixture_identifier *identifier = NULL;
  fixture_score *score = NULL;
  fixture_valzubool *boolean_value = NULL;
  fixture_valzuconstrained *constrained = NULL;
  fixture_valzudate *date = NULL;
  fixture_valzudatetime *datetime = NULL;
  fixture_valzudatetimezutzz *datetime_tz = NULL;
  fixture_valzudecimal *decimal = NULL;
  fixture_valzudouble *double_value = NULL;
  fixture_valzuduration *duration = NULL;
  fixture_person_create_args_v1_t args;

  memset(&args, 0, sizeof(args));
  memset(&aliases_chunk, 0, sizeof(aliases_chunk));
  CHECK(fixture_identifier_open(package, view_of(key), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_open(package, 30, &score, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzubool_open(package, 1u, &boolean_value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_open(package, 30, &constrained,
                                      out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudate_open(package, view_of("2026-08-14"), &date,
                               out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetime_open(package, view_of("2026-08-14T09:30:00"),
                                   &datetime,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetimezutzz_open(
            package, view_of("2026-08-14T09:30:00Z"), &datetime_tz,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudecimal_open(package, view_of("30.5"), &decimal,
                                  out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudouble_open(package, UINT64_C(0x403e800000000000),
                                 &double_value,
                                 out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuduration_open(package, view_of("PT30S"), &duration,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  if (nonempty) {
    CHECK(fixture_aliases_open(package, view_of("unsupported"), &alias, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    aliases[0] = alias;
    aliases_chunk.struct_size = sizeof(aliases_chunk);
    aliases_chunk.version = FIXTURE_CREATE_ARGS_VERSION;
    aliases_chunk.values = aliases;
    aliases_chunk.count = 1u;
    args.field_aliases_chunks = &aliases_chunk;
  }
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_identifier = identifier;
  args.field_score = score;
  args.field_valzubool = boolean_value;
  args.field_valzuconstrained = constrained;
  args.field_valzudate = date;
  args.field_valzudatetime = datetime;
  args.field_valzudatetimezutzz = datetime_tz;
  args.field_valzudecimal = decimal;
  args.field_valzudouble = double_value;
  args.field_valzuduration = duration;
  CHECK(fixture_person_create_open(package, &args, out_create,
                                   out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuduration_close(&duration) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudouble_close(&double_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudecimal_close(&decimal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetimezutzz_close(&datetime_tz) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudatetime_close(&datetime) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzudate_close(&date) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzuconstrained_close(&constrained) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_valzubool_close(&boolean_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_score_close(&score) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  if (alias != NULL) CHECK(fixture_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
open_membership_create(const type_bridge_schema_package_t *package,
                       type_bridge_byte_view_t iid,
                       fixture_membership_create **out_create,
                       type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_person_ref *reference = NULL;
  fixture_membership_member_player *member = NULL;
  fixture_membership_create_args_v1_t args;

  memset(&args, 0, sizeof(args));
  CHECK(fixture_person_ref_from_iid(package, iid, &reference,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_member_player_from_person(
            reference, &member, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.role_member = member;
  CHECK(fixture_membership_create_open(package, &args, out_create,
                                       out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_member_player_close(&member) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
open_interaction_create(const type_bridge_schema_package_t *package,
                        const char *key, type_bridge_byte_view_t target_iid,
                        fixture_interaction_create **out_create,
                        type_bridge_execution_diagnostics_t **out_diagnostics) {
  fixture_identifier *identifier = NULL;
  fixture_person_ref *reference = NULL;
  fixture_interaction_target_player *target = NULL;
  fixture_interaction_create_args_v1_t args;

  memset(&args, 0, sizeof(args));
  CHECK(fixture_identifier_open(package, view_of(key), &identifier,
                                out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_from_iid(package, target_iid, &reference,
                                    out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_target_player_from_person(
            reference, &target, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_identifier = identifier;
  args.role_target = target;
  CHECK(fixture_interaction_create_open(package, &args, out_create,
                                        out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_target_player_close(&target) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static void model_plan_mark(const char *name) {
#ifndef __cplusplus
  const char *path = getenv("TYPE_BRIDGE_TEST_PLAN_CAPTURE_LABEL");
  if (path != NULL) {
    FILE *file = fopen(path, "w");
    if (file == NULL) abort();
    if (*name != '\0' && strchr(name, '.') == NULL) fputs("unkeyed_entity_iid_lifecycle.", file);
    if (fputs(name, file) < 0 || fclose(file) != 0) abort();
  }
#else
  (void)name;
#endif
}

static void model_plan_identity(const char *name, type_bridge_byte_view_t iid) {
#ifndef __cplusplus
  const char *path = getenv("TYPE_BRIDGE_TEST_PLAN_CAPTURE_RECORDS");
  if (path != NULL) {
    FILE *file = fopen(path, "a");
    if (file == NULL || fprintf(file, "[\"%s\",\"%.*s\"]\n", name,
        (int)iid.length, (const char *)iid.data) < 0 || fclose(file) != 0) abort();
  }
#else
  (void)name;
  (void)iid;
#endif
}

static int run_staged_counter_count(
    const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database, uint64_t expected,
    type_bridge_execution_diagnostics_t **diagnostics) {
  fixture_query_session *session = NULL;
  fixture_counter_query_exact_binding *binding = NULL;
  fixture_counter_query_exact_one_selection *selection = NULL;
  fixture_query *query = NULL;
  fixture_query_count_terminal *terminal = NULL;
  type_bridge_staged_transaction_t *transaction = NULL;
  type_bridge_pending_t *pending = NULL;
  type_bridge_query_result_t *result = NULL;
  uint64_t count = 0u;
  CHECK(fixture_query_session_open(package, &session, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_query_exact_binding_open(session, &binding, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_query_exact_one_selection_open(binding, &selection, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_positional_1(session, fixture_counter_query_exact_one_selection_ref(selection), &query, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_count_by(fixture_counter_query_exact_binding_root(query, binding), &terminal, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_transaction(database, TYPE_BRIDGE_TRANSACTION_READ, &transaction, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_staged_query_stage(transaction, (const type_bridge_query_terminal_t *)terminal,
      TYPE_BRIDGE_QUERY_TERMINAL_COUNT, NULL, NULL, &pending, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_pending_query_result(pending, &result, diagnostics) == TYPE_BRIDGE_STATUS_IN_USE);
  CHECK(result == NULL && *diagnostics == NULL);
  CHECK(fixture_query_count_terminal_close(&terminal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_close(&query) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_query_exact_one_selection_close(&selection) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_query_exact_binding_close(&binding) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_query_session_close(&session) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("borrowed_transaction_lifecycle.staged_count");
  CHECK(type_bridge_staged_transaction_commit(transaction, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  type_bridge_staged_transaction_close(&transaction);
  CHECK(type_bridge_pending_query_result(pending, &result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_query_result_count(result, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == expected);
  type_bridge_pending_free(&pending);
  CHECK(type_bridge_query_result_close(&result) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int run_counter_plan_lifecycle(
    const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database,
    const type_bridge_query_execution_limits_v1_t *limits,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_status_t write_status;
  fixture_counter_update_batch_result *fixture_counter_update_result = NULL;
  type_bridge_iid_t *iid_argument = NULL;
  type_bridge_query_execution_limits_v1_t common_limits = *limits;
  common_limits.items = UINT64_C(100000);
  common_limits.bytes = UINT64_C(67108864);
  common_limits.graph_nodes = UINT64_C(1000000);
  common_limits.attribute_values = UINT64_C(4000000);
  common_limits.collection_members = UINT64_C(1000000);
  common_limits.role_players = UINT64_C(1000000);
  limits = &common_limits;
  fixture_counter_create *inputs[2] = {NULL, NULL};
  fixture_counter_create *replacement = NULL;
  fixture_counter *value = NULL;
  fixture_counterzuvalue *counter_value = NULL;
  fixture_counter_insert_batch_builder *insert_builder = NULL;
  fixture_counter_insert_batch *insert_batch = NULL;
  fixture_counter_insert_batch_result *insert_result = NULL;
  fixture_counter_update_batch_builder *update_builder = NULL;
  fixture_counter_update_batch *update_batch = NULL;
  fixture_counter_update_batch_result *update_result = NULL;
  fixture_counter_delete_batch_builder *delete_builder = NULL;
  fixture_counter_delete_batch *delete_batch = NULL;
  fixture_counter_delete_batch_result *delete_result = NULL;
  uint8_t iid_storage[2][256];
  size_t iid_lengths[2] = {0u, 0u};
  type_bridge_byte_view_t ids[2];
  type_bridge_byte_view_t borrowed = {NULL, 0u};
  uint64_t before = 0u;
  uint64_t count = 0u;
  size_t result_count = 0u;
  size_t index;
  int64_t scalar = 0;
  CHECK(fixture_counter_database_count(database, limits, NULL, &before, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_insert_batch_builder_open(package, limits, NULL, &insert_builder, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index = 0u; index < 2u; ++index) {
    CHECK(open_counter_create(package, (int64_t)index + 1, &inputs[index], out_diagnostics) == 0);
    CHECK(fixture_counter_insert_batch_builder_add(insert_builder, inputs[index], out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_counter_insert_batch_builder_finish(&insert_builder, &insert_batch, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("insert");
  CHECK(fixture_counter_database_insert_batch_execute(database, insert_batch, limits, NULL, &insert_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_counter_insert_batch_result_count(insert_result, &result_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && result_count == 2u);
  for (index = 0u; index < 2u; ++index) {
    CHECK(fixture_counter_insert_batch_result_thing_at(insert_result, index, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_iid(value, &borrowed, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_iid(borrowed, iid_storage[index], sizeof(iid_storage[index]), &iid_lengths[index]));
    ids[index].data = iid_storage[index];
    ids[index].length = iid_lengths[index];
    model_plan_identity(index == 0u ? "counter-left" : "counter-right", ids[index]);
    CHECK(fixture_counter_counterzuvalue(value, &counter_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counterzuvalue_value(counter_value, &scalar, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && scalar == (int64_t)index + 1);
    CHECK(fixture_counterzuvalue_close(&counter_value) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_close(&value) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_counter_insert_batch_result_close(&insert_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_insert_batch_close(&insert_batch) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("unkeyed_entity_iid_lifecycle.count");
  CHECK(fixture_counter_database_count(database, limits, NULL, &count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && count == before + 2u);
  model_plan_mark("");
  CHECK(run_staged_counter_count(package, database, before + 2u, out_diagnostics) == 0);
  CHECK(open_counter_create(package, 11, &replacement, out_diagnostics) == 0);
  model_plan_mark("update");
  CHECK(TEST_SINGLE_WRITE(fixture_counter, update, fixture_counter_database_update(database, checked_iid(ids[0], &iid_argument), replacement, limits, NULL,&fixture_counter_update_result, out_diagnostics), &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_counter_iid(value, &borrowed, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && borrowed.length == ids[0].length && memcmp(borrowed.data, ids[0].data, borrowed.length) == 0);
  CHECK(fixture_counter_counterzuvalue(value, &counter_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counterzuvalue_value(counter_value, &scalar, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && scalar == 11);
  CHECK(fixture_counterzuvalue_close(&counter_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_close(&value) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("unkeyed_entity_iid_lifecycle.get_by_iid");
  CHECK(fixture_counter_database_get_by_iid(database, checked_iid(ids[0], &iid_argument), limits, NULL, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_counter_counterzuvalue(value, &counter_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counterzuvalue_value(counter_value, &scalar, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && scalar == 11);
  CHECK(fixture_counterzuvalue_close(&counter_value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_close(&value) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_update_batch_builder_open(package, limits, NULL, &update_builder, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_update_batch_builder_add(update_builder, checked_iid(ids[0], &iid_argument), replacement, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_update_batch_builder_add(update_builder, checked_iid(ids[1], &iid_argument), inputs[1], out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_update_batch_builder_finish(&update_builder, &update_batch, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("entity_batch_update_delete_atomic.update");
  CHECK(fixture_counter_database_update_batch_execute(database, update_batch, limits, NULL, &update_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_counter_update_batch_result_count(update_result, &result_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && result_count == 2u);
  for (index=0u; index<2u; ++index) {
    CHECK(fixture_counter_update_batch_result_thing_at(update_result, index, &value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_iid(value, &borrowed, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && borrowed.length == ids[index].length && memcmp(borrowed.data, ids[index].data, borrowed.length) == 0);
    CHECK(fixture_counter_close(&value) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_counter_update_batch_result_close(&update_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_update_batch_close(&update_batch) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_delete_batch_builder_open(package, limits, NULL, &delete_builder, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index = 0u; index < 2u; ++index)
    CHECK(fixture_counter_delete_batch_builder_add(delete_builder, checked_iid(ids[index], &iid_argument), out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_delete_batch_builder_finish(&delete_builder, &delete_batch, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("delete");
  CHECK(fixture_counter_database_delete_batch_execute(database, delete_batch, limits, NULL, &delete_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_counter_delete_batch_result_count(delete_result, &result_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && result_count == 2u);
  CHECK(fixture_counter_delete_batch_result_close(&delete_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_delete_batch_close(&delete_batch) == TYPE_BRIDGE_STATUS_OK);
  {
    fixture_counter_manager manager = {NULL, {0u, 0u, 0u, 0u}};
    fixture_counter_delete_returning_batch_result *returned = NULL;
    fixture_counter *snapshot = NULL;
    type_bridge_iid_t *owned_iids[2] = {NULL, NULL};
    const type_bridge_iid_t *typed_iids[2];
    size_t returned_count = 0u;
    CHECK(fixture_counter_manager_open(package, &manager, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_insert_batch_builder_open(package, limits, NULL, &insert_builder, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    for (index = 0u; index < 2u; ++index) {
    CHECK(fixture_counter_insert_batch_builder_add(insert_builder, inputs[index], out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_counter_insert_batch_builder_finish(&insert_builder, &insert_batch, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_database_insert_batch_execute(database, insert_batch, limits, NULL, &insert_result, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    for (index = 0u; index < 2u; ++index) {
    CHECK(fixture_counter_insert_batch_result_thing_at(insert_result, index, &snapshot, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_iid(snapshot, &borrowed, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_iid_open(borrowed, &owned_iids[index]) == TYPE_BRIDGE_STATUS_OK);
      typed_iids[index] = owned_iids[index];
      model_plan_identity(index == 0u ? "counter-left" : "counter-right", borrowed);
    CHECK(fixture_counter_close(&snapshot) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_counter_insert_batch_result_close(&insert_result) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_insert_batch_close(&insert_batch) == TYPE_BRIDGE_STATUS_OK);
    for (index = 0u; index < 2u; ++index) {
      model_plan_mark(index == 0u ? "manager_returning_delete.entity" : "manager_returning_delete.entity_many");
      if (index == 0u) {
    CHECK(fixture_counter_manager_database_delete_returning(database, &manager, typed_iids[0], limits, NULL, &returned, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      } else {
    CHECK(fixture_counter_manager_database_delete_many_returning(database, &manager, typed_iids, 2u, limits, NULL, &returned, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
      }
      model_plan_mark("");
    CHECK(fixture_counter_delete_returning_batch_result_count(returned, &returned_count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && returned_count == 1u);
    CHECK(fixture_counter_delete_returning_batch_result_item(returned, &snapshot, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && snapshot != NULL);
    CHECK(fixture_counter_delete_returning_batch_result_close(&returned) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_counterzuvalue(snapshot, &counter_value, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counterzuvalue_value(counter_value, &scalar, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && scalar == (int64_t)index + 1);
    CHECK(fixture_counterzuvalue_close(&counter_value) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_counter_close(&snapshot) == TYPE_BRIDGE_STATUS_OK);
    }
    for (index = 0u; index < 2u; ++index) {
    CHECK(type_bridge_iid_close(&owned_iids[index]) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_counter_manager_close(&manager) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_counter_database_count(database, limits, NULL, &count, out_diagnostics) == TYPE_BRIDGE_STATUS_OK && count == before);
  CHECK(fixture_counter_create_close(&replacement) == TYPE_BRIDGE_STATUS_OK);
  for (index = 0u; index < 2u; ++index)
    CHECK(fixture_counter_create_close(&inputs[index]) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int open_empty_ordered_link(
    const type_bridge_schema_package_t *package, type_bridge_byte_view_t iid,
    const char *identifier_text, int nonempty, fixture_networkzhlink_create **out,
    type_bridge_execution_diagnostics_t **diagnostics) {
  fixture_networkzhlink_create_args_v1_t args;
  fixture_identifier *identifier = NULL;
  fixture_person_ref *reference = NULL;
  fixture_networkzhlink_origin_player *origin = NULL;
  fixture_networkzhlink_destination_player *destination = NULL;
  fixture_networkzhlink_participant_player *participant = NULL;
  const fixture_networkzhlink_participant_player *participants[1];
  fixture_networkzhlink_create_role_participant_chunks_v1_t chunk;
  memset(&args, 0, sizeof(args));
  memset(&chunk, 0, sizeof(chunk));
  CHECK(fixture_identifier_open(package, view_of(identifier_text), &identifier, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_from_iid(package, iid, &reference, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_origin_player_from_person(reference, &origin, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_destination_player_from_person(reference, &destination, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  if (nonempty) {
    CHECK(fixture_networkzhlink_participant_player_from_person(reference, &participant, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    participants[0] = participant;
    chunk.struct_size = sizeof(chunk);
    chunk.version = FIXTURE_CREATE_ARGS_VERSION;
    chunk.values = participants;
    chunk.count = 1u;
    args.role_participant_chunks = &chunk;
  }
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  args.field_identifier = identifier;
  args.role_origin = origin;
  args.role_destination = destination;
  CHECK(fixture_networkzhlink_create_open(package, &args, out, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  if (participant != NULL) CHECK(fixture_networkzhlink_participant_player_close(&participant) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_destination_player_close(&destination) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_origin_player_close(&origin) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_ref_close(&reference) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_identifier_close(&identifier) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int check_ordered_refusal(type_bridge_execution_diagnostics_t **diagnostics) {
  type_bridge_execution_diagnostic_view_v1_t view;
  size_t count = 0u;
  CHECK(type_bridge_execution_diagnostics_count(*diagnostics, &count) == TYPE_BRIDGE_STATUS_OK && count == 1u);
  CHECK(type_bridge_execution_diagnostics_get_v1(*diagnostics, 0u, &view) == TYPE_BRIDGE_STATUS_OK);
  CHECK(view.category == TYPE_BRIDGE_EXECUTION_DIAGNOSTIC_UNSUPPORTED_CAPABILITY);
  CHECK(same_text(view.code, "query_plan_ordered_collection_unsupported"));
  CHECK(type_bridge_execution_diagnostics_close(diagnostics) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int run_manager_batch_lifecycle(
    const type_bridge_schema_package_t *package, const type_bridge_database_t *database,
    type_bridge_byte_view_t person_iid, const type_bridge_query_execution_limits_v1_t *limits,
    type_bridge_execution_diagnostics_t **diagnostics) {
  type_bridge_iid_t *iid_argument = NULL;
  fixture_person_create *people[2] = {NULL, NULL};
  fixture_person *person = NULL;
  fixture_person_put_batch_builder *people_builder = NULL;
  fixture_person_put_batch *people_batch = NULL;
  fixture_person_put_batch_result *people_result = NULL;
  fixture_networkzhlink_create *links[2] = {NULL, NULL};
  fixture_networkzhlink *link = NULL;
  fixture_networkzhlink_put_batch_builder *put_builder = NULL;
  fixture_networkzhlink_put_batch *put_batch = NULL;
  fixture_networkzhlink_put_batch_result *put_result = NULL;
  fixture_networkzhlink_insert_batch_builder *insert_builder = NULL;
  fixture_networkzhlink_insert_batch *insert_batch = NULL;
  fixture_networkzhlink_insert_batch_result *insert_result = NULL;
  fixture_networkzhlink_update_batch_builder *update_builder = NULL;
  fixture_networkzhlink_update_batch *update_batch = NULL;
  fixture_networkzhlink_update_batch_result *update_result = NULL;
  fixture_networkzhlink_delete_batch_builder *delete_builder = NULL;
  fixture_networkzhlink_delete_batch *delete_batch = NULL;
  fixture_networkzhlink_delete_batch_result *delete_result = NULL;
  uint8_t person_storage[256], link_storage[2][256];
  size_t person_length = 0u, lengths[2] = {0u, 0u}, count = 0u, index;
  type_bridge_byte_view_t borrowed, dana_iid, ids[2];
  CHECK(open_person_create(package, "data-ada", 0, &people[0], diagnostics) == 0);
  CHECK(open_person_create(package, "data-dana", 0, &people[1], diagnostics) == 0);
  CHECK(fixture_person_put_batch_builder_open(package, limits, NULL, &people_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_person_put_batch_builder_add(people_builder, people[index], diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_put_batch_builder_finish(&people_builder, &people_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("entity_batch_insert_put.put");
  CHECK(fixture_person_database_put_batch_execute(database, people_batch, limits, NULL, &people_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_person_put_batch_result_count(people_result, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 2u);
  for (index=0u; index<2u; ++index) {
    CHECK(fixture_person_put_batch_result_thing_at(people_result, index, &person, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_person_iid(person, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    if (index==0u) CHECK(borrowed.length == person_iid.length && memcmp(borrowed.data, person_iid.data, borrowed.length) == 0);
    else CHECK(copy_iid(borrowed, person_storage, sizeof(person_storage), &person_length));
    CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  }
  dana_iid.data = person_storage; dana_iid.length = person_length;
  model_plan_identity("data-dana", dana_iid);
  CHECK(fixture_person_put_batch_result_close(&people_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_put_batch_close(&people_batch) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_person_create_close(&people[index]) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_database_delete_by_iid(database, checked_iid(dana_iid, &iid_argument), limits, NULL, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_empty_ordered_link(package, person_iid, "data-link-forward", 0, &links[0], diagnostics) == 0);
  CHECK(open_empty_ordered_link(package, person_iid, "data-link-return", 0, &links[1], diagnostics) == 0);
  CHECK(fixture_networkzhlink_put_batch_builder_open(package, limits, NULL, &put_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_networkzhlink_put_batch_builder_add(put_builder, links[index], diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_put_batch_builder_finish(&put_builder, &put_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("relation_batch_insert_put.put");
  CHECK(fixture_networkzhlink_database_put_batch_execute(database, put_batch, limits, NULL, &put_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_put_batch_result_count(put_result, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 2u);
  for (index=0u; index<2u; ++index) {
    CHECK(fixture_networkzhlink_put_batch_result_thing_at(put_result, index, &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_iid(link, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_iid(borrowed, link_storage[index], sizeof(link_storage[index]), &lengths[index]));
    ids[index].data = link_storage[index]; ids[index].length = lengths[index];
    model_plan_identity(index==0u ? "link-forward" : "link-return", ids[index]);
    CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_networkzhlink_put_batch_result_close(&put_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_put_batch_close(&put_batch) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_builder_open(package, limits, NULL, &delete_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_networkzhlink_delete_batch_builder_add(delete_builder, checked_iid(ids[index], &iid_argument), diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_builder_finish(&delete_builder, &delete_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_database_delete_batch_execute(database, delete_batch, limits, NULL, &delete_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_result_close(&delete_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_close(&delete_batch) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_insert_batch_builder_open(package, limits, NULL, &insert_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_networkzhlink_insert_batch_builder_add(insert_builder, links[index], diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_insert_batch_builder_finish(&insert_builder, &insert_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("relation_batch_insert_put.insert");
  CHECK(fixture_networkzhlink_database_insert_batch_execute(database, insert_batch, limits, NULL, &insert_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_insert_batch_result_count(insert_result, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 2u);
  for (index=0u; index<2u; ++index) {
    CHECK(fixture_networkzhlink_insert_batch_result_thing_at(insert_result, index, &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_iid(link, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(copy_iid(borrowed, link_storage[index], sizeof(link_storage[index]), &lengths[index]));
    ids[index].data = link_storage[index]; ids[index].length = lengths[index];
    model_plan_identity(index==0u ? "link-forward" : "link-return", ids[index]);
    CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_networkzhlink_insert_batch_result_close(&insert_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_insert_batch_close(&insert_batch) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_update_batch_builder_open(package, limits, NULL, &update_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_networkzhlink_update_batch_builder_add(update_builder, checked_iid(ids[index], &iid_argument), links[index], diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_update_batch_builder_finish(&update_builder, &update_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("relation_batch_update_delete_atomic.update");
  CHECK(fixture_networkzhlink_database_update_batch_execute(database, update_batch, limits, NULL, &update_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_update_batch_result_count(update_result, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 2u);
  for (index=0u; index<2u; ++index) {
    CHECK(fixture_networkzhlink_update_batch_result_thing_at(update_result, index, &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_iid(link, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK && borrowed.length == ids[index].length && memcmp(borrowed.data, ids[index].data, borrowed.length) == 0);
    CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_networkzhlink_update_batch_result_close(&update_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_update_batch_close(&update_batch) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_builder_open(package, limits, NULL, &delete_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  for (index=0u; index<2u; ++index) CHECK(fixture_networkzhlink_delete_batch_builder_add(delete_builder, checked_iid(ids[index], &iid_argument), diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_builder_finish(&delete_builder, &delete_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("relation_batch_update_delete_atomic.delete");
  CHECK(fixture_networkzhlink_database_delete_batch_execute(database, delete_batch, limits, NULL, &delete_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_delete_batch_result_count(delete_result, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 2u);
  CHECK(fixture_networkzhlink_delete_batch_result_close(&delete_result) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_delete_batch_close(&delete_batch) == TYPE_BRIDGE_STATUS_OK);
  {
    fixture_networkzhlink_manager manager = {NULL, {0u, 0u, 0u, 0u}};
    fixture_networkzhlink_delete_returning_batch_result *returned = NULL;
    fixture_networkzhlink *snapshot = NULL;
    type_bridge_iid_t *owned_iids[2] = {NULL, NULL};
    const type_bridge_iid_t *typed_iids[2];
    size_t returned_count = 0u;
    CHECK(fixture_networkzhlink_manager_open(package, &manager, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_insert_batch_builder_open(package, limits, NULL, &insert_builder, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    for (index = 0u; index < 2u; ++index) {
    CHECK(fixture_networkzhlink_insert_batch_builder_add(insert_builder, links[index], diagnostics) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_networkzhlink_insert_batch_builder_finish(&insert_builder, &insert_batch, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_database_insert_batch_execute(database, insert_batch, limits, NULL, &insert_result, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    for (index = 0u; index < 2u; ++index) {
    CHECK(fixture_networkzhlink_insert_batch_result_thing_at(insert_result, index, &snapshot, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_iid(snapshot, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_iid_open(borrowed, &owned_iids[index]) == TYPE_BRIDGE_STATUS_OK);
      typed_iids[index] = owned_iids[index];
      model_plan_identity(index == 0u ? "link-forward" : "link-return", borrowed);
    CHECK(fixture_networkzhlink_close(&snapshot) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_networkzhlink_insert_batch_result_close(&insert_result) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_networkzhlink_insert_batch_close(&insert_batch) == TYPE_BRIDGE_STATUS_OK);
    for (index = 0u; index < 2u; ++index) {
      model_plan_mark(index == 0u ? "manager_returning_delete.relation" : "manager_returning_delete.relation_many");
      if (index == 0u) {
    CHECK(fixture_networkzhlink_manager_database_delete_returning(database, &manager, typed_iids[0], limits, NULL, &returned, diagnostics) == TYPE_BRIDGE_STATUS_OK);
      } else {
    CHECK(fixture_networkzhlink_manager_database_delete_many_returning(database, &manager, typed_iids, 2u, limits, NULL, &returned, diagnostics) == TYPE_BRIDGE_STATUS_OK);
      }
      model_plan_mark("");
    CHECK(fixture_networkzhlink_delete_returning_batch_result_count(returned, &returned_count, diagnostics) == TYPE_BRIDGE_STATUS_OK && returned_count == 1u);
    CHECK(fixture_networkzhlink_delete_returning_batch_result_item(returned, &snapshot, diagnostics) == TYPE_BRIDGE_STATUS_OK && snapshot != NULL);
    CHECK(fixture_networkzhlink_delete_returning_batch_result_close(&returned) == TYPE_BRIDGE_STATUS_OK);
      {
        fixture_identifier *key = NULL;
        type_bridge_byte_view_t text = {NULL, 0u};
        const char *expected = index == 0u ? "data-link-forward" : "data-link-return";
    CHECK(fixture_networkzhlink_identifier(snapshot, &key, diagnostics) == TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_identifier_value(key, &text, diagnostics) == TYPE_BRIDGE_STATUS_OK);
        CHECK(text.length == strlen(expected) && memcmp(text.data, expected, text.length) == 0);
    CHECK(fixture_identifier_close(&key) == TYPE_BRIDGE_STATUS_OK);
      }
    CHECK(fixture_networkzhlink_close(&snapshot) == TYPE_BRIDGE_STATUS_OK);
    }
    for (index = 0u; index < 2u; ++index) {
    CHECK(type_bridge_iid_close(&owned_iids[index]) == TYPE_BRIDGE_STATUS_OK);
    }
    CHECK(fixture_networkzhlink_manager_close(&manager) == TYPE_BRIDGE_STATUS_OK);
  }
  for (index=0u; index<2u; ++index) CHECK(fixture_networkzhlink_create_close(&links[index]) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int run_empty_ordered_lifecycle(
    const type_bridge_schema_package_t *package, const type_bridge_database_t *database,
    const type_bridge_query_execution_limits_v1_t *limits,
    type_bridge_execution_diagnostics_t **diagnostics) {
  type_bridge_status_t write_status;
  fixture_networkzhlink_insert_batch_result *fixture_networkzhlink_insert_result = NULL;
  fixture_networkzhlink_put_batch_result *fixture_networkzhlink_put_result = NULL;
  fixture_networkzhlink_update_batch_result *fixture_networkzhlink_update_result = NULL;
  fixture_person_insert_batch_result *fixture_person_insert_result = NULL;
  fixture_person_put_batch_result *fixture_person_put_result = NULL;
  fixture_person_update_batch_result *fixture_person_update_result = NULL;
  type_bridge_iid_t *iid_argument = NULL;
  fixture_person_create *input = NULL;
  fixture_person *person = NULL;
  fixture_networkzhlink_create *link_input = NULL;
  fixture_networkzhlink *link = NULL;
  uint64_t total = 0u;
  uint8_t person_storage[256], link_storage[256];
  size_t person_length = 0u, link_length = 0u, count = 0u;
  type_bridge_byte_view_t person_iid, link_iid, borrowed;
  CHECK(open_person_create(package, "data-ada", 0, &input, diagnostics) == 0);
  model_plan_mark("ordered_distinct_collections.entity_insert");
  CHECK(TEST_SINGLE_WRITE(fixture_person, insert, fixture_person_database_insert(database, input, limits, NULL,&fixture_person_insert_result, diagnostics), &person, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_person_iid(person, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, person_storage, sizeof(person_storage), &person_length));
  person_iid.data = person_storage; person_iid.length = person_length;
  model_plan_identity("data-ada", person_iid);
  CHECK(fixture_person_aliases_count(person, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 0u);
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.entity_put");
  CHECK(TEST_SINGLE_WRITE(fixture_person, put, fixture_person_database_put(database, input, limits, NULL,&fixture_person_put_result, diagnostics), &person, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_person_iid(person, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(borrowed.length == person_iid.length && memcmp(borrowed.data, person_iid.data, borrowed.length) == 0);
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.entity_count");
  /* The Query consumer already owns its query-target person. */
  CHECK(fixture_person_database_count(database, limits, NULL, &total, diagnostics) == TYPE_BRIDGE_STATUS_OK && total == 2u);
  model_plan_mark("");
  model_plan_mark("ordered_distinct_collections.entity_update");
  CHECK(TEST_SINGLE_WRITE(fixture_person, update, fixture_person_database_update(database, checked_iid(person_iid, &iid_argument), input, limits, NULL,&fixture_person_update_result, diagnostics), &person, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.entity_read");
  CHECK(fixture_person_database_get_by_iid(database, checked_iid(person_iid, &iid_argument), limits, NULL, &person, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_person_aliases_count(person, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 0u);
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_empty_ordered_link(package, person_iid, "data-link-forward", 0, &link_input, diagnostics) == 0);
  model_plan_mark("ordered_distinct_collections.role_insert");
  CHECK(TEST_SINGLE_WRITE(fixture_networkzhlink, insert, fixture_networkzhlink_database_insert(database, link_input, limits, NULL,&fixture_networkzhlink_insert_result, diagnostics), &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_iid(link, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, link_storage, sizeof(link_storage), &link_length));
  link_iid.data = link_storage; link_iid.length = link_length;
  model_plan_identity("link-forward", link_iid);
  CHECK(fixture_networkzhlink_participant_count(link, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 0u);
  CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.role_put");
  CHECK(TEST_SINGLE_WRITE(fixture_networkzhlink, put, fixture_networkzhlink_database_put(database, link_input, limits, NULL,&fixture_networkzhlink_put_result, diagnostics), &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_iid(link, &borrowed, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(borrowed.length == link_iid.length && memcmp(borrowed.data, link_iid.data, borrowed.length) == 0);
  CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.role_count");
  CHECK(fixture_networkzhlink_database_count(database, limits, NULL, &total, diagnostics) == TYPE_BRIDGE_STATUS_OK && total == 1u);
  model_plan_mark("");
  model_plan_mark("ordered_distinct_collections.role_update");
  CHECK(TEST_SINGLE_WRITE(fixture_networkzhlink, update, fixture_networkzhlink_database_update(database, checked_iid(link_iid, &iid_argument), link_input, limits, NULL,&fixture_networkzhlink_update_result, diagnostics), &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_participant_count(link, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 0u);
  CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.role_read");
  CHECK(fixture_networkzhlink_database_get_by_iid(database, checked_iid(link_iid, &iid_argument), limits, NULL, &link, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_participant_count(link, &count, diagnostics) == TYPE_BRIDGE_STATUS_OK && count == 0u);
  CHECK(fixture_networkzhlink_close(&link) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_create_close(&input) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_person_create(package, "data-ada", 1, &input, diagnostics) == 0);
  model_plan_mark("ordered_distinct_collections.refused_nonempty");
  CHECK(TEST_SINGLE_WRITE(fixture_person, insert, fixture_person_database_insert(database, input, limits, NULL,&fixture_person_insert_result, diagnostics), &person, diagnostics) == TYPE_BRIDGE_STATUS_UNSUPPORTED);
  CHECK(person == NULL && check_ordered_refusal(diagnostics) == 0);
  model_plan_mark("");
  CHECK(fixture_person_create_close(&input) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_networkzhlink_create_close(&link_input) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_empty_ordered_link(package, person_iid, "data-link-forward", 1, &link_input, diagnostics) == 0);
  model_plan_mark("ordered_distinct_collections.refused_nonempty");
  CHECK(TEST_SINGLE_WRITE(fixture_networkzhlink, insert, fixture_networkzhlink_database_insert(database, link_input, limits, NULL,&fixture_networkzhlink_insert_result, diagnostics), &link, diagnostics) == TYPE_BRIDGE_STATUS_UNSUPPORTED);
  CHECK(link == NULL && check_ordered_refusal(diagnostics) == 0);
  model_plan_mark("");
  CHECK(fixture_networkzhlink_create_close(&link_input) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("ordered_distinct_collections.role_delete");
  CHECK(fixture_networkzhlink_database_delete_by_iid(database, checked_iid(link_iid, &iid_argument), limits, NULL, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(run_manager_batch_lifecycle(package, database, person_iid, limits, diagnostics) == 0);
  model_plan_mark("ordered_distinct_collections.entity_delete");
  CHECK(fixture_person_database_delete_by_iid(database, checked_iid(person_iid, &iid_argument), limits, NULL, diagnostics) == TYPE_BRIDGE_STATUS_OK);
  model_plan_mark("");
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
run_unkeyed_lifecycles(const type_bridge_schema_package_t *package,
                       const type_bridge_database_t *database,
                       type_bridge_byte_view_t person_iid,
                       const type_bridge_query_execution_limits_v1_t *limits,
                       type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_status_t write_status;
  fixture_counter_insert_batch_result *fixture_counter_insert_result = NULL;
  fixture_counter_update_batch_result *fixture_counter_update_result = NULL;
  fixture_membership_insert_batch_result *fixture_membership_insert_result = NULL;
  fixture_membership_update_batch_result *fixture_membership_update_result = NULL;
  type_bridge_iid_t *iid_argument = NULL;
  uint8_t iid_storage[256];
  size_t iid_length = 0u;
  type_bridge_byte_view_t iid;
  type_bridge_byte_view_t borrowed;
  uint64_t count = 0u;
  fixture_counter_create *counter_create = NULL;
  fixture_counter_create *counter_update = NULL;
  fixture_counter *counter = NULL;
  fixture_membership_create *membership_create = NULL;
  fixture_membership *membership = NULL;

  CHECK(open_counter_create(package, 10, &counter_create, out_diagnostics) ==
        0);
  CHECK(TEST_SINGLE_WRITE(fixture_counter, insert, fixture_counter_database_insert(database, counter_create, limits,
                                           NULL,&fixture_counter_insert_result, out_diagnostics), &counter, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_iid(counter, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, iid_storage, sizeof(iid_storage), &iid_length));
  iid.data = iid_storage;
  iid.length = iid_length;
  CHECK(fixture_counter_close(&counter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_database_get_by_iid(database, checked_iid(iid, &iid_argument), limits, NULL,
                                               &counter, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_close(&counter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_counter_create(package, 11, &counter_update, out_diagnostics) ==
        0);
  CHECK(TEST_SINGLE_WRITE(fixture_counter, update, fixture_counter_database_update(
            database, checked_iid(iid, &iid_argument), counter_update, limits, NULL,&fixture_counter_update_result,
            out_diagnostics), &counter, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_close(&counter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_database_delete_by_iid(database, checked_iid(iid, &iid_argument), limits, NULL,
                                                  out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_database_count(database, limits, NULL, &count,
                                          out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(fixture_counter_create_close(&counter_update) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_create_close(&counter_create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(open_membership_create(package, person_iid, &membership_create,
                               out_diagnostics) == 0);
  CHECK(TEST_SINGLE_WRITE(fixture_membership, insert, fixture_membership_database_insert(
            database, membership_create, limits, NULL,&fixture_membership_insert_result,
            out_diagnostics), &membership, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_iid(membership, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, iid_storage, sizeof(iid_storage), &iid_length));
  iid.data = iid_storage;
  iid.length = iid_length;
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_get_by_iid(
            database, checked_iid(iid, &iid_argument), limits, NULL, &membership, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  CHECK(TEST_SINGLE_WRITE(fixture_membership, update, fixture_membership_database_update(
            database, checked_iid(iid, &iid_argument), membership_create, limits, NULL,&fixture_membership_update_result,
            out_diagnostics), &membership, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_delete_by_iid(database, checked_iid(iid, &iid_argument), limits,
                                                     NULL, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_count(database, limits, NULL, &count,
                                             out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(fixture_membership_create_close(&membership_create) ==
        TYPE_BRIDGE_STATUS_OK);
  puts("ABI 1.6 unkeyed entity/relation IID lifecycle: passed");
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
run_robot_batches(const type_bridge_schema_package_t *package,
                  const type_bridge_database_t *database,
                  const type_bridge_query_execution_limits_v1_t *limits,
                  type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_iid_t *iid_argument = NULL;
  uint8_t insert_iid_storage[256];
  uint8_t put_iid_storage[256];
  size_t insert_iid_length = 0u;
  size_t put_iid_length = 0u;
  type_bridge_byte_view_t insert_iid;
  type_bridge_byte_view_t put_iid;
  type_bridge_byte_view_t borrowed;
  uint64_t count = 0u;
  size_t result_count = 0u;
  fixture_robot_create *create = NULL;
  fixture_robot *robot = NULL;
  fixture_robot_insert_batch_builder *insert_builder = NULL;
  fixture_robot_insert_batch *insert_batch = NULL;
  fixture_robot_insert_batch_result *insert_result = NULL;
  fixture_robot_put_batch_builder *put_builder = NULL;
  fixture_robot_put_batch *put_batch = NULL;
  fixture_robot_put_batch_result *put_result = NULL;
  fixture_robot_update_batch_builder *update_builder = NULL;
  fixture_robot_update_batch *update_batch = NULL;
  fixture_robot_update_batch_result *update_result = NULL;
  fixture_robot_delete_batch_builder *delete_builder = NULL;
  fixture_robot_delete_batch *delete_batch = NULL;
  fixture_robot_delete_batch_result *delete_result = NULL;
  type_bridge_write_transaction_t *write_transaction = NULL;

  CHECK(open_robot_create(package, 100, 10, &create, out_diagnostics) == 0);
  CHECK(fixture_robot_insert_batch_builder_open(
            package, limits, NULL, &insert_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_insert_batch_builder_add(
            insert_builder, create, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_insert_batch_builder_finish(
            &insert_builder, &insert_batch, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(insert_builder == NULL);
  CHECK(fixture_robot_database_insert_batch_execute(
            database, insert_batch, limits, NULL, &insert_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_insert_batch_result_count(insert_result, &result_count,
                                                out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_robot_insert_batch_result_thing_at(insert_result, 0u, &robot,
                                                   out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_iid(robot, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, insert_iid_storage, sizeof(insert_iid_storage),
                 &insert_iid_length));
  insert_iid.data = insert_iid_storage;
  insert_iid.length = insert_iid_length;
  CHECK(fixture_robot_close(&robot) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_insert_batch_result_close(&insert_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_insert_batch_close(&insert_batch) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_create_close(&create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(open_robot_create(package, 101, 11, &create, out_diagnostics) == 0);
  CHECK(fixture_robot_put_batch_builder_open(package, limits, NULL,
                                             &put_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_put_batch_builder_add(
            put_builder, create, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_put_batch_builder_finish(&put_builder, &put_batch,
                                               out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_put_batch_execute(
            database, put_batch, limits, NULL, &put_result, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_put_batch_result_count(put_result, &result_count,
                                             out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_robot_put_batch_result_thing_at(
            put_result, 0u, &robot, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_iid(robot, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, put_iid_storage, sizeof(put_iid_storage),
                 &put_iid_length));
  put_iid.data = put_iid_storage;
  put_iid.length = put_iid_length;
  CHECK(fixture_robot_close(&robot) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_put_batch_result_close(&put_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_put_batch_close(&put_batch) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_create_close(&create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(open_robot_create(package, 100, 12, &create, out_diagnostics) == 0);
  CHECK(fixture_robot_update_batch_builder_open(
            package, limits, NULL, &update_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_update_batch_builder_add(update_builder, checked_iid(insert_iid, &iid_argument),
                                               create, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_update_batch_builder_finish(
            &update_builder, &update_batch, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_update_batch_execute(
            database, update_batch, limits, NULL, &update_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_update_batch_result_count(update_result, &result_count,
                                                out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_robot_update_batch_result_thing_at(update_result, 0u, &robot,
                                                   out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_close(&robot) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_update_batch_result_close(&update_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_update_batch_close(&update_batch) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_create_close(&create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_robot_delete_batch_builder_open(
            package, limits, NULL, &delete_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_delete_batch_builder_add(delete_builder, checked_iid(insert_iid, &iid_argument),
                                               out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_delete_batch_builder_finish(
            &delete_builder, &delete_batch, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_delete_batch_execute(
            database, delete_batch, limits, NULL, &delete_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_delete_batch_result_count(delete_result, &result_count,
                                                out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_robot_delete_batch_result_close(&delete_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_delete_batch_close(&delete_batch) ==
        TYPE_BRIDGE_STATUS_OK);

  /* Row 0 is new; row 1 collides with the still-live put row. The provider
   * failure must roll back row 0 as part of the batch transaction. */
  CHECK(fixture_robot_database_count(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);
  CHECK(open_robot_create(package, 102, 12, &create, out_diagnostics) == 0);
  {
    fixture_robot_create *conflict = NULL;
    CHECK(open_robot_create(package, 101, 13, &conflict, out_diagnostics) == 0);
    CHECK(fixture_robot_insert_batch_builder_open(
              package, limits, NULL, &insert_builder, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_builder_add(insert_builder, create,
                                                 out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_builder_add(insert_builder, conflict,
                                                 out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_builder_finish(
              &insert_builder, &insert_batch, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_database_insert_batch_execute(
              database, insert_batch, limits, NULL, &insert_result,
              out_diagnostics) != TYPE_BRIDGE_STATUS_OK);
    CHECK(insert_result == NULL && *out_diagnostics != NULL);
    CHECK(check_exact_provider_write_failure(*out_diagnostics) == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_close(&insert_batch) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_create_close(&conflict) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_robot_create_close(&create) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_count(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);

  /* Repeat the later-row collision through a borrowed write owner. The
   * failure latches the provider transaction rollback-only. Commit must reject
   * before provider dispatch and retain the owner for explicit rollback. */
  CHECK(open_robot_create(package, 103, 14, &create, out_diagnostics) == 0);
  {
    fixture_robot_create *conflict = NULL;
    type_bridge_status_t rollback_status;
    CHECK(open_robot_create(package, 101, 15, &conflict, out_diagnostics) == 0);
    CHECK(fixture_robot_insert_batch_builder_open(
              package, limits, NULL, &insert_builder, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_builder_add(insert_builder, create,
                                                 out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_builder_add(insert_builder, conflict,
                                                 out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_insert_batch_builder_finish(
              &insert_builder, &insert_batch, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_write_transaction_open(
              database, limits, NULL, &write_transaction, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_write_transaction_insert_batch_execute(
              write_transaction, insert_batch, limits, NULL, &insert_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
    CHECK(insert_result == NULL && *out_diagnostics != NULL);
    CHECK(check_exact_provider_write_failure(*out_diagnostics) == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_write_transaction_commit(&write_transaction, limits,
                                                  NULL, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
    CHECK(write_transaction != NULL && *out_diagnostics != NULL);
    CHECK(check_exact_rollback_only(*out_diagnostics) == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    rollback_status = type_bridge_write_transaction_rollback(&write_transaction,
                                                             out_diagnostics);
    CHECK(write_transaction == NULL);
    if (rollback_status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED) {
      /* TypeDB may already have terminally aborted its provider transaction
       * after the duplicate-key statement. The public owner is still consumed
       * by rollback, and the state proof below remains authoritative. */
      CHECK(*out_diagnostics != NULL);
      CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK);
    } else {
      CHECK(rollback_status == TYPE_BRIDGE_STATUS_OK &&
            *out_diagnostics == NULL);
    }
    CHECK(fixture_robot_insert_batch_close(&insert_batch) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_create_close(&conflict) == TYPE_BRIDGE_STATUS_OK);
  }
  CHECK(fixture_robot_create_close(&create) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_count(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);
  CHECK(fixture_robot_database_delete_by_iid(database, checked_iid(put_iid, &iid_argument), limits, NULL,
                                                out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_count(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  puts("ABI 1.6 entity insert/put/update/delete batches: passed");
  puts("ABI 1.6 later-row failure has zero committed prefix: passed");
  puts("ABI 1.6 borrowed failure is rollback-only before commit: passed");
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

static int
run_interaction_batches(const type_bridge_schema_package_t *package,
                        const type_bridge_database_t *database,
                        type_bridge_byte_view_t person_iid,
                        const type_bridge_query_execution_limits_v1_t *limits,
                        type_bridge_execution_diagnostics_t **out_diagnostics) {
  type_bridge_iid_t *iid_argument = NULL;
  uint8_t insert_iid_storage[256];
  uint8_t put_iid_storage[256];
  size_t insert_iid_length = 0u;
  size_t put_iid_length = 0u;
  type_bridge_byte_view_t insert_iid;
  type_bridge_byte_view_t put_iid;
  type_bridge_byte_view_t borrowed;
  size_t result_count = 0u;
  uint64_t count = 0u;
  fixture_interaction_create *create = NULL;
  fixture_interaction *interaction = NULL;
  fixture_interaction_insert_batch_builder *insert_builder = NULL;
  fixture_interaction_insert_batch *insert_batch = NULL;
  fixture_interaction_insert_batch_result *insert_result = NULL;
  fixture_interaction_put_batch_builder *put_builder = NULL;
  fixture_interaction_put_batch *put_batch = NULL;
  fixture_interaction_put_batch_result *put_result = NULL;
  fixture_interaction_update_batch_builder *update_builder = NULL;
  fixture_interaction_update_batch *update_batch = NULL;
  fixture_interaction_update_batch_result *update_result = NULL;
  fixture_interaction_delete_batch_builder *delete_builder = NULL;
  fixture_interaction_delete_batch *delete_batch = NULL;
  fixture_interaction_delete_batch_result *delete_result = NULL;

  CHECK(open_interaction_create(package, "query-interaction-insert",
                                person_iid, &create, out_diagnostics) == 0);
  CHECK(fixture_interaction_insert_batch_builder_open(
            package, limits, NULL, &insert_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_insert_batch_builder_add(
            insert_builder, create, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_insert_batch_builder_finish(
            &insert_builder, &insert_batch, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_insert_batch_execute(
            database, insert_batch, limits, NULL, &insert_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_insert_batch_result_count(
            insert_result, &result_count, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_interaction_insert_batch_result_thing_at(
            insert_result, 0u, &interaction, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_iid(interaction, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, insert_iid_storage, sizeof(insert_iid_storage),
                 &insert_iid_length));
  insert_iid.data = insert_iid_storage;
  insert_iid.length = insert_iid_length;
  CHECK(fixture_interaction_close(&interaction) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_insert_batch_result_close(&insert_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_insert_batch_close(&insert_batch) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_create_close(&create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(open_interaction_create(package, "query-interaction-put", person_iid,
                                &create, out_diagnostics) == 0);
  CHECK(fixture_interaction_put_batch_builder_open(
            package, limits, NULL, &put_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_put_batch_builder_add(
            put_builder, create, out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_put_batch_builder_finish(&put_builder, &put_batch,
                                                     out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_put_batch_execute(
            database, put_batch, limits, NULL, &put_result, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_put_batch_result_count(put_result, &result_count,
                                                   out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_interaction_put_batch_result_thing_at(
            put_result, 0u, &interaction, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_iid(interaction, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, put_iid_storage, sizeof(put_iid_storage),
                 &put_iid_length));
  put_iid.data = put_iid_storage;
  put_iid.length = put_iid_length;
  CHECK(fixture_interaction_close(&interaction) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_put_batch_result_close(&put_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_put_batch_close(&put_batch) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_create_close(&create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(open_interaction_create(package, "query-interaction-insert",
                                person_iid, &create, out_diagnostics) == 0);
  CHECK(fixture_interaction_update_batch_builder_open(
            package, limits, NULL, &update_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_update_batch_builder_add(update_builder, checked_iid(insert_iid, &iid_argument),
                                                     create, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_update_batch_builder_finish(
            &update_builder, &update_batch, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_update_batch_execute(
            database, update_batch, limits, NULL, &update_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_update_batch_result_count(
            update_result, &result_count, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_interaction_update_batch_result_thing_at(
            update_result, 0u, &interaction, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_close(&interaction) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_update_batch_result_close(&update_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_update_batch_close(&update_batch) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_create_close(&create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(fixture_interaction_delete_batch_builder_open(
            package, limits, NULL, &delete_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_delete_batch_builder_add(delete_builder, checked_iid(insert_iid, &iid_argument),
                                                     out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_delete_batch_builder_finish(
            &delete_builder, &delete_batch, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_delete_batch_execute(
            database, delete_batch, limits, NULL, &delete_result,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_delete_batch_result_count(
            delete_result, &result_count, out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        result_count == 1u);
  CHECK(fixture_interaction_delete_batch_result_close(&delete_result) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_delete_batch_close(&delete_batch) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_delete_by_iid(database, checked_iid(put_iid, &iid_argument), limits,
                                                      NULL, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_count(database, limits, NULL, &count,
                                              out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  puts("ABI 1.6 relation insert/put/update/delete batches: passed");
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}

/* A database's answer ceiling binds every call on it: a default-limit
 * count on a zero-timeout ceiling refuses before provider work, with the
 * same refusal a zero-timeout call limit yields on an unbounded database. */
static int check_answer_ceiling_binds_calls(
    type_bridge_runtime_t *runtime, const type_bridge_schema_package_t *package,
    const type_bridge_database_t *database,
    const type_bridge_database_config_t *base_config,
    const type_bridge_query_execution_limits_v1_t *limits) {
  type_bridge_database_config_t config = *base_config;
  type_bridge_query_execution_limits_v1_t bounded_limits = *limits;
  type_bridge_database_t *bounded = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_execution_diagnostic_view_v1_t ceiling_refusal;
  type_bridge_execution_diagnostic_view_v1_t call_refusal;
  type_bridge_execution_diagnostics_t *call_diagnostics = NULL;
  uint64_t count = 0u;

  memset(&ceiling_refusal, 0, sizeof(ceiling_refusal));
  memset(&call_refusal, 0, sizeof(call_refusal));
  bounded_limits.timeout_milliseconds = 0;
  bounded_limits.bytes = 1u;
  config.answer_limits = bounded_limits;
  CHECK(type_bridge_database_open(runtime, package, &config, NULL, &bounded,
                                     &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_database_count(bounded, limits, NULL, &count,
                                         &diagnostics) ==
        TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
  CHECK(type_bridge_execution_diagnostics_get_v1(
            diagnostics, 0u, &ceiling_refusal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_database_count(database, &bounded_limits, NULL, &count,
                                         &call_diagnostics) ==
        TYPE_BRIDGE_STATUS_RESOURCE_LIMIT);
  CHECK(type_bridge_execution_diagnostics_get_v1(
            call_diagnostics, 0u, &call_refusal) == TYPE_BRIDGE_STATUS_OK);
  CHECK(ceiling_refusal.category == call_refusal.category &&
        ceiling_refusal.code.length == call_refusal.code.length &&
        memcmp(ceiling_refusal.code.data, call_refusal.code.data,
               call_refusal.code.length) == 0 &&
        ceiling_refusal.detail_count == call_refusal.detail_count);
  CHECK(type_bridge_execution_diagnostics_close(&call_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_execution_diagnostics_close(&diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_close(&bounded, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(bounded == NULL && diagnostics == NULL);
  return 0;
}

int main(void) {
  type_bridge_status_t write_status;
  fixture_person_insert_batch_result *fixture_person_insert_result = NULL;
  type_bridge_iid_t *iid_argument = NULL;
  const char *address = getenv("TYPEDB_ADDRESS");
  const char *database_name = getenv("TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
  /* The harness selects the server from the SDK profile matrix. */
  const char *expected_server_version =
      getenv("TYPE_BRIDGE_EXPECTED_SERVER_VERSION");
  const char *http_port_text = getenv("TYPEDB_HTTP_PORT");
  char *http_end = NULL;
  unsigned long http_port = 0ul;
  type_bridge_schema_package_t *chunked_package = NULL;
  type_bridge_schema_package_t *package = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_read_transaction_t *read_transaction = NULL;
  type_bridge_write_transaction_t *write_transaction = NULL;
  fixture_person_create *person_create = NULL;
  fixture_person *person = NULL;
  uint8_t person_iid_storage[256];
  size_t person_iid_length = 0u;
  type_bridge_byte_view_t person_iid;
  type_bridge_byte_view_t borrowed;
  type_bridge_byte_view_t version;
  uint64_t count = 0u;
  type_bridge_runtime_config_v1_t runtime_config;
  type_bridge_database_config_t database_config;
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;

  CHECK(address != NULL && address[0] != '\0');
  CHECK(expected_server_version != NULL &&
        expected_server_version[0] != '\0');
  CHECK(database_name != NULL && database_name[0] != '\0');
  CHECK(username != NULL && username[0] != '\0');
  CHECK(password != NULL);
  CHECK(http_port_text != NULL && http_port_text[0] != '\0');
  http_port = strtoul(http_port_text, &http_end, 10);
  CHECK(http_end != NULL && *http_end == '\0' && http_port > 0ul &&
        http_port <= 65535ul);

  CHECK(fixture_schema_package_open(&chunked_package, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(chunked_package != NULL && diagnostics == NULL);
  CHECK(query_schema_package_open_flat(&package, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && diagnostics == NULL);
  CHECK(type_bridge_schema_package_close(&chunked_package) ==
        TYPE_BRIDGE_STATUS_OK);
  puts("ABI 1.6 flat and chunked V2 package opens: passed");

  memset(&runtime_config, 0, sizeof(runtime_config));
  runtime_config.struct_size = sizeof(runtime_config);
  runtime_config.version = TYPE_BRIDGE_RUNTIME_CONFIG_VERSION;
  runtime_config.worker_threads = TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN;
  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  memset(&database_config, 0, sizeof(database_config));
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  database_config.address = view_of(address);
  database_config.database = view_of(database_name);
  database_config.username = view_of(username);
  database_config.password = view_of(password);
  database_config.http_port = (uint32_t)http_port;
  database_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  database_config.connection_limits = limits;
  database_config.answer_limits = limits;
  CHECK(type_bridge_database_config_validate(
            &database_config, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_open(runtime, package, &database_config, NULL,
                                     &database,
                                     &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_server_version(database, &version) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(version, expected_server_version));

  CHECK(type_bridge_read_transaction_open(database, &limits, NULL,
                                             &read_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_read_transaction_count(read_transaction, &limits,
                                                  NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(type_bridge_read_transaction_close(&read_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_write_transaction_open(
            database, &limits, NULL, &write_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_write_transaction_count(
            write_transaction, &limits, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(type_bridge_write_transaction_commit(&write_transaction, &limits,
                                                NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(write_transaction == NULL && diagnostics == NULL);
  CHECK(check_answer_ceiling_binds_calls(runtime, package, database,
                                         &database_config, &limits) == 0);
  puts("ABI 1.6 database and transaction policy entries: passed");

  CHECK(open_person_create(package, "query-target", 0, &person_create,
                           &diagnostics) == 0);
  CHECK(TEST_SINGLE_WRITE(fixture_person, insert, fixture_person_database_insert(database, person_create, &limits,
                                          NULL,&fixture_person_insert_result, &diagnostics), &person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_iid(person, &borrowed, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, person_iid_storage, sizeof(person_iid_storage),
                 &person_iid_length));
  person_iid.data = person_iid_storage;
  person_iid.length = person_iid_length;
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);

  CHECK(run_counter_plan_lifecycle(package, database, &limits, &diagnostics) == 0);
  CHECK(run_empty_ordered_lifecycle(package, database, &limits, &diagnostics) == 0);
  CHECK(run_unkeyed_lifecycles(package, database, person_iid, &limits,
                               &diagnostics) == 0);
  CHECK(run_robot_batches(package, database, &limits, &diagnostics) == 0);
  CHECK(run_interaction_batches(package, database, person_iid, &limits,
                                &diagnostics) == 0);

  CHECK(fixture_person_database_delete_by_iid(database, checked_iid(person_iid, &iid_argument), &limits,
                                                 NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_database_count(database, &limits, NULL, &count,
                                         &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(fixture_person_create_close(&person_create) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_close(&database, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_runtime_close(&runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_schema_package_close(&package) == TYPE_BRIDGE_STATUS_OK);
  CHECK(diagnostics == NULL);
  puts("ABI 1.6 successor generated consumer: passed");
  CHECK(type_bridge_iid_close(&iid_argument) == TYPE_BRIDGE_STATUS_OK);
  return 0;
}
