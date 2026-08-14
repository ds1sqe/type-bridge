#include <acme_v3/models.h>

#include <stddef.h>
#include <string.h>

const type_bridge_projected_token_v1_t @MODEL_TOKEN@ = {
    sizeof(type_bridge_projected_token_v1_t),
    TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
    TYPE_BRIDGE_PROJECTED_TOKEN_MODEL,
    0u,
    {0u},
    {0u, 0u, 0u, 0u},
};

const type_bridge_projected_token_v1_t acme_v3_projected_field_token_0 = {
    sizeof(type_bridge_projected_token_v1_t),
    TYPE_BRIDGE_PROJECTED_TOKEN_VERSION,
    TYPE_BRIDGE_PROJECTED_TOKEN_FIELD,
    0u,
    {0u},
    {0u, 0u, 0u, 0u},
};

#define HANDLE(type, storage) ((type *)(void *)&(storage))
#define CONST_HANDLE(type, storage) ((const type *)(const void *)&(storage))
#define CHECK(condition)                                                        \
  do {                                                                          \
    if (!(condition)) {                                                         \
      return __LINE__;                                                          \
    }                                                                           \
  } while (0)
#define STUB_CHECK(condition)                                                   \
  do {                                                                          \
    if (!(condition) && stub_error == 0) {                                      \
      stub_error = __LINE__;                                                    \
    }                                                                           \
  } while (0)

static max_align_t database_storage;
static max_align_t package_storage;
static max_align_t cancellation_storage;
static max_align_t create_storage;
static max_align_t builder_storage;
static max_align_t batch_storage;
static max_align_t result_storage;
static max_align_t thing_storage;
static max_align_t query_binding_storage;
static max_align_t query_storage;
static max_align_t query_result_storage;
static max_align_t diagnostics_storage;
static max_align_t alternate_storage;

#define DATABASE CONST_HANDLE(type_bridge_database_t, database_storage)
#define PACKAGE CONST_HANDLE(type_bridge_schema_package_t, package_storage)
#define CANCELLATION CONST_HANDLE(type_bridge_cancellation_t, cancellation_storage)
#define CREATE CONST_HANDLE(acme_v3_keyed_create, create_storage)
#define GENERIC_CREATE CONST_HANDLE(type_bridge_projected_create_t, create_storage)
#define BUILDER HANDLE(type_bridge_projected_batch_builder_t, builder_storage)
#define NOMINAL_BUILDER HANDLE(acme_v3_keyed_insert_batch_builder, builder_storage)
#define BATCH HANDLE(type_bridge_projected_batch_t, batch_storage)
#define NOMINAL_BATCH HANDLE(acme_v3_keyed_insert_batch, batch_storage)
#define RESULT HANDLE(type_bridge_projected_batch_result_t, result_storage)
#define NOMINAL_RESULT HANDLE(acme_v3_keyed_insert_batch_result, result_storage)
#define THING HANDLE(type_bridge_projected_thing_t, thing_storage)
#define NOMINAL_THING HANDLE(acme_v3_keyed, thing_storage)
#define QUERY_BINDING HANDLE(type_bridge_query_binding_t, query_binding_storage)
#define NOMINAL_QUERY_BINDING                                                   \
  HANDLE(acme_v3_keyed_query_exact_binding, query_binding_storage)
#define QUERY HANDLE(type_bridge_query_t, query_storage)
#define NOMINAL_QUERY HANDLE(acme_v3_query, query_storage)
#define NOMINAL_FILTER HANDLE(acme_v3_keyed_manager_filter, query_storage)
#define QUERY_RESULT HANDLE(type_bridge_query_result_t, query_result_storage)
#define NOMINAL_MANAGER_RESULT                                                  \
  HANDLE(acme_v3_keyed_manager_all_result, query_result_storage)
#define DIAGNOSTICS HANDLE(type_bridge_execution_diagnostics_t, diagnostics_storage)
#define ALTERNATE(type) HANDLE(type, alternate_storage)

enum {
  PREFLIGHT_CALL_MAX = 4,
  PREFLIGHT_INPUT_MAX = 8,
  PREFLIGHT_OUTPUT_MAX = 2,
};

typedef struct preflight_record {
  size_t input_count;
  type_bridge_generated_opaque_input_v1_t inputs[PREFLIGHT_INPUT_MAX];
  size_t output_count;
  type_bridge_generated_output_range_v1_t outputs[PREFLIGHT_OUTPUT_MAX];
} preflight_record_t;

static preflight_record_t preflight_records[PREFLIGHT_CALL_MAX];
static type_bridge_status_t preflight_statuses[PREFLIGHT_CALL_MAX];
static size_t preflight_calls;
static int stub_error;
static size_t crud_calls;
static size_t builder_open_calls;
static size_t builder_add_calls;
static size_t builder_finish_calls;
static size_t builder_close_calls;
static size_t batch_close_calls;
static size_t execute_calls;
static size_t result_count_calls;
static size_t result_thing_calls;
static size_t result_close_calls;
static size_t query_binding_close_calls;
static size_t query_close_calls;
static size_t query_result_thing_calls;
static size_t query_result_close_calls;
static size_t thing_close_calls;
static size_t manager_dependency_calls;
static type_bridge_status_t crud_status;
static type_bridge_status_t builder_open_status;
static type_bridge_status_t builder_add_status;
static type_bridge_status_t builder_finish_status;
static type_bridge_status_t execute_status;
static type_bridge_status_t result_count_status;
static type_bridge_status_t result_thing_status;
static type_bridge_status_t query_binding_close_status;
static type_bridge_status_t query_close_status;
static type_bridge_status_t query_result_thing_status;
static type_bridge_status_t query_result_close_status;
static void *crud_generic_output_slot;
static void *builder_open_generic_output_slot;
static void *builder_finish_generic_owner_slot;
static void *builder_finish_generic_output_slot;
static void *execute_generic_output_slot;
static void *result_thing_generic_output_slot;
static void *query_binding_close_generic_owner_slot;
static void *query_close_generic_owner_slot;
static void *query_result_thing_generic_output_slot;
static void *query_result_close_generic_owner_slot;

static void reset_probe(void) {
  memset(preflight_records, 0, sizeof(preflight_records));
  memset(preflight_statuses, 0, sizeof(preflight_statuses));
  preflight_calls = 0u;
  stub_error = 0;
  crud_calls = 0u;
  builder_open_calls = 0u;
  builder_add_calls = 0u;
  builder_finish_calls = 0u;
  builder_close_calls = 0u;
  batch_close_calls = 0u;
  execute_calls = 0u;
  result_count_calls = 0u;
  result_thing_calls = 0u;
  result_close_calls = 0u;
  query_binding_close_calls = 0u;
  query_close_calls = 0u;
  query_result_thing_calls = 0u;
  query_result_close_calls = 0u;
  thing_close_calls = 0u;
  manager_dependency_calls = 0u;
  crud_status = TYPE_BRIDGE_STATUS_OK;
  builder_open_status = TYPE_BRIDGE_STATUS_OK;
  builder_add_status = TYPE_BRIDGE_STATUS_OK;
  builder_finish_status = TYPE_BRIDGE_STATUS_OK;
  execute_status = TYPE_BRIDGE_STATUS_OK;
  result_count_status = TYPE_BRIDGE_STATUS_OK;
  result_thing_status = TYPE_BRIDGE_STATUS_OK;
  query_binding_close_status = TYPE_BRIDGE_STATUS_OK;
  query_close_status = TYPE_BRIDGE_STATUS_OK;
  query_result_thing_status = TYPE_BRIDGE_STATUS_OK;
  query_result_close_status = TYPE_BRIDGE_STATUS_OK;
  crud_generic_output_slot = NULL;
  builder_open_generic_output_slot = NULL;
  builder_finish_generic_owner_slot = NULL;
  builder_finish_generic_output_slot = NULL;
  execute_generic_output_slot = NULL;
  result_thing_generic_output_slot = NULL;
  query_binding_close_generic_owner_slot = NULL;
  query_close_generic_owner_slot = NULL;
  query_result_thing_generic_output_slot = NULL;
  query_result_close_generic_owner_slot = NULL;
}

type_bridge_status_t type_bridge_generated_opaque_alias_preflight_v1(
    const type_bridge_generated_opaque_input_v1_t *inputs,
    size_t input_count,
    const type_bridge_generated_output_range_v1_t *outputs,
    size_t output_count) {
  size_t index;
  preflight_record_t *record;
  type_bridge_status_t status;
  if (preflight_calls >= PREFLIGHT_CALL_MAX ||
      input_count > PREFLIGHT_INPUT_MAX || output_count > PREFLIGHT_OUTPUT_MAX) {
    stub_error = __LINE__;
    return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  }
  record = &preflight_records[preflight_calls];
  record->input_count = input_count;
  for (index = 0u; index < input_count; ++index) {
    record->inputs[index] = inputs[index];
  }
  record->output_count = output_count;
  for (index = 0u; index < output_count; ++index) {
    record->outputs[index] = outputs[index];
  }
  status = preflight_statuses[preflight_calls];
  ++preflight_calls;
  return status;
}

type_bridge_status_t type_bridge_database_entity_insert_v2(
    const type_bridge_database_t *database,
    const type_bridge_projected_token_v1_t *model,
    const type_bridge_projected_create_t *create,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++crud_calls;
  crud_generic_output_slot = out_thing;
  STUB_CHECK(database == DATABASE);
  STUB_CHECK(model == &@MODEL_TOKEN@);
  STUB_CHECK(create == GENERIC_CREATE);
  STUB_CHECK(limits != NULL);
  STUB_CHECK(cancellation == CANCELLATION);
  STUB_CHECK(out_thing != NULL && *out_thing == NULL);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (crud_status == TYPE_BRIDGE_STATUS_OK) {
    *out_thing = THING;
  } else {
    *out_diagnostics = DIAGNOSTICS;
  }
  return crud_status;
}

type_bridge_status_t type_bridge_projected_batch_builder_open_v1(
    const type_bridge_schema_package_t *package,
    const type_bridge_projected_token_v1_t *model,
    type_bridge_projected_batch_operation_t operation,
    const type_bridge_query_execution_limits_v1_t *construction_limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_builder_t **out_builder,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++builder_open_calls;
  builder_open_generic_output_slot = out_builder;
  STUB_CHECK(package == PACKAGE);
  STUB_CHECK(model == &@MODEL_TOKEN@);
  STUB_CHECK(operation == TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT);
  STUB_CHECK(construction_limits != NULL);
  STUB_CHECK(cancellation == CANCELLATION);
  STUB_CHECK(out_builder != NULL && *out_builder == NULL);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (builder_open_status == TYPE_BRIDGE_STATUS_OK) {
    *out_builder = BUILDER;
  } else {
    *out_diagnostics = DIAGNOSTICS;
  }
  return builder_open_status;
}

type_bridge_status_t type_bridge_projected_batch_builder_add_v1(
    type_bridge_projected_batch_builder_t *builder,
    type_bridge_byte_view_t iid,
    const type_bridge_projected_create_t *create,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++builder_add_calls;
  STUB_CHECK(builder == BUILDER);
  STUB_CHECK(iid.data == NULL && iid.length == 0u);
  STUB_CHECK(create == GENERIC_CREATE);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (builder_add_status != TYPE_BRIDGE_STATUS_OK) {
    *out_diagnostics = DIAGNOSTICS;
  }
  return builder_add_status;
}

type_bridge_status_t type_bridge_projected_batch_builder_finish(
    type_bridge_projected_batch_builder_t **builder,
    type_bridge_projected_batch_t **out_batch,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++builder_finish_calls;
  builder_finish_generic_owner_slot = builder;
  builder_finish_generic_output_slot = out_batch;
  STUB_CHECK(builder != NULL && *builder == BUILDER);
  STUB_CHECK(out_batch != NULL && *out_batch == NULL);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (builder_finish_status == TYPE_BRIDGE_STATUS_OK) {
    *builder = NULL;
    *out_batch = BATCH;
  } else {
    *out_diagnostics = DIAGNOSTICS;
  }
  return builder_finish_status;
}

type_bridge_status_t type_bridge_projected_batch_builder_close(
    type_bridge_projected_batch_builder_t **builder) {
  ++builder_close_calls;
  STUB_CHECK(builder != NULL && *builder == BUILDER);
  *builder = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t type_bridge_projected_batch_close(
    type_bridge_projected_batch_t **batch) {
  ++batch_close_calls;
  STUB_CHECK(batch != NULL && *batch == BATCH);
  *batch = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t type_bridge_database_projected_batch_execute_v1(
    const type_bridge_database_t *database,
    const type_bridge_projected_batch_t *batch,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_projected_batch_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++execute_calls;
  execute_generic_output_slot = out_result;
  STUB_CHECK(database == DATABASE);
  STUB_CHECK(batch == BATCH);
  STUB_CHECK(limits != NULL);
  STUB_CHECK(cancellation == CANCELLATION);
  STUB_CHECK(out_result != NULL && *out_result == NULL);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (execute_status == TYPE_BRIDGE_STATUS_OK) {
    *out_result = RESULT;
  } else {
    *out_diagnostics = DIAGNOSTICS;
  }
  return execute_status;
}

type_bridge_status_t type_bridge_projected_batch_result_count(
    const type_bridge_projected_batch_result_t *result,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_batch_operation_t expected_operation,
    size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++result_count_calls;
  STUB_CHECK(result == RESULT);
  STUB_CHECK(expected_model == &@MODEL_TOKEN@);
  STUB_CHECK(expected_operation == TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT);
  STUB_CHECK(out_count != NULL && *out_count == 0u);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (result_count_status == TYPE_BRIDGE_STATUS_OK) {
    *out_count = 3u;
  } else {
    *out_diagnostics = DIAGNOSTICS;
  }
  return result_count_status;
}

type_bridge_status_t type_bridge_projected_batch_result_thing_at(
    const type_bridge_projected_batch_result_t *result,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_projected_batch_operation_t expected_operation,
    size_t index,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++result_thing_calls;
  result_thing_generic_output_slot = out_thing;
  STUB_CHECK(result == RESULT);
  STUB_CHECK(expected_model == &@MODEL_TOKEN@);
  STUB_CHECK(expected_operation == TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT);
  STUB_CHECK(index == 2u);
  STUB_CHECK(out_thing != NULL && *out_thing == NULL);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  if (result_thing_status == TYPE_BRIDGE_STATUS_OK) {
    *out_thing = THING;
  } else {
    *out_diagnostics = DIAGNOSTICS;
  }
  return result_thing_status;
}

type_bridge_status_t type_bridge_projected_batch_result_close(
    type_bridge_projected_batch_result_t **result) {
  ++result_close_calls;
  STUB_CHECK(result != NULL && *result == RESULT);
  *result = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t type_bridge_query_binding_close(
    type_bridge_query_binding_t **binding) {
  ++query_binding_close_calls;
  query_binding_close_generic_owner_slot = binding;
  STUB_CHECK(binding != NULL && *binding == QUERY_BINDING);
  if (query_binding_close_status == TYPE_BRIDGE_STATUS_OK) {
    *binding = NULL;
  }
  return query_binding_close_status;
}

type_bridge_status_t type_bridge_query_close(type_bridge_query_t **query) {
  ++query_close_calls;
  query_close_generic_owner_slot = query;
  STUB_CHECK(query != NULL && (*query == NULL || *query == QUERY));
  if (query_close_status == TYPE_BRIDGE_STATUS_OK) {
    *query = NULL;
  }
  return query_close_status;
}

type_bridge_status_t type_bridge_query_result_row_slot_thing_at(
    const type_bridge_query_result_t *result,
    type_bridge_query_result_kind_t expected_result_kind, size_t row_index,
    size_t slot_index, size_t thing_index,
    const type_bridge_projected_token_v1_t *expected_model,
    type_bridge_query_match_mode_t expected_mode,
    type_bridge_query_selection_kind_t expected_kind,
    type_bridge_projected_thing_t **out_thing,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  ++query_result_thing_calls;
  query_result_thing_generic_output_slot = out_thing;
  STUB_CHECK(result == QUERY_RESULT);
  STUB_CHECK(expected_result_kind == TYPE_BRIDGE_QUERY_RESULT_ROWS);
  STUB_CHECK(row_index == 2u && slot_index == 0u && thing_index == 0u);
  STUB_CHECK(expected_model == &@MODEL_TOKEN@);
  STUB_CHECK(expected_mode == TYPE_BRIDGE_QUERY_MATCH_EXACT);
  STUB_CHECK(expected_kind == TYPE_BRIDGE_QUERY_SELECTION_ONE);
  STUB_CHECK(out_thing != NULL && *out_thing == NULL);
  STUB_CHECK(out_diagnostics != NULL && *out_diagnostics == NULL);
  *out_thing = THING;
  if (query_result_thing_status != TYPE_BRIDGE_STATUS_OK) {
    *out_diagnostics = DIAGNOSTICS;
  }
  return query_result_thing_status;
}

type_bridge_status_t type_bridge_projected_thing_close(
    type_bridge_projected_thing_t **thing) {
  ++thing_close_calls;
  STUB_CHECK(thing != NULL && (*thing == NULL || *thing == THING));
  *thing = NULL;
  return TYPE_BRIDGE_STATUS_OK;
}

type_bridge_status_t type_bridge_query_result_close(
    type_bridge_query_result_t **result) {
  ++query_result_close_calls;
  query_result_close_generic_owner_slot = result;
  STUB_CHECK(result != NULL && *result == QUERY_RESULT);
  if (query_result_close_status == TYPE_BRIDGE_STATUS_OK) {
    *result = NULL;
  }
  return query_result_close_status;
}

type_bridge_status_t type_bridge_query_field_open(
    const type_bridge_query_binding_t *binding,
    const type_bridge_projected_token_v1_t *field,
    type_bridge_query_field_t **out_field,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)binding;
  (void)field;
  (void)out_field;
  (void)out_diagnostics;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_field_compare_value(
    const type_bridge_query_field_t *field,
    const type_bridge_projected_token_v1_t *expected_field,
    type_bridge_query_comparison_t comparison,
    const type_bridge_projected_value_t *value,
    type_bridge_query_predicate_t **out_predicate,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)field;
  (void)expected_field;
  (void)comparison;
  (void)value;
  (void)out_predicate;
  (void)out_diagnostics;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_field_close(
    type_bridge_query_field_t **field) {
  (void)field;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_predicate_close(
    type_bridge_query_predicate_t **predicate) {
  (void)predicate;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_where(
    const type_bridge_query_t *query,
    const type_bridge_query_predicate_t *predicate,
    type_bridge_query_t **out_query,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)query;
  (void)predicate;
  (void)out_query;
  (void)out_diagnostics;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_terminal_open_v1(
    const type_bridge_query_t *query,
    const type_bridge_query_terminal_descriptor_v1_t *descriptor,
    type_bridge_query_terminal_t **out_terminal,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)query;
  (void)descriptor;
  (void)out_terminal;
  (void)out_diagnostics;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_terminal_close(
    type_bridge_query_terminal_t **terminal) {
  (void)terminal;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_database_query_execute_v1(
    const type_bridge_database_t *database,
    const type_bridge_query_terminal_t *terminal,
    type_bridge_query_terminal_kind_t expected_terminal_kind,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation,
    type_bridge_query_result_t **out_result,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)database;
  (void)terminal;
  (void)expected_terminal_kind;
  (void)limits;
  (void)cancellation;
  (void)out_result;
  (void)out_diagnostics;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

type_bridge_status_t type_bridge_query_result_count(
    const type_bridge_query_result_t *result, uint64_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics) {
  (void)result;
  (void)out_count;
  (void)out_diagnostics;
  ++manager_dependency_calls;
  return TYPE_BRIDGE_STATUS_PANIC;
}

#define CHECK_INPUT(call, index, expected_kind, expected_pointer, expected_count) \
  CHECK(preflight_records[(call)].inputs[(index)].kind == (expected_kind));       \
  CHECK(preflight_records[(call)].inputs[(index)].pointer ==                     \
        (const void *)(expected_pointer));                                       \
  CHECK(preflight_records[(call)].inputs[(index)].count == (expected_count))
#define CHECK_OUTPUT(call, index, expected_pointer, expected_length)             \
  CHECK(preflight_records[(call)].outputs[(index)].pointer ==                    \
        (void *)(expected_pointer));                                             \
  CHECK(preflight_records[(call)].outputs[(index)].length == (expected_length))

int main(void) {
  type_bridge_query_execution_limits_v1_t limits =
      TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;
  type_bridge_status_t status;
  acme_v3_keyed *thing;
  acme_v3_keyed_insert_batch_builder *builder;
  acme_v3_keyed_insert_batch *batch;
  acme_v3_keyed_insert_batch_result *result;
  acme_v3_keyed_manager manager;
  acme_v3_keyed_manager manager_snapshot;
  acme_v3_keyed_manager_filter *filter;
  acme_v3_keyed_manager_filter_ref_v1_t filter_ref;
  acme_v3_keyed_manager_all_result *manager_result;
  type_bridge_execution_diagnostics_t *diagnostics;
  uint64_t manager_count;
  size_t count;

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  thing = ALTERNATE(acme_v3_keyed);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_database_insert_v2(
      DATABASE, CREATE, &limits, CANCELLATION, &thing, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(thing == ALTERNATE(acme_v3_keyed));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(crud_calls == 0u && preflight_calls == 1u && stub_error == 0);
  CHECK(preflight_records[0].input_count == 5u);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_DATABASE, DATABASE, 1u);
  CHECK_INPUT(0, 1, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN,
              &@MODEL_TOKEN@, 1u);
  CHECK_INPUT(0, 2, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE, CREATE, 1u);
  CHECK_INPUT(0, 3, TYPE_BRIDGE_GENERATED_INPUT_BYTES, &limits, sizeof(limits));
  CHECK_INPUT(0, 4, TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION, CANCELLATION, 1u);
  CHECK(preflight_records[0].output_count == 2u);
  CHECK_OUTPUT(0, 0, &thing, sizeof(thing));
  CHECK_OUTPUT(0, 1, &diagnostics, sizeof(diagnostics));

  reset_probe();
  thing = ALTERNATE(acme_v3_keyed);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_database_insert_v2(
      DATABASE, CREATE, &limits, CANCELLATION, &thing, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && thing == NOMINAL_THING);
  CHECK(diagnostics == NULL && crud_calls == 1u && stub_error == 0);
  CHECK(crud_generic_output_slot != (void *)&thing);

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  builder = ALTERNATE(acme_v3_keyed_insert_batch_builder);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_open(
      PACKAGE, &limits, CANCELLATION, &builder, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(builder == ALTERNATE(acme_v3_keyed_insert_batch_builder));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(builder_open_calls == 0u && preflight_calls == 1u && stub_error == 0);

  reset_probe();
  builder = ALTERNATE(acme_v3_keyed_insert_batch_builder);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_open(
      PACKAGE, &limits, CANCELLATION, &builder, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && builder == NOMINAL_BUILDER);
  CHECK(diagnostics == NULL && builder_open_calls == 1u && stub_error == 0);
  CHECK(builder_open_generic_output_slot != (void *)&builder);

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_add(builder, CREATE, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(builder_add_calls == 0u && preflight_calls == 1u && stub_error == 0);
  CHECK(preflight_records[0].input_count == 2u);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER,
              BUILDER, 1u);
  CHECK_INPUT(0, 1, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE, CREATE, 1u);

  reset_probe();
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_add(builder, CREATE, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && diagnostics == NULL);
  CHECK(builder_add_calls == 1u && stub_error == 0);

  reset_probe();
  preflight_statuses[1] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  builder = NOMINAL_BUILDER;
  batch = ALTERNATE(acme_v3_keyed_insert_batch);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_finish(
      &builder, &batch, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(builder == NOMINAL_BUILDER);
  CHECK(batch == ALTERNATE(acme_v3_keyed_insert_batch));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(builder_finish_calls == 0u && preflight_calls == 2u && stub_error == 0);
  CHECK(preflight_records[0].input_count == 1u);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER,
              BUILDER, 1u);
  CHECK(preflight_records[0].output_count == 2u);
  CHECK_OUTPUT(0, 0, &builder, sizeof(builder));
  CHECK_OUTPUT(0, 1, &batch, sizeof(batch));
  CHECK(preflight_records[1].input_count == 3u);
  CHECK_INPUT(1, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER,
              BUILDER, 1u);
  CHECK_INPUT(1, 1, TYPE_BRIDGE_GENERATED_INPUT_BYTES, &builder,
              sizeof(builder));
  CHECK_INPUT(1, 2, TYPE_BRIDGE_GENERATED_INPUT_BYTES, &batch, sizeof(batch));
  CHECK(preflight_records[1].output_count == 1u);
  CHECK_OUTPUT(1, 0, &diagnostics, sizeof(diagnostics));

  reset_probe();
  builder_finish_status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  builder = NOMINAL_BUILDER;
  batch = ALTERNATE(acme_v3_keyed_insert_batch);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_finish(
      &builder, &batch, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  CHECK(builder == NOMINAL_BUILDER && batch == NULL && diagnostics == DIAGNOSTICS);
  CHECK(builder_finish_calls == 1u && stub_error == 0);
  CHECK(builder_finish_generic_owner_slot != (void *)&builder);
  CHECK(builder_finish_generic_output_slot != (void *)&batch);

  reset_probe();
  builder = NOMINAL_BUILDER;
  batch = ALTERNATE(acme_v3_keyed_insert_batch);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_builder_finish(
      &builder, &batch, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK);
  CHECK(builder == NULL && batch == NOMINAL_BATCH && diagnostics == NULL);
  CHECK(builder_finish_calls == 1u && stub_error == 0);

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  result = ALTERNATE(acme_v3_keyed_insert_batch_result);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_database_insert_batch_execute(
      DATABASE, batch, &limits, CANCELLATION, &result, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(result == ALTERNATE(acme_v3_keyed_insert_batch_result));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(execute_calls == 0u && preflight_calls == 1u && stub_error == 0);

  reset_probe();
  result = ALTERNATE(acme_v3_keyed_insert_batch_result);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_database_insert_batch_execute(
      DATABASE, batch, &limits, CANCELLATION, &result, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && result == NOMINAL_RESULT);
  CHECK(diagnostics == NULL && execute_calls == 1u && stub_error == 0);
  CHECK(execute_generic_output_slot != (void *)&result);
  CHECK_INPUT(0, 1, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH, BATCH, 1u);

  reset_probe();
  count = 99u;
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_result_count(result, &count, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && count == 3u && diagnostics == NULL);
  CHECK(result_count_calls == 1u && stub_error == 0);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT,
              RESULT, 1u);
  CHECK_INPUT(0, 1, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN,
              &@MODEL_TOKEN@, 1u);

  reset_probe();
  thing = ALTERNATE(acme_v3_keyed);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_insert_batch_result_thing_at(
      result, 2u, &thing, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && thing == NOMINAL_THING);
  CHECK(diagnostics == NULL && result_thing_calls == 1u && stub_error == 0);
  CHECK(result_thing_generic_output_slot != (void *)&thing);

  reset_probe();
  builder = NOMINAL_BUILDER;
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  status = acme_v3_keyed_insert_batch_builder_close(&builder);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT && builder == NOMINAL_BUILDER);
  CHECK(builder_close_calls == 0u && stub_error == 0);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_BUILDER,
              BUILDER, 1u);

  reset_probe();
  builder = NOMINAL_BUILDER;
  status = acme_v3_keyed_insert_batch_builder_close(&builder);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && builder == NULL);
  CHECK(builder_close_calls == 1u && stub_error == 0);

  reset_probe();
  status = acme_v3_keyed_insert_batch_close(&batch);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && batch == NULL);
  CHECK(batch_close_calls == 1u && stub_error == 0);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH, BATCH, 1u);

  reset_probe();
  status = acme_v3_keyed_insert_batch_result_close(&result);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && result == NULL);
  CHECK(result_close_calls == 1u && stub_error == 0);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_BATCH_RESULT,
              RESULT, 1u);

  reset_probe();
  memset(&manager, 0, sizeof(manager));
  manager.binding = NOMINAL_QUERY_BINDING;
  manager.root_query = NOMINAL_QUERY;
  manager.reserved[2] = 1u;
  manager_snapshot = manager;
  status = acme_v3_keyed_manager_close(&manager);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(memcmp(&manager, &manager_snapshot, sizeof(manager)) == 0);
  CHECK(preflight_calls == 1u && query_close_calls == 0u &&
        query_binding_close_calls == 0u && stub_error == 0);
  CHECK(preflight_records[0].input_count == 2u);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_QUERY_BINDING, QUERY_BINDING,
              1u);
  CHECK_INPUT(0, 1, TYPE_BRIDGE_GENERATED_INPUT_QUERY, QUERY, 1u);
  CHECK_OUTPUT(0, 0, &manager, sizeof(manager));

  reset_probe();
  manager.reserved[2] = 0u;
  manager_snapshot = manager;
  query_close_status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  status = acme_v3_keyed_manager_close(&manager);
  CHECK(status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  CHECK(memcmp(&manager, &manager_snapshot, sizeof(manager)) == 0);
  CHECK(query_close_calls == 1u && query_binding_close_calls == 0u &&
        stub_error == 0);
  CHECK(query_close_generic_owner_slot != (void *)&manager.root_query);

  reset_probe();
  query_binding_close_status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  status = acme_v3_keyed_manager_close(&manager);
  CHECK(status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  CHECK(manager.binding == NOMINAL_QUERY_BINDING && manager.root_query == NULL);
  CHECK(query_close_calls == 1u && query_binding_close_calls == 1u &&
        stub_error == 0);
  CHECK(query_close_generic_owner_slot != (void *)&manager.root_query);
  CHECK(query_binding_close_generic_owner_slot != (void *)&manager.binding);

  reset_probe();
  status = acme_v3_keyed_manager_close(&manager);
  CHECK(status == TYPE_BRIDGE_STATUS_OK);
  CHECK(manager.binding == NULL && manager.root_query == NULL);
  CHECK(query_close_calls == 1u && query_binding_close_calls == 1u &&
        stub_error == 0);

  reset_probe();
  memset(&manager, 0, sizeof(manager));
  manager.binding = NOMINAL_QUERY_BINDING;
  manager.root_query = NOMINAL_QUERY;
  filter_ref = acme_v3_keyed_manager_filter_root(&manager);
  manager.reserved[0] = 1u;
  manager_snapshot = manager;
  filter = ALTERNATE(acme_v3_keyed_manager_filter);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_filter_identifier_eq(
      &manager, filter_ref, CONST_HANDLE(acme_v3_identifier, create_storage),
      &filter, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(memcmp(&manager, &manager_snapshot, sizeof(manager)) == 0);
  CHECK(filter == ALTERNATE(acme_v3_keyed_manager_filter));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(preflight_calls == 1u && manager_dependency_calls == 0u &&
        stub_error == 0);

  reset_probe();
  manager.reserved[0] = 0u;
  filter_ref = acme_v3_keyed_manager_filter_root(&manager);
  filter_ref.reserved[1] = 1u;
  filter = ALTERNATE(acme_v3_keyed_manager_filter);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_filter_identifier_eq(
      &manager, filter_ref, CONST_HANDLE(acme_v3_identifier, create_storage),
      &filter, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(filter == ALTERNATE(acme_v3_keyed_manager_filter));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(preflight_calls == 1u && manager_dependency_calls == 0u &&
        stub_error == 0);

  reset_probe();
  filter_ref = acme_v3_keyed_manager_filter_root(&manager);
  manager_snapshot = manager;
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  status = acme_v3_keyed_manager_filter_identifier_eq(
      &manager, filter_ref, CONST_HANDLE(acme_v3_identifier, create_storage),
      (acme_v3_keyed_manager_filter **)(void *)&manager.root_query,
      &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(memcmp(&manager, &manager_snapshot, sizeof(manager)) == 0);
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(preflight_calls == 1u && manager_dependency_calls == 0u &&
        stub_error == 0);
  CHECK_OUTPUT(0, 0, &manager.root_query, sizeof(manager.root_query));

  reset_probe();
  manager.reserved[0] = 1u;
  manager_snapshot = manager;
  filter_ref = acme_v3_keyed_manager_filter_root(&manager);
  manager_count = UINT64_C(99);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_database_count(
      DATABASE, &manager, filter_ref, &limits, CANCELLATION, &manager_count,
      &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(memcmp(&manager, &manager_snapshot, sizeof(manager)) == 0);
  CHECK(manager_count == UINT64_C(99));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(preflight_calls == 1u && manager_dependency_calls == 0u &&
        stub_error == 0);

  reset_probe();
  manager.reserved[0] = 0u;
  filter_ref = acme_v3_keyed_manager_filter_root(&manager);
  filter_ref.reserved[3] = 1u;
  manager_count = UINT64_C(99);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_database_count(
      DATABASE, &manager, filter_ref, &limits, CANCELLATION, &manager_count,
      &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(manager_count == UINT64_C(99));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(preflight_calls == 1u && manager_dependency_calls == 0u &&
        stub_error == 0);

  reset_probe();
  filter_ref = acme_v3_keyed_manager_filter_root(&manager);
  manager_snapshot = manager;
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  status = acme_v3_keyed_manager_database_count(
      DATABASE, &manager, filter_ref, &limits, CANCELLATION,
      &manager.reserved[0], &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(memcmp(&manager, &manager_snapshot, sizeof(manager)) == 0);
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(preflight_calls == 1u && manager_dependency_calls == 0u &&
        stub_error == 0);
  CHECK_OUTPUT(0, 0, &manager.reserved[0], sizeof(manager.reserved[0]));

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  filter = NOMINAL_FILTER;
  status = acme_v3_keyed_manager_filter_close(&filter);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
        filter == NOMINAL_FILTER);
  CHECK(query_close_calls == 0u && preflight_calls == 1u && stub_error == 0);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_QUERY, QUERY, 1u);
  CHECK_OUTPUT(0, 0, &filter, sizeof(filter));

  reset_probe();
  query_close_status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  filter = NOMINAL_FILTER;
  status = acme_v3_keyed_manager_filter_close(&filter);
  CHECK(status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED &&
        filter == NOMINAL_FILTER);
  CHECK(query_close_calls == 1u && stub_error == 0);
  CHECK(query_close_generic_owner_slot != (void *)&filter);

  reset_probe();
  filter = NOMINAL_FILTER;
  status = acme_v3_keyed_manager_filter_close(&filter);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && filter == NULL);
  CHECK(query_close_calls == 1u && stub_error == 0);
  CHECK(query_close_generic_owner_slot != (void *)&filter);

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  manager_result = NOMINAL_MANAGER_RESULT;
  thing = ALTERNATE(acme_v3_keyed);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_all_result_at(
      manager_result, 2u, &thing, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT);
  CHECK(thing == ALTERNATE(acme_v3_keyed));
  CHECK(diagnostics == ALTERNATE(type_bridge_execution_diagnostics_t));
  CHECK(query_result_thing_calls == 0u && thing_close_calls == 0u &&
        preflight_calls == 1u && stub_error == 0);
  CHECK(preflight_records[0].input_count == 2u);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_QUERY_RESULT, QUERY_RESULT,
              1u);
  CHECK_INPUT(0, 1, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN,
              &@MODEL_TOKEN@, 1u);
  CHECK_OUTPUT(0, 0, &thing, sizeof(thing));
  CHECK_OUTPUT(0, 1, &diagnostics, sizeof(diagnostics));

  reset_probe();
  query_result_thing_status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  manager_result = NOMINAL_MANAGER_RESULT;
  thing = ALTERNATE(acme_v3_keyed);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_all_result_at(
      manager_result, 2u, &thing, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED);
  CHECK(thing == NULL && diagnostics == DIAGNOSTICS);
  CHECK(query_result_thing_calls == 1u && thing_close_calls == 1u &&
        stub_error == 0);
  CHECK(query_result_thing_generic_output_slot != (void *)&thing);

  reset_probe();
  manager_result = NOMINAL_MANAGER_RESULT;
  thing = ALTERNATE(acme_v3_keyed);
  diagnostics = ALTERNATE(type_bridge_execution_diagnostics_t);
  status = acme_v3_keyed_manager_all_result_at(
      manager_result, 2u, &thing, &diagnostics);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && thing == NOMINAL_THING &&
        diagnostics == NULL);
  CHECK(query_result_thing_calls == 1u && thing_close_calls == 1u &&
        stub_error == 0);
  CHECK(query_result_thing_generic_output_slot != (void *)&thing);

  reset_probe();
  preflight_statuses[0] = TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  manager_result = NOMINAL_MANAGER_RESULT;
  status = acme_v3_keyed_manager_all_result_close(&manager_result);
  CHECK(status == TYPE_BRIDGE_STATUS_INVALID_ARGUMENT &&
        manager_result == NOMINAL_MANAGER_RESULT);
  CHECK(query_result_close_calls == 0u && preflight_calls == 1u &&
        stub_error == 0);
  CHECK_INPUT(0, 0, TYPE_BRIDGE_GENERATED_INPUT_QUERY_RESULT, QUERY_RESULT,
              1u);
  CHECK_OUTPUT(0, 0, &manager_result, sizeof(manager_result));

  reset_probe();
  query_result_close_status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED;
  manager_result = NOMINAL_MANAGER_RESULT;
  status = acme_v3_keyed_manager_all_result_close(&manager_result);
  CHECK(status == TYPE_BRIDGE_STATUS_EXECUTION_FAILED &&
        manager_result == NOMINAL_MANAGER_RESULT);
  CHECK(query_result_close_calls == 1u && stub_error == 0);
  CHECK(query_result_close_generic_owner_slot != (void *)&manager_result);

  reset_probe();
  manager_result = NOMINAL_MANAGER_RESULT;
  status = acme_v3_keyed_manager_all_result_close(&manager_result);
  CHECK(status == TYPE_BRIDGE_STATUS_OK && manager_result == NULL);
  CHECK(query_result_close_calls == 1u && stub_error == 0);
  CHECK(query_result_close_generic_owner_slot != (void *)&manager_result);

  return 0;
}
