#include <assert.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <tb_users/models.h>
#include <threads.h>
#define CHECK(call)                                                            \
  do {                                                                         \
    type_bridge_status_t s_ = (call);                                          \
    if (s_ != TYPE_BRIDGE_STATUS_OK) {                                         \
      fprintf(stderr, "%s: %d\n", #call, (int)s_);                             \
      exit(2);                                                                 \
    }                                                                          \
  } while (0)
static type_bridge_migration_cancellation_t *cancellation = NULL;
static atomic_bool callback_returned = false;
static type_bridge_byte_view_t view(const char *s) {
  return (type_bridge_byte_view_t){(const uint8_t *)s, strlen(s)};
}
typedef struct context {
  type_bridge_schema_package_t *package;
  type_bridge_iid_t *ids[2];
  int fail;
  type_bridge_write_transaction_t *escaped;
} context;
static type_bridge_status_t
derive(type_bridge_write_transaction_t *tx, void *data,
       type_bridge_execution_diagnostics_t **errors) {
  context *ctx = data;
  ctx->escaped = tx;
  type_bridge_write_transaction_t *borrowed = tx;
  assert(type_bridge_write_transaction_commit(&borrowed, NULL, NULL, errors) !=
         TYPE_BRIDGE_STATUS_OK);
  assert(borrowed == tx);
  CHECK(type_bridge_execution_diagnostics_close(errors));
  for (size_t i = 0; i < 2; i++) {
    tb_users_person *person = NULL;
    tb_users_ssn *ssn = NULL;
    tb_users_gender *gender = NULL;
    tb_users_person_create *create = NULL;
    tb_users_person_update_batch_result *result = NULL;
    type_bridge_byte_view_t text = {0};
    CHECK(tb_users_person_write_transaction_get_by_iid(tx, ctx->ids[i], NULL,
                                                       NULL, &person, errors));
    CHECK(tb_users_person_ssn(person, &ssn, errors));
    CHECK(tb_users_ssn_value(ssn, &text, errors));
    CHECK(tb_users_gender_open(
        ctx->package, view(text.data[text.length - 1] % 2 ? "male" : "female"),
        &gender, errors));
    tb_users_person_create_args_v1_t args = {0};
    args.struct_size = sizeof(args);
    args.version = 1;
    args.field_ssn = ssn;
    args.field_gender = gender;
    CHECK(tb_users_person_create_open(ctx->package, &args, &create, errors));
    CHECK(tb_users_person_write_transaction_update(
        tx, ctx->ids[i], create, NULL, NULL, &result, errors));
    CHECK(tb_users_person_update_batch_result_close(&result));
    CHECK(tb_users_person_create_close(&create));
    CHECK(tb_users_gender_close(&gender));
    CHECK(tb_users_ssn_close(&ssn));
    CHECK(tb_users_person_close(&person));
  }
  if (ctx->fail)
    return TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  if (cancellation) {
    CHECK(type_bridge_migration_cancellation_cancel(cancellation));
    thrd_sleep(&(struct timespec){.tv_nsec = 100000000}, NULL);
    atomic_store(&callback_returned, true);
    return TYPE_BRIDGE_STATUS_OK;
  }
  type_bridge_raw_result_t *raw = NULL;
  type_bridge_diagnostics_t *raw_errors = NULL;
  CHECK(type_bridge_migration_transaction_execute_query(
      tx,
      view("match $p isa person; fetch {\"ssn\":$p.ssn,\"gender\":$p.gender};"),
      &raw, &raw_errors));
  CHECK(type_bridge_raw_result_close(&raw));
  return TYPE_BRIDGE_STATUS_OK;
}
static type_bridge_status_t
reverse(type_bridge_write_transaction_t *tx, void *data,
        type_bridge_execution_diagnostics_t **errors) {
  (void)data;
  (void)errors;
  type_bridge_raw_result_t *raw = NULL;
  type_bridge_diagnostics_t *raw_errors = NULL;
  CHECK(type_bridge_migration_transaction_execute_query(
      tx, view("match $p isa person, has gender $g; delete has $g of $p;"),
      &raw, &raw_errors));
  CHECK(type_bridge_raw_result_close(&raw));
  return TYPE_BRIDGE_STATUS_OK;
}
static void assert_no_gender(type_bridge_database_t *db, context *ctx,
                             type_bridge_execution_diagnostics_t **errors) {
  for (size_t i = 0; i < 2; i++) {
    tb_users_person *person = NULL;
    tb_users_gender *gender = NULL;
    CHECK(tb_users_person_database_get_by_iid(db, ctx->ids[i], NULL, NULL,
                                              &person, errors));
    CHECK(tb_users_person_gender(person, &gender, errors));
    assert(gender == NULL);
    CHECK(tb_users_person_close(&person));
  }
}
static type_bridge_migration_plan_t *
plan(type_bridge_migration_catalog_t *catalog,
     type_bridge_migration_identity_t **ids, int data, int rollback) {
  type_bridge_migration_plan_t *preview = NULL, *result = NULL;
  type_bridge_diagnostics_t *errors = NULL;
  const type_bridge_migration_identity_t *target = ids[data];
  if (rollback)
    CHECK(type_bridge_migration_catalog_preview_rollback(
        catalog, (const type_bridge_migration_identity_t *const *)ids, 2,
        &target, 1, &preview, &errors));
  else
    CHECK(type_bridge_migration_catalog_preview_apply(
        catalog,
        data ? (const type_bridge_migration_identity_t *const *)ids : NULL,
        (size_t)data, &target, 1, 0, &preview, &errors));
  type_bridge_migration_approval_builder_t *builder = NULL;
  type_bridge_migration_approval_set_t *approvals = NULL;
  CHECK(type_bridge_migration_plan_approval_builder(preview, &builder));
  if (data)
    CHECK(type_bridge_migration_approval_builder_approve(builder, 0, &errors));
  CHECK(type_bridge_migration_approval_builder_finish(builder, &approvals,
                                                      &errors));
  CHECK(type_bridge_migration_plan_authorize(preview, approvals, &result,
                                             &errors));
  CHECK(type_bridge_migration_approval_set_close(&approvals));
  CHECK(type_bridge_migration_approval_builder_close(&builder));
  CHECK(type_bridge_migration_plan_close(&preview));
  return result;
}
static type_bridge_status_t execute(type_bridge_migration_plan_t *plan,
                                    type_bridge_database_t *db) {
  type_bridge_migration_execution_outcome_t *out = NULL;
  type_bridge_diagnostics_t *errors = NULL;
  type_bridge_migration_execution_options_v1_t options = {
      sizeof(options),
      TYPE_BRIDGE_MIGRATION_EXECUTION_OPTIONS_V1,
      0,
      0,
      UINT64_MAX,
      UINT64_MAX,
      cancellation};
  type_bridge_status_t status = type_bridge_migration_plan_execute_with_options(
      plan, db, view("c-callback"), &options, &out, &errors);
  CHECK(type_bridge_migration_execution_outcome_close(&out));
  CHECK(type_bridge_diagnostics_close(&errors));
  return status;
}
int main(int argc, char **argv) {
  assert(argc == 2);
  FILE *file = fopen(argv[1], "rb");
  assert(file);
  assert(!fseek(file, 0, SEEK_END));
  long length = ftell(file);
  rewind(file);
  char *bundle = malloc((size_t)length);
  assert(bundle);
  assert(fread(bundle, 1, (size_t)length, file) == (size_t)length);
  fclose(file);
  type_bridge_runtime_config_v1_t rc = {
      sizeof(rc), TYPE_BRIDGE_RUNTIME_CONFIG_VERSION, 2, 0, {0}};
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_database_t *db = NULL;
  context ctx = {0};
  ctx.fail = 1;
  type_bridge_execution_diagnostics_t *errors = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  CHECK(type_bridge_runtime_open_v1(&rc, &runtime, &errors));
  CHECK(tb_users_schema_package_open(&ctx.package, &errors));
  type_bridge_database_config_t dc = {0};
  dc.struct_size = sizeof(dc);
  dc.version = TYPE_BRIDGE_DATABASE_CONFIG_VERSION;
  dc.address = view(getenv("TYPEDB_ADDRESS"));
  dc.database = view(getenv("TYPE_BRIDGE_CALLBACK_DATABASE"));
  dc.username =
      view(getenv("TYPEDB_USERNAME") ? getenv("TYPEDB_USERNAME") : "admin");
  dc.password =
      view(getenv("TYPEDB_PASSWORD") ? getenv("TYPEDB_PASSWORD") : "password");
  dc.http_port = (uint32_t)strtoul(getenv("TYPEDB_HTTP_PORT"), NULL, 10);
  dc.connection_limits = (type_bridge_query_execution_limits_v1_t)
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  dc.answer_limits = dc.connection_limits;
  CHECK(
      type_bridge_database_open(runtime, ctx.package, &dc, NULL, &db, &errors));
  type_bridge_database_administration_t *admin = NULL;
  uint32_t outcome = 0;
  CHECK(type_bridge_database_administration_open(db, &admin, &diagnostics));
  CHECK(type_bridge_database_administration_create(admin, &outcome,
                                                   &diagnostics));
  type_bridge_migration_catalog_t *catalog = NULL;
  CHECK(type_bridge_migration_catalog_open(
      ctx.package, (type_bridge_byte_view_t){(uint8_t *)bundle, (size_t)length},
      &catalog, &errors));
  free(bundle);
  type_bridge_migration_identity_t *ids[2] = {0};
  for (size_t i = 0; i < 2; i++) {
    type_bridge_migration_history_entry_t *entry = NULL;
    CHECK(type_bridge_migration_catalog_entry_at(catalog, i, &entry));
    CHECK(type_bridge_migration_history_entry_identity(entry, &ids[i]));
    CHECK(type_bridge_migration_history_entry_close(&entry));
  }
  type_bridge_migration_plan_t *p = plan(catalog, ids, 0, 0);
  CHECK(execute(p, db));
  CHECK(type_bridge_migration_plan_close(&p));
  for (size_t i = 0; i < 2; i++) {
    tb_users_ssn *ssn = NULL;
    tb_users_person_create *create = NULL;
    tb_users_person_insert_batch_result *inserted = NULL;
    tb_users_person *person = NULL;
    type_bridge_byte_view_t iid = {0};
    CHECK(tb_users_ssn_open(ctx.package, view(i ? "1232" : "1231"), &ssn,
                            &errors));
    tb_users_person_create_args_v1_t args = {0};
    args.struct_size = sizeof(args);
    args.version = 1;
    args.field_ssn = ssn;
    CHECK(tb_users_person_create_open(ctx.package, &args, &create, &errors));
    CHECK(tb_users_person_database_insert(db, create, NULL, NULL, &inserted,
                                          &errors));
    CHECK(tb_users_person_insert_batch_result_item(inserted, &person, &errors));
    CHECK(tb_users_person_iid(person, &iid, &errors));
    CHECK(type_bridge_iid_open(iid, &ctx.ids[i]));
    CHECK(tb_users_person_close(&person));
    CHECK(tb_users_person_insert_batch_result_close(&inserted));
    CHECK(tb_users_person_create_close(&create));
    CHECK(tb_users_ssn_close(&ssn));
  }
  CHECK(type_bridge_migration_catalog_register(catalog, view("derive-gender"),
                                               derive, reverse, &ctx, &errors));
  p = plan(catalog, ids, 1, 0);
  assert(execute(p, db) != TYPE_BRIDGE_STATUS_OK);
  assert_no_gender(db, &ctx, &errors);
  ctx.fail = 0;
  CHECK(type_bridge_migration_cancellation_new(&cancellation));
  assert(execute(p, db) != TYPE_BRIDGE_STATUS_OK);
  assert(atomic_load(&callback_returned));
  assert_no_gender(db, &ctx, &errors);
  CHECK(type_bridge_migration_cancellation_close(&cancellation));
  CHECK(execute(p, db));
  CHECK(type_bridge_migration_plan_close(&p));
  type_bridge_raw_result_t *raw = NULL;
  assert(type_bridge_migration_transaction_execute_query(
             ctx.escaped, view("match $p isa person; fetch {\"ssn\":$p.ssn};"),
             &raw, &diagnostics) != TYPE_BRIDGE_STATUS_OK);
  CHECK(type_bridge_diagnostics_close(&diagnostics));
  p = plan(catalog, ids, 1, 1);
  CHECK(execute(p, db));
  CHECK(type_bridge_migration_plan_close(&p));
  p = plan(catalog, ids, 1, 0);
  CHECK(execute(p, db));
  CHECK(type_bridge_migration_plan_close(&p));
  for (size_t i = 0; i < 2; i++) {
    CHECK(type_bridge_iid_close(&ctx.ids[i]));
    CHECK(type_bridge_migration_identity_close(&ids[i]));
  }
  CHECK(type_bridge_migration_catalog_close(&catalog));
  CHECK(type_bridge_database_administration_close(&admin));
  CHECK(type_bridge_database_close(&db, &errors));
  CHECK(type_bridge_schema_package_close(&ctx.package));
  CHECK(type_bridge_runtime_close(&runtime, &errors));
  puts("PASS C generated managers + raw + lifecycle refusal + failure retry + "
       "reverse + escaped handle + owner cleanup");
  return 0;
}
