#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <fixture/models.h>

#ifdef __cplusplus
extern "C" {
#endif
type_bridge_status_t TYPE_BRIDGE_CALL phase4_schema_package_open_flat_v2(
    type_bridge_schema_package_t **out_package,
    type_bridge_execution_diagnostics_t **out_diagnostics);
#ifdef __cplusplus
}
#endif

#define CHECK(condition)                                                       \
  do {                                                                         \
    if (!(condition)) {                                                        \
      fprintf(stderr, "ABI 1.4 successor check failed at line %d\n",           \
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
                   fixture_person_create **out_create,
                   type_bridge_execution_diagnostics_t **out_diagnostics) {
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
  args.struct_size = sizeof(args);
  args.version = FIXTURE_CREATE_ARGS_VERSION;
  /* aliases is intentionally empty: TypeDB 3.12.1 cannot provide ordered
   * list-instance evidence, so this lane exercises no ordered values. */
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

static int
run_unkeyed_lifecycles(const type_bridge_schema_package_t *package,
                       const type_bridge_database_t *database,
                       type_bridge_byte_view_t person_iid,
                       const type_bridge_query_execution_limits_v1_t *limits,
                       type_bridge_execution_diagnostics_t **out_diagnostics) {
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
  CHECK(fixture_counter_database_insert_v2(database, counter_create, limits,
                                           NULL, &counter, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_iid(counter, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, iid_storage, sizeof(iid_storage), &iid_length));
  iid.data = iid_storage;
  iid.length = iid_length;
  CHECK(fixture_counter_close(&counter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_database_get_by_iid_v2(database, iid, limits, NULL,
                                               &counter, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_close(&counter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(open_counter_create(package, 11, &counter_update, out_diagnostics) ==
        0);
  CHECK(fixture_counter_database_update_v2(
            database, iid, counter_update, limits, NULL, &counter,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_close(&counter) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_database_delete_by_iid_v2(database, iid, limits, NULL,
                                                  out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_database_count_v2(database, limits, NULL, &count,
                                          out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(fixture_counter_create_close(&counter_update) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_create_close(&counter_create) == TYPE_BRIDGE_STATUS_OK);

  CHECK(open_membership_create(package, person_iid, &membership_create,
                               out_diagnostics) == 0);
  CHECK(fixture_membership_database_insert_v2(
            database, membership_create, limits, NULL, &membership,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_iid(membership, &borrowed, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, iid_storage, sizeof(iid_storage), &iid_length));
  iid.data = iid_storage;
  iid.length = iid_length;
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_get_by_iid_v2(
            database, iid, limits, NULL, &membership, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_update_v2(
            database, iid, membership_create, limits, NULL, &membership,
            out_diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_close(&membership) == TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_delete_by_iid_v2(database, iid, limits,
                                                     NULL, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_membership_database_count_v2(database, limits, NULL, &count,
                                             out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(fixture_membership_create_close(&membership_create) ==
        TYPE_BRIDGE_STATUS_OK);
  puts("ABI 1.4 unkeyed entity/relation IID lifecycle: passed");
  return 0;
}

static int
run_robot_batches(const type_bridge_schema_package_t *package,
                  const type_bridge_database_t *database,
                  const type_bridge_query_execution_limits_v1_t *limits,
                  type_bridge_execution_diagnostics_t **out_diagnostics) {
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
  CHECK(fixture_robot_update_batch_builder_add(update_builder, insert_iid,
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
  CHECK(fixture_robot_delete_batch_builder_add(delete_builder, insert_iid,
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
  CHECK(fixture_robot_database_count_v2(database, limits, NULL, &count,
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
  CHECK(fixture_robot_database_count_v2(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);

  /* Repeat the later-row collision through a borrowed V2 write owner. The
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
    CHECK(type_bridge_write_transaction_open_v2(
              database, limits, NULL, &write_transaction, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(fixture_robot_write_transaction_insert_batch_execute(
              write_transaction, insert_batch, limits, NULL, &insert_result,
              out_diagnostics) == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
    CHECK(insert_result == NULL && *out_diagnostics != NULL);
    CHECK(check_exact_provider_write_failure(*out_diagnostics) == 0);
    CHECK(type_bridge_execution_diagnostics_close(out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(type_bridge_write_transaction_commit_v2(&write_transaction, limits,
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
  CHECK(fixture_robot_database_count_v2(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 1u);
  CHECK(fixture_robot_database_delete_by_iid_v2(database, put_iid, limits, NULL,
                                                out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_robot_database_count_v2(database, limits, NULL, &count,
                                        out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  puts("ABI 1.4 entity insert/put/update/delete batches: passed");
  puts("ABI 1.4 later-row failure has zero committed prefix: passed");
  puts("ABI 1.4 borrowed failure is rollback-only before commit: passed");
  return 0;
}

static int
run_interaction_batches(const type_bridge_schema_package_t *package,
                        const type_bridge_database_t *database,
                        type_bridge_byte_view_t person_iid,
                        const type_bridge_query_execution_limits_v1_t *limits,
                        type_bridge_execution_diagnostics_t **out_diagnostics) {
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

  CHECK(open_interaction_create(package, "phase4-interaction-insert",
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

  CHECK(open_interaction_create(package, "phase4-interaction-put", person_iid,
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

  CHECK(open_interaction_create(package, "phase4-interaction-insert",
                                person_iid, &create, out_diagnostics) == 0);
  CHECK(fixture_interaction_update_batch_builder_open(
            package, limits, NULL, &update_builder, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_update_batch_builder_add(update_builder, insert_iid,
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
  CHECK(fixture_interaction_delete_batch_builder_add(delete_builder, insert_iid,
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
  CHECK(fixture_interaction_database_delete_by_iid_v2(database, put_iid, limits,
                                                      NULL, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_interaction_database_count_v2(database, limits, NULL, &count,
                                              out_diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  puts("ABI 1.4 relation insert/put/update/delete batches: passed");
  return 0;
}

int main(void) {
  const char *address = getenv("TYPEDB_ADDRESS");
  const char *database_name = getenv("TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
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
  type_bridge_database_config_v2_t database_config;
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;

  CHECK(address != NULL && address[0] != '\0');
  CHECK(database_name != NULL && database_name[0] != '\0');
  CHECK(username != NULL && username[0] != '\0');
  CHECK(password != NULL);
  CHECK(http_port_text != NULL && http_port_text[0] != '\0');
  http_port = strtoul(http_port_text, &http_end, 10);
  CHECK(http_end != NULL && *http_end == '\0' && http_port > 0ul &&
        http_port <= 65535ul);

  CHECK(fixture_schema_package_open_v2(&chunked_package, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(chunked_package != NULL && diagnostics == NULL);
  CHECK(phase4_schema_package_open_flat_v2(&package, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(package != NULL && diagnostics == NULL);
  CHECK(type_bridge_schema_package_close(&chunked_package) ==
        TYPE_BRIDGE_STATUS_OK);
  puts("ABI 1.4 flat and chunked V2 package opens: passed");

  memset(&runtime_config, 0, sizeof(runtime_config));
  runtime_config.struct_size = sizeof(runtime_config);
  runtime_config.version = TYPE_BRIDGE_RUNTIME_CONFIG_VERSION;
  runtime_config.worker_threads = TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN;
  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);

  memset(&database_config, 0, sizeof(database_config));
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION;
  database_config.address = view_of(address);
  database_config.database = view_of(database_name);
  database_config.username = view_of(username);
  database_config.password = view_of(password);
  database_config.http_port = (uint32_t)http_port;
  database_config.tls_mode = TYPE_BRIDGE_TLS_DISABLED;
  database_config.connection_limits = limits;
  database_config.answer_limits = limits;
  CHECK(type_bridge_database_config_validate_v2(
            &database_config, &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_open_v2(runtime, package, &database_config, NULL,
                                     &database,
                                     &diagnostics) == TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_database_server_version(database, &version) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(version, "3.12.1"));

  CHECK(type_bridge_read_transaction_open_v2(database, &limits, NULL,
                                             &read_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_read_transaction_count_v2(read_transaction, &limits,
                                                  NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(type_bridge_read_transaction_close(&read_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_write_transaction_open_v2(
            database, &limits, NULL, &write_transaction, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_counter_write_transaction_count_v2(
            write_transaction, &limits, NULL, &count, &diagnostics) ==
            TYPE_BRIDGE_STATUS_OK &&
        count == 0u);
  CHECK(type_bridge_write_transaction_commit_v2(&write_transaction, &limits,
                                                NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(write_transaction == NULL && diagnostics == NULL);
  puts("ABI 1.4 database and transaction policy entries: passed");

  CHECK(open_person_create(package, "phase4-target", &person_create,
                           &diagnostics) == 0);
  CHECK(fixture_person_database_insert_v2(database, person_create, &limits,
                                          NULL, &person, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_iid(person, &borrowed, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_iid(borrowed, person_iid_storage, sizeof(person_iid_storage),
                 &person_iid_length));
  person_iid.data = person_iid_storage;
  person_iid.length = person_iid_length;
  CHECK(fixture_person_close(&person) == TYPE_BRIDGE_STATUS_OK);

  CHECK(run_unkeyed_lifecycles(package, database, person_iid, &limits,
                               &diagnostics) == 0);
  CHECK(run_robot_batches(package, database, &limits, &diagnostics) == 0);
  CHECK(run_interaction_batches(package, database, person_iid, &limits,
                                &diagnostics) == 0);

  CHECK(fixture_person_database_delete_by_iid_v2(database, person_iid, &limits,
                                                 NULL, &diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(fixture_person_database_count_v2(database, &limits, NULL, &count,
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
  puts("ABI 1.4 successor generated consumer: passed");
  return 0;
}
