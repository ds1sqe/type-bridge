#include <acme_v3/models.h>

type_bridge_status_t wrong_model(
    acme_v3_keyed_insert_batch_builder *builder,
    const acme_v3_unkeyed_create *wrong,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return acme_v3_keyed_insert_batch_builder_add(builder, wrong, diagnostics);
}

type_bridge_status_t wrong_manager_filter(
    const acme_v3_keyed_manager *manager,
    acme_v3_membership_manager_filter_ref_v1_t wrong,
    const acme_v3_identifier *value,
    acme_v3_keyed_manager_filter **out_filter,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return acme_v3_keyed_manager_filter_identifier_eq(
      manager, wrong, value, out_filter, diagnostics);
}

type_bridge_status_t wrong_manager_result(
    const acme_v3_membership_manager_all_result *wrong, acme_v3_keyed **out,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return acme_v3_keyed_manager_all_result_at(wrong, 0u, out, diagnostics);
}
