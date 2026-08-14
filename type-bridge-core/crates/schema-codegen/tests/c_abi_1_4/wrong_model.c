#include <acme_v3/models.h>

type_bridge_status_t wrong_model(
    acme_v3_keyed_insert_batch_builder *builder,
    const acme_v3_unkeyed_create *wrong,
    type_bridge_execution_diagnostics_t **diagnostics) {
  return acme_v3_keyed_insert_batch_builder_add(builder, wrong, diagnostics);
}
