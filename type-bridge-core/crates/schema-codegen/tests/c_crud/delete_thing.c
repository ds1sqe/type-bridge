#include <acme_v3/models.h>

void delete_thing(void) {
  type_bridge_status_t (*value)(
      const acme_v3_gathering_delete_batch_result *, size_t,
      acme_v3_gathering **, type_bridge_execution_diagnostics_t **) =
      acme_v3_gathering_delete_batch_result_thing_at;
  (void)value;
}
