#include <acme_nominal/models.h>

static type_bridge_status_t (*const keyed_insert)(
    const type_bridge_database_t *, const acme_nominal_keyed_create *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, acme_nominal_keyed_insert_batch_result **,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_database_insert;
static type_bridge_status_t (*const keyed_add)(
    acme_nominal_keyed_insert_batch_builder *, const acme_nominal_keyed_create *,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_insert_batch_builder_add;
static type_bridge_status_t (*const keyed_update_add)(
    acme_nominal_keyed_update_batch_builder *, const type_bridge_iid_t *,
    const acme_nominal_keyed_create *, type_bridge_execution_diagnostics_t **) =
    acme_nominal_keyed_update_batch_builder_add;
static type_bridge_status_t (*const gathering_delete_add)(
    acme_nominal_gathering_delete_batch_builder *, const type_bridge_iid_t *,
    type_bridge_execution_diagnostics_t **) = acme_nominal_gathering_delete_batch_builder_add;
static type_bridge_status_t (*const membership_execute)(
    const type_bridge_write_transaction_t *, const acme_nominal_membership_put_batch *,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, acme_nominal_membership_put_batch_result **,
    type_bridge_execution_diagnostics_t **) =
    acme_nominal_membership_write_transaction_put_batch_execute;
static type_bridge_status_t (*const keyed_thing_at)(
    const acme_nominal_keyed_insert_batch_result *, size_t, acme_nominal_keyed **,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_insert_batch_result_thing_at;
static type_bridge_status_t (*const gathering_delete_count)(
    const acme_nominal_gathering_delete_batch_result *, size_t *,
    type_bridge_execution_diagnostics_t **) = acme_nominal_gathering_delete_batch_result_count;
static type_bridge_status_t (*const keyed_manager_open)(
    const type_bridge_schema_package_t *, acme_nominal_keyed_manager *,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_manager_open;
static type_bridge_status_t (*const keyed_manager_identifier_eq)(
    const acme_nominal_keyed_manager *, acme_nominal_keyed_manager_filter_ref_v1_t,
    const acme_nominal_identifier *, acme_nominal_keyed_manager_filter **,
    type_bridge_execution_diagnostics_t **) =
    acme_nominal_keyed_manager_filter_identifier_eq;
static type_bridge_status_t (*const keyed_manager_database_all)(
    const type_bridge_database_t *, const acme_nominal_keyed_manager *,
    acme_nominal_keyed_manager_filter_ref_v1_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, acme_nominal_keyed_manager_all_result **,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_manager_database_all;
static type_bridge_status_t (*const keyed_manager_read_first)(
    const type_bridge_read_transaction_t *, const acme_nominal_keyed_manager *,
    acme_nominal_keyed_manager_filter_ref_v1_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, acme_nominal_keyed **,
    type_bridge_execution_diagnostics_t **) =
    acme_nominal_keyed_manager_read_transaction_first;
static type_bridge_status_t (*const keyed_manager_database_count)(
    const type_bridge_database_t *, const acme_nominal_keyed_manager *,
    acme_nominal_keyed_manager_filter_ref_v1_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint64_t *,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_manager_database_count;
static type_bridge_status_t (*const keyed_manager_read_exists)(
    const type_bridge_read_transaction_t *, const acme_nominal_keyed_manager *,
    acme_nominal_keyed_manager_filter_ref_v1_t,
    const type_bridge_query_execution_limits_v1_t *,
    const type_bridge_cancellation_t *, uint8_t *,
    type_bridge_execution_diagnostics_t **) =
    acme_nominal_keyed_manager_read_transaction_exists;
static type_bridge_status_t (*const keyed_manager_result_at)(
    const acme_nominal_keyed_manager_all_result *, size_t, acme_nominal_keyed **,
    type_bridge_execution_diagnostics_t **) = acme_nominal_keyed_manager_all_result_at;

int main(void) {
  (void)keyed_insert;
  (void)keyed_add;
  (void)keyed_update_add;
  (void)gathering_delete_add;
  (void)membership_execute;
  (void)keyed_thing_at;
  (void)gathering_delete_count;
  (void)keyed_manager_open;
  (void)keyed_manager_identifier_eq;
  (void)keyed_manager_database_all;
  (void)keyed_manager_read_first;
  (void)keyed_manager_database_count;
  (void)keyed_manager_read_exists;
  (void)keyed_manager_result_at;
  return 0;
}

void unkeyed_put_remains_available(void) {
  type_bridge_status_t (*value)(
      const type_bridge_database_t *, const acme_nominal_unkeyed_create *,
      const type_bridge_query_execution_limits_v1_t *,
      const type_bridge_cancellation_t *, acme_nominal_unkeyed_put_batch_result **,
      type_bridge_execution_diagnostics_t **) = acme_nominal_unkeyed_database_put;
  (void)value;
}
