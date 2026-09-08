#include <acme_v3/models.h>

void keyless_put(void) {
  type_bridge_status_t (*value)(
      const type_bridge_database_t *, const acme_v3_unkeyed_create *,
      const type_bridge_query_execution_limits_v1_t *,
      const type_bridge_cancellation_t *, acme_v3_unkeyed **,
      type_bridge_execution_diagnostics_t **) = acme_v3_unkeyed_database_put_v2;
  (void)value;
}
