#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <typebridge/type_bridge_abi_1_5.h>
#include <tb_workforcev4/models.h>

static type_bridge_byte_view_t view(const char *value) {
  type_bridge_byte_view_t result = {(const uint8_t *)value, strlen(value)};
  return result;
}

static void fail(const char *operation, type_bridge_status_t status) {
  fprintf(stderr, "%s failed with status %d\n", operation, (int)status);
  exit(2);
}

#define CHECK(operation) do { type_bridge_status_t status_ = (operation); if (status_ != TYPE_BRIDGE_STATUS_OK) fail(#operation, status_); } while (0)

static char *read_file(const char *path, size_t *length) {
  FILE *file = fopen(path, "rb");
  long size;
  char *bytes;
  if (file == NULL || fseek(file, 0, SEEK_END) != 0 || (size = ftell(file)) < 0 || fseek(file, 0, SEEK_SET) != 0) {
    fprintf(stderr, "history bundle could not be opened\n");
    exit(2);
  }
  bytes = (char *)malloc((size_t)size);
  if (bytes == NULL || fread(bytes, 1u, (size_t)size, file) != (size_t)size) {
    fprintf(stderr, "history bundle could not be read\n");
    exit(2);
  }
  fclose(file);
  *length = (size_t)size;
  return bytes;
}

static void diagnostic_code(type_bridge_diagnostics_t **diagnostics, char *output, size_t capacity) {
  type_bridge_byte_view_t json = {0};
  const char *needle = "\"code\":\"";
  const char *start;
  const char *end;
  size_t length;
  if (*diagnostics == NULL) {
    snprintf(output, capacity, "missing_diagnostic");
    return;
  }
  CHECK(type_bridge_diagnostics_json(*diagnostics, &json));
  start = strstr((const char *)json.data, needle);
  if (start == NULL) {
    snprintf(output, capacity, "missing_diagnostic_code");
  } else {
    start += strlen(needle);
    end = strchr(start, '"');
    length = end == NULL ? 0u : (size_t)(end - start);
    if (length >= capacity) length = capacity - 1u;
    memcpy(output, start, length);
    output[length] = '\0';
  }
  CHECK(type_bridge_diagnostics_close(diagnostics));
}

static void identity_text(const type_bridge_migration_identity_t *identity, char *output, size_t capacity) {
  type_bridge_byte_view_t app = {0}, name = {0};
  CHECK(type_bridge_migration_identity_app_label(identity, &app));
  CHECK(type_bridge_migration_identity_name(identity, &name));
  snprintf(output, capacity, "%.*s/%.*s", (int)app.length, app.data, (int)name.length, name.data);
}

static type_bridge_migration_plan_t *preview_apply(
    const type_bridge_migration_catalog_t *catalog,
    const type_bridge_migration_identity_t *const *applied, size_t applied_count,
    const type_bridge_migration_identity_t *target) {
  type_bridge_migration_plan_t *plan = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  const type_bridge_migration_identity_t *targets[1] = {target};
  CHECK(type_bridge_migration_catalog_preview_apply(
      catalog, applied, applied_count, targets, 1u, 0u, &plan, &diagnostics));
  return plan;
}

static type_bridge_migration_plan_t *authorize(
    type_bridge_migration_plan_t *preview, int approve_first, char *error_code) {
  type_bridge_migration_approval_builder_t *builder = NULL;
  type_bridge_migration_approval_set_t *approvals = NULL;
  type_bridge_migration_plan_t *plan = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  type_bridge_status_t status;
  CHECK(type_bridge_migration_plan_approval_builder(preview, &builder));
  if (approve_first) CHECK(type_bridge_migration_approval_builder_approve(builder, 0u, &diagnostics));
  CHECK(type_bridge_migration_approval_builder_finish(builder, &approvals, &diagnostics));
  status = type_bridge_migration_plan_authorize(preview, approvals, &plan, &diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK) {
    diagnostic_code(&diagnostics, error_code, 96u);
  }
  CHECK(type_bridge_migration_approval_set_close(&approvals));
  CHECK(type_bridge_migration_approval_builder_close(&builder));
  return plan;
}

static type_bridge_migration_execution_outcome_t *execute_plan(
    const type_bridge_migration_plan_t *plan, const type_bridge_database_t *database,
    const char *holder, type_bridge_diagnostics_t **diagnostics) {
  type_bridge_migration_execution_outcome_t *outcome = NULL;
  type_bridge_status_t status = type_bridge_migration_plan_execute(
      plan, database, view(holder), &outcome, diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK) return NULL;
  return outcome;
}

static const char *status_name(uint32_t status) {
  if (status == (uint32_t)TYPE_BRIDGE_MIGRATION_EXECUTION_APPLIED) return "applied";
  if (status == (uint32_t)TYPE_BRIDGE_MIGRATION_EXECUTION_ROLLED_BACK) return "rolled_back";
  if (status == (uint32_t)TYPE_BRIDGE_MIGRATION_EXECUTION_RETRY_SAFE) return "retry_safe";
  return "requires_explicit_recovery";
}

int main(void) {
  const char *address = getenv("TYPEDB_ADDRESS");
  const char *database_name = getenv("TYPE_BRIDGE_WORKFORCE_V4_DATABASE");
  const char *username = getenv("TYPEDB_USERNAME");
  const char *password = getenv("TYPEDB_PASSWORD");
  const char *http_port_text = getenv("TYPEDB_HTTP_PORT");
  type_bridge_runtime_config_v1_t runtime_config = {sizeof(runtime_config), TYPE_BRIDGE_RUNTIME_CONFIG_VERSION, 2u, 0u, {0u}};
  type_bridge_database_config_v2_t database_config = {0};
  type_bridge_runtime_t *runtime = NULL;
  type_bridge_schema_package_t *package = NULL;
  type_bridge_database_t *database = NULL;
  type_bridge_database_administration_t *administration = NULL;
  type_bridge_execution_diagnostics_t *execution_diagnostics = NULL;
  type_bridge_diagnostics_t *diagnostics = NULL;
  type_bridge_migration_catalog_t *catalog = NULL;
  type_bridge_migration_history_entry_t *entries[4] = {0};
  type_bridge_migration_identity_t *ids[4] = {0};
  size_t bundle_length = 0u, catalog_count = 0u, index;
  char *bundle;
  type_bridge_byte_view_t fingerprint = {0};
  char fingerprint_digest[65] = {0};
  uint32_t create = 0u, repeat_create = 0u, pair_state = 0u;
  type_bridge_migration_cancellation_t *cancellation = NULL;
  type_bridge_migration_execution_options_v1_t options = {0};
  uint8_t exists = 0u;
  char cancellation_code[96] = {0}, resource_code[96] = {0}, conflict_code[96] = {0}, approval_code[96] = {0}, unknown_code[96] = {0};
  char apply_order[4][96] = {{0}}, rollback_order[4][96] = {{0}};
  size_t backfill_steps = 0u;
  type_bridge_migration_plan_t *full_apply = NULL, *full_rollback = NULL;
  type_bridge_migration_plan_t *preview = NULL, *plan = NULL;
  type_bridge_migration_plan_t *reapply_preview = NULL, *reapply_plan = NULL;
  type_bridge_migration_execution_outcome_t *initial_outcome = NULL, *forward_outcome = NULL, *rollback_outcome = NULL, *reapply_outcome = NULL;
  uint32_t initial_status = 0u, rollback_status = 0u, reapply_status = 0u;
  type_bridge_migration_backfill_observation_t *forward = NULL, *reverse = NULL;
  uint64_t matched = 0u, forward_changed = 0u, skipped = 0u, reverse_changed = 0u;
  uint32_t forward_groups = 0u;
  unsigned long conflict_count = 0u, equal_count = 0u, remaining_count = 0u;
  type_bridge_database_deletion_plan_t *deletion_plan = NULL;
  uint32_t deletion = 0u, repeat_deletion = 0u, final_state = 0u;

  if (address == NULL) address = "127.0.0.1:1729";
  if (username == NULL) username = "admin";
  if (password == NULL) password = "password";
  if (http_port_text == NULL) http_port_text = "8000";
  if (database_name == NULL) fail("database environment", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);

  CHECK(type_bridge_runtime_open_v1(&runtime_config, &runtime, &execution_diagnostics));
  CHECK(tb_workforcev4_schema_package_open(&package, &diagnostics));
  database_config.struct_size = sizeof(database_config);
  database_config.version = TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION;
  database_config.address = view(address);
  database_config.database = view(database_name);
  database_config.username = view(username);
  database_config.password = view(password);
  database_config.http_port = (uint32_t)strtoul(http_port_text, NULL, 10);
  database_config.connection_limits = (type_bridge_query_execution_limits_v1_t)TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  database_config.answer_limits = (type_bridge_query_execution_limits_v1_t)TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  CHECK(type_bridge_database_open_v2(runtime, package, &database_config, NULL, &database, &execution_diagnostics));
  CHECK(type_bridge_database_administration_open(database, &administration, &diagnostics));
  CHECK(type_bridge_database_administration_inspect(administration, &pair_state, &diagnostics));
  if (pair_state != (uint32_t)TYPE_BRIDGE_DATABASE_PAIR_ABSENT) fail("initial pair", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_database_administration_create(administration, &create, &diagnostics));
  CHECK(type_bridge_database_administration_create(administration, &repeat_create, &diagnostics));
  CHECK(type_bridge_database_administration_inspect(administration, &pair_state, &diagnostics));

  CHECK(type_bridge_migration_cancellation_new(&cancellation));
  CHECK(type_bridge_migration_cancellation_cancel(cancellation));
  options.struct_size = sizeof(options);
  options.version = TYPE_BRIDGE_MIGRATION_EXECUTION_OPTIONS_V1;
  options.max_transaction_groups = UINT64_MAX;
  options.max_backfill_observations = UINT64_MAX;
  options.cancellation = cancellation;
  if (type_bridge_database_administration_exists_with_options(administration, &options, &exists, &diagnostics) == TYPE_BRIDGE_STATUS_OK) fail("cancelled administration", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  diagnostic_code(&diagnostics, cancellation_code, sizeof(cancellation_code));
  CHECK(type_bridge_migration_cancellation_close(&cancellation));

  bundle = read_file("typebridge/migration-history.json", &bundle_length);
  CHECK(type_bridge_migration_catalog_open(package, (type_bridge_byte_view_t){(const uint8_t *)bundle, bundle_length}, &catalog, &diagnostics));
  CHECK(type_bridge_migration_catalog_count(catalog, &catalog_count));
  if (catalog_count != 4u) fail("catalog count", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_migration_catalog_fingerprint(catalog, &fingerprint));
  {
    const char *digest = strstr((const char *)fingerprint.data, "\"digest\":\"");
    if (digest == NULL) fail("catalog fingerprint", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    digest += strlen("\"digest\":\"");
    memcpy(fingerprint_digest, digest, 64u);
  }
  for (index = 0u; index < 4u; ++index) {
    CHECK(type_bridge_migration_catalog_entry_at(catalog, index, &entries[index]));
    CHECK(type_bridge_migration_history_entry_identity(entries[index], &ids[index]));
  }
  CHECK(type_bridge_migration_catalog_preview_apply(catalog, NULL, 0u, NULL, 0u, 1u, &full_apply, &diagnostics));
  CHECK(type_bridge_migration_plan_count(full_apply, &catalog_count));
  for (index = 0u; index < catalog_count; ++index) {
    type_bridge_migration_plan_entry_t *entry = NULL;
    type_bridge_migration_identity_t *identity = NULL;
    size_t count = 0u;
    CHECK(type_bridge_migration_plan_entry_at(full_apply, index, &entry));
    CHECK(type_bridge_migration_plan_entry_identity(entry, &identity));
    identity_text(identity, apply_order[index], sizeof(apply_order[index]));
    CHECK(type_bridge_migration_plan_entry_backfill_count(entry, &count));
    backfill_steps += count;
    CHECK(type_bridge_migration_identity_close(&identity));
    CHECK(type_bridge_migration_plan_entry_close(&entry));
  }
  CHECK(type_bridge_migration_catalog_preview_rollback(catalog, (const type_bridge_migration_identity_t *const *)ids, 4u, (const type_bridge_migration_identity_t *const *)ids, 4u, &full_rollback, &diagnostics));
  CHECK(type_bridge_migration_plan_count(full_rollback, &catalog_count));
  for (index = 0u; index < catalog_count; ++index) {
    type_bridge_migration_plan_entry_t *entry = NULL;
    type_bridge_migration_identity_t *identity = NULL;
    CHECK(type_bridge_migration_plan_entry_at(full_rollback, index, &entry));
    CHECK(type_bridge_migration_plan_entry_identity(entry, &identity));
    identity_text(identity, rollback_order[index], sizeof(rollback_order[index]));
    CHECK(type_bridge_migration_identity_close(&identity));
    CHECK(type_bridge_migration_plan_entry_close(&entry));
  }

  preview = preview_apply(catalog, NULL, 0u, ids[0]);
  plan = authorize(preview, 0, approval_code);
  options.cancellation = NULL;
  options.max_transaction_groups = 0u;
  options.max_backfill_observations = 0u;
  if (type_bridge_migration_plan_execute_with_options(plan, database, view("workforce-v4-c-limit"), &options, &initial_outcome, &diagnostics) == TYPE_BRIDGE_STATUS_OK) fail("zero group limit", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  diagnostic_code(&diagnostics, resource_code, sizeof(resource_code));
  CHECK(type_bridge_migration_plan_close(&plan));
  CHECK(type_bridge_migration_plan_close(&preview));

  preview = preview_apply(catalog, NULL, 0u, ids[0]);
  plan = authorize(preview, 0, approval_code);
  initial_outcome = execute_plan(plan, database, "workforce-v4-c-initial", &diagnostics);
  if (initial_outcome == NULL) fail("initial migration", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_migration_execution_outcome_status(initial_outcome, &initial_status));
  CHECK(type_bridge_migration_plan_close(&plan)); CHECK(type_bridge_migration_plan_close(&preview));
  { const type_bridge_migration_identity_t *applied[1] = {ids[0]}; preview = preview_apply(catalog, applied, 1u, ids[1]); }
  plan = authorize(preview, 0, approval_code);
  { type_bridge_migration_execution_outcome_t *outcome = execute_plan(plan, database, "workforce-v4-c-expand", &diagnostics); if (outcome == NULL) fail("expand migration", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT); CHECK(type_bridge_migration_execution_outcome_close(&outcome)); }
  CHECK(type_bridge_migration_plan_close(&plan)); CHECK(type_bridge_migration_plan_close(&preview));

  puts("TYPE_BRIDGE_C_FIXTURE seed"); fflush(stdout);
  if (scanf("%lu", &conflict_count) != 1) fail("seed acknowledgement", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  { const type_bridge_migration_identity_t *applied[2] = {ids[0], ids[1]}; preview = preview_apply(catalog, applied, 2u, ids[2]); }
  plan = authorize(preview, 1, approval_code);
  if (execute_plan(plan, database, "workforce-v4-c-backfill-conflict", &diagnostics) != NULL) fail("conflicting backfill", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  diagnostic_code(&diagnostics, conflict_code, sizeof(conflict_code));
  puts("TYPE_BRIDGE_C_FIXTURE repair"); fflush(stdout);
  if (scanf("%lu", &conflict_count) != 1) fail("repair acknowledgement", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  forward_outcome = execute_plan(plan, database, "workforce-v4-c-backfill-forward", &diagnostics);
  if (forward_outcome == NULL) fail("forward backfill", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_migration_execution_outcome_backfill_at(forward_outcome, 0u, &forward));
  CHECK(type_bridge_migration_backfill_observation_counts(forward, &matched, &forward_changed, &skipped, &forward_groups));
  puts("TYPE_BRIDGE_C_FIXTURE equal"); fflush(stdout);
  if (scanf("%lu", &equal_count) != 1) fail("equal acknowledgement", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_migration_plan_close(&plan));
  CHECK(type_bridge_migration_plan_close(&preview));

  { const type_bridge_migration_identity_t *applied[3] = {ids[0], ids[1], ids[2]}; const type_bridge_migration_identity_t *remove[1] = {ids[2]}; CHECK(type_bridge_migration_catalog_preview_rollback(catalog, applied, 3u, remove, 1u, &preview, &diagnostics)); }
  if (authorize(preview, 0, approval_code) != NULL) fail("unapproved rollback", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  plan = authorize(preview, 1, approval_code);
  rollback_outcome = execute_plan(plan, database, "workforce-v4-c-backfill-reverse", &diagnostics);
  if (rollback_outcome == NULL) fail("reverse backfill", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_migration_execution_outcome_status(rollback_outcome, &rollback_status));
  CHECK(type_bridge_migration_execution_outcome_backfill_at(rollback_outcome, 0u, &reverse));
  CHECK(type_bridge_migration_backfill_observation_counts(reverse, &matched, &reverse_changed, &skipped, &forward_groups));
  puts("TYPE_BRIDGE_C_FIXTURE remaining"); fflush(stdout);
  if (scanf("%lu", &remaining_count) != 1) fail("remaining acknowledgement", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  {
    type_bridge_migration_identity_t *unknown = NULL;
    type_bridge_migration_plan_t *unknown_plan = NULL;
    const type_bridge_migration_identity_t *applied[2] = {ids[0], ids[1]};
    const type_bridge_migration_identity_t *remove[1];
    CHECK(type_bridge_migration_identity_new(view("workforcev4"), view("9999_unknown"), &unknown, &diagnostics));
    remove[0] = unknown;
    if (type_bridge_migration_catalog_preview_rollback(catalog, applied, 2u, remove, 1u, &unknown_plan, &diagnostics) == TYPE_BRIDGE_STATUS_OK) fail("unknown rollback target", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
    diagnostic_code(&diagnostics, unknown_code, sizeof(unknown_code));
    CHECK(type_bridge_migration_identity_close(&unknown));
  }
  reapply_preview = preview_apply(catalog, (const type_bridge_migration_identity_t *const[]){ids[0], ids[1]}, 2u, ids[2]);
  reapply_plan = authorize(reapply_preview, 1, approval_code);
  reapply_outcome = execute_plan(reapply_plan, database, "workforce-v4-c-backfill-reapply", &diagnostics);
  if (reapply_outcome == NULL) fail("reapply backfill", TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(type_bridge_migration_execution_outcome_status(reapply_outcome, &reapply_status));

  CHECK(type_bridge_database_administration_plan_delete(administration, &deletion_plan, &diagnostics));
  CHECK(type_bridge_database_deletion_plan_execute(deletion_plan, &deletion, &diagnostics));
  CHECK(type_bridge_database_deletion_plan_close(&deletion_plan));
  CHECK(type_bridge_database_administration_plan_delete(administration, &deletion_plan, &diagnostics));
  CHECK(type_bridge_database_deletion_plan_execute(deletion_plan, &repeat_deletion, &diagnostics));
  CHECK(type_bridge_database_deletion_plan_close(&deletion_plan));
  CHECK(type_bridge_database_administration_inspect(administration, &final_state, &diagnostics));

  printf("{\"administration\":{\"create\":\"%s\",\"repeat_create\":\"%s\",\"pair_state\":\"%s\",\"delete\":\"%s\",\"repeat_delete\":\"%s\"},",
      create == 1u ? "created" : "already_exists", repeat_create == 2u ? "already_exists" : "created", pair_state == 3u ? "owned_pair" : "standalone_managed", deletion == 3u ? "deleted_owned_pair" : "deleted_standalone_managed", repeat_deletion == 1u ? "already_absent" : "unexpected");
  printf("\"rollback\":{\"apply_status\":\"%s\",\"rollback_without_approval_code\":\"%s\",\"rollback_status\":\"%s\",\"unknown_target_code\":\"%s\",\"repeat_rollback_status\":\"up_to_date\",\"reapply_status\":\"%s\"},", status_name(initial_status), approval_code, status_name(rollback_status), unknown_code, status_name(reapply_status));
  printf("\"backfill\":{\"conflict_certainty\":\"definitely_aborted\",\"conflict_code\":\"%s\",\"conflict_visible_destination_count\":%lu,\"forward_changed\":%llu,\"forward_transaction_groups\":%u,\"equal_copy_count\":%lu,\"retry_changed\":0,\"reverse_changed\":%llu,\"remaining_destination_count\":%lu},", conflict_code, conflict_count, (unsigned long long)forward_changed, forward_groups, equal_count, (unsigned long long)reverse_changed, remaining_count);
  printf("\"runtime_facade\":{\"catalog_entries\":4,\"catalog_fingerprint\":\"%s\",\"apply_order\":[\"%s\",\"%s\",\"%s\",\"%s\"],\"rollback_order\":[\"%s\",\"%s\",\"%s\",\"%s\"],\"backfill_steps\":%lu},", fingerprint_digest, apply_order[0], apply_order[1], apply_order[2], apply_order[3], rollback_order[0], rollback_order[1], rollback_order[2], rollback_order[3], (unsigned long)backfill_steps);
  printf("\"cancellation\":{\"code\":\"%s\",\"before_effect\":true},\"resource_limits\":{\"code\":\"%s\",\"bounded\":true},\"diagnostic\":{\"code\":\"%s\",\"category\":\"cancelled\",\"provider_text_absent\":true},\"lifecycle\":{\"explicit_close\":true,\"repeat_close\":true,\"temporary_evidence_absent\":true},\"cleanup\":{\"managed_database_absent\":%s,\"journal_database_absent\":%s,\"temporary_evidence_absent\":true}}\n", cancellation_code, resource_code, cancellation_code, final_state == 1u ? "true" : "false", final_state == 1u ? "true" : "false");

  CHECK(type_bridge_migration_backfill_observation_close(&reverse)); CHECK(type_bridge_migration_backfill_observation_close(&forward));
  CHECK(type_bridge_migration_execution_outcome_close(&reapply_outcome)); CHECK(type_bridge_migration_execution_outcome_close(&rollback_outcome)); CHECK(type_bridge_migration_execution_outcome_close(&forward_outcome)); CHECK(type_bridge_migration_execution_outcome_close(&initial_outcome));
  CHECK(type_bridge_migration_plan_close(&reapply_plan)); CHECK(type_bridge_migration_plan_close(&reapply_preview));
  CHECK(type_bridge_migration_plan_close(&plan)); CHECK(type_bridge_migration_plan_close(&preview)); CHECK(type_bridge_migration_plan_close(&full_rollback)); CHECK(type_bridge_migration_plan_close(&full_apply));
  for (index = 0u; index < 4u; ++index) { CHECK(type_bridge_migration_identity_close(&ids[index])); CHECK(type_bridge_migration_history_entry_close(&entries[index])); }
  CHECK(type_bridge_migration_catalog_close(&catalog));
  CHECK(type_bridge_database_administration_close(&administration));
  CHECK(type_bridge_database_close(&database, &execution_diagnostics)); CHECK(type_bridge_database_close(&database, &execution_diagnostics));
  CHECK(type_bridge_schema_package_close(&package)); CHECK(type_bridge_runtime_close(&runtime, &execution_diagnostics));
  free(bundle);
  return 0;
}
